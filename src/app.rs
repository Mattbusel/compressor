//! app: the egui interface, the custom theme, and the async to UI bridge.
//!
//! The UI is modeled as an explicit state machine ([`UiState`]). The network
//! call runs on a tokio runtime and its result is handed back to the UI thread
//! through an `mpsc` channel that we poll once per frame, so the interface never
//! freezes while a request is in flight.
//!
//! Rust primitives used here:
//!   - `enum` sum type for state: `UiState` makes the four possible screens
//!     (Idle, Loading, Result, Error) explicit and mutually exclusive.
//!   - `struct` holding owned state: `CompressorApp` owns the runtime, the input
//!     buffers, the current state, and the receiver end of the channel.
//!   - `mpsc` channel: `std::sync::mpsc` carries the result from the spawned
//!     task back to the UI thread. `try_recv` is non blocking.
//!   - tokio runtime + `spawn`: runs `engine::compress` off the UI thread.
//!   - `Clone` on `egui::Context`: a clone is moved into the task so it can
//!     request a repaint exactly when the result is ready.
//!   - trait impl: `impl eframe::App for CompressorApp` plugs our type into the
//!     eframe event loop via the `update` method.
//!   - design tokens: named `const` values for color, spacing, type scale, and
//!     radius, so the look is defined in one place.

use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;
use egui::{
    Align, Button, Color32, FontFamily, FontId, Frame, Layout, Margin, RichText, Rounding,
    ScrollArea, Spinner, Stroke, TextEdit, TextStyle, Vec2,
};

use crate::domain::{Compression, PlacementContent, ProductInput, AudienceInput};
use crate::engine::{self, EngineError};

// --- Design tokens --------------------------------------------------------
//
// Color tokens. A deep, slightly blue charcoal base with a single confident
// accent (warm coral) used sparingly for the primary action and small markers.

const BG: Color32 = Color32::from_rgb(0x0F, 0x11, 0x16); // app background
const SURFACE: Color32 = Color32::from_rgb(0x16, 0x19, 0x20); // cards
const SURFACE_ALT: Color32 = Color32::from_rgb(0x1E, 0x22, 0x2B); // inputs, chips
const BORDER: Color32 = Color32::from_rgb(0x2A, 0x2F, 0x3A); // hairline borders
const TEXT: Color32 = Color32::from_rgb(0xE6, 0xE8, 0xEC); // primary text
const MUTED: Color32 = Color32::from_rgb(0x97, 0x9F, 0xAD); // secondary text
const ACCENT: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x3D); // the one accent
const ON_ACCENT: Color32 = Color32::from_rgb(0x16, 0x0A, 0x05); // text on accent
const DANGER: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x6B); // error text

// Spacing scale. One source of truth for vertical and horizontal rhythm.
const SP_XS: f32 = 4.0;
const SP_SM: f32 = 8.0;
const SP_MD: f32 = 16.0;
const SP_LG: f32 = 24.0;
const SP_XL: f32 = 32.0;

// Type scale. Sizes used across the interface.
const TS_DISPLAY: f32 = 30.0;
const TS_HEADING: f32 = 20.0;
const TS_BODY: f32 = 15.0;
const TS_SMALL: f32 = 13.0;
const TS_EYEBROW: f32 = 12.0;

// Corner radius for surfaces and controls.
const RADIUS: f32 = 12.0;
const RADIUS_SM: f32 = 8.0;

/// Install the custom dark theme onto the egui context.
///
/// This sets the type scale (text styles), the color palette (visuals), and the
/// spacing defaults. Doing it once at startup means every widget inherits the
/// designed look without per call styling.
fn install_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    // Type scale: map each semantic text style to a concrete font size.
    style.text_styles = [
        (TextStyle::Heading, FontId::new(TS_HEADING, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(TS_BODY, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(TS_BODY, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(TS_SMALL, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(TS_SMALL, FontFamily::Monospace)),
    ]
    .into();

    // Color palette and surface treatment.
    let v = &mut style.visuals;
    v.dark_mode = true;
    v.override_text_color = Some(TEXT);
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.extreme_bg_color = SURFACE_ALT; // text edit background
    v.faint_bg_color = SURFACE;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(0xFF, 0x6B, 0x3D, 0x40);
    v.selection.stroke = Stroke::new(1.0, ACCENT);

    // Rounded, bordered widgets across every interaction state.
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.rounding = Rounding::same(RADIUS_SM);
        w.bg_fill = SURFACE_ALT;
        w.weak_bg_fill = SURFACE_ALT;
        w.fg_stroke = Stroke::new(1.0, TEXT);
        w.bg_stroke = Stroke::new(1.0, BORDER);
    }
    v.window_rounding = Rounding::same(RADIUS);

    // Spacing rhythm.
    style.spacing.item_spacing = Vec2::new(SP_SM, SP_SM);
    style.spacing.button_padding = Vec2::new(SP_MD, SP_SM);

    ctx.set_style(style);
}

/// The explicit UI state machine.
///
/// `enum` sum type: the interface is in exactly one of these states at any time,
/// which removes whole classes of "loading and showing a result at once" bugs.
enum UiState {
    Idle,
    Loading,
    Result(Compression),
    Error(String),
}

/// The application. Owns everything the UI needs to function.
pub struct CompressorApp {
    /// The tokio runtime that drives the async engine call off the UI thread.
    runtime: tokio::runtime::Runtime,
    /// The two text buffers bound to the input fields.
    product_text: String,
    audience_text: String,
    /// The current screen.
    state: UiState,
    /// Receiver for the in flight request's result, if any.
    ///
    /// `Option<Receiver<...>>`: `Some` while a request is pending, `None`
    /// otherwise. The result type is exactly what `engine::compress` returns.
    rx: Option<Receiver<Result<Compression, EngineError>>>,
}

impl CompressorApp {
    /// Build the app: install the theme and start the runtime.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_theme(&cc.egui_ctx);

        // A small multi threaded runtime is plenty for a single request at a
        // time. `enable_all` turns on the IO and timer drivers reqwest needs.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to start the tokio runtime");

        Self {
            runtime,
            product_text: String::new(),
            audience_text: String::new(),
            state: UiState::Idle,
            rx: None,
        }
    }

    /// Validate the inputs and, if good, spawn the async request.
    ///
    /// This is the only place that transitions into `Loading`. On invalid input
    /// it transitions straight to `Error` without touching the network.
    fn submit(&mut self, ctx: &egui::Context) {
        let product = match ProductInput::new(&self.product_text) {
            Ok(p) => p,
            Err(e) => {
                self.state = UiState::Error(e.to_string());
                return;
            }
        };
        let audience = match AudienceInput::new(&self.audience_text) {
            Ok(a) => a,
            Err(e) => {
                self.state = UiState::Error(e.to_string());
                return;
            }
        };

        // mpsc channel: `tx` moves into the task, `rx` stays here and is polled
        // each frame. The channel decouples the worker thread from the UI thread.
        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        self.state = UiState::Loading;

        // Clone the context so the task can wake the UI exactly when done.
        let ctx = ctx.clone();
        self.runtime.spawn(async move {
            let result = engine::compress(product, audience).await;
            // If the receiver is gone (window closed), the send simply fails;
            // ignore that with `let _ =`.
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    /// Poll the channel once. Called at the top of every frame.
    ///
    /// `try_recv` never blocks: it returns immediately whether or not a value is
    /// ready, which is what keeps the UI responsive.
    fn poll_result(&mut self) {
        let Some(rx) = &self.rx else {
            return;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.state = match result {
                    Ok(compression) => UiState::Result(compression),
                    Err(error) => UiState::Error(error.to_string()),
                };
                self.rx = None;
            }
            Err(TryRecvError::Empty) => {} // still working
            Err(TryRecvError::Disconnected) => {
                self.state = UiState::Error("the worker task ended unexpectedly".to_owned());
                self.rx = None;
            }
        }
    }
}

/// trait impl: this is what makes `CompressorApp` an eframe application. eframe
/// calls `update` every frame; everything the user sees is drawn here.
impl eframe::App for CompressorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Step 1: drain any finished request into the state machine.
        self.poll_result();

        // Step 2: draw. The central panel uses our background and generous
        // margins so content breathes.
        egui::CentralPanel::default()
            .frame(
                Frame::none()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(SP_XL, SP_LG)),
            )
            .show(ctx, |ui| {
                ScrollArea::vertical().show(ui, |ui| {
                    self.header(ui);
                    ui.add_space(SP_LG);
                    self.inputs(ui, ctx);
                    ui.add_space(SP_LG);
                    self.output(ui);
                });
            });
    }
}

impl CompressorApp {
    /// Title block: product name, the thesis in one line, and an accent rule.
    fn header(&self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Compressor").size(TS_DISPLAY).strong().color(TEXT));
        ui.add_space(SP_XS);
        ui.label(
            RichText::new("Remove the layers between a product and the customer who needs it.")
                .size(TS_BODY)
                .color(MUTED),
        );
        ui.add_space(SP_SM);
        // A short accent rule: the single confident accent, used sparingly.
        let (rect, _) = ui.allocate_exact_size(Vec2::new(64.0, 3.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, Rounding::same(2.0), ACCENT);
    }

    /// The two input fields and the primary action.
    fn inputs(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        card(ui, |ui| {
            field_label(ui, "PRODUCT");
            ui.add_space(SP_XS);
            ui.add(
                TextEdit::multiline(&mut self.product_text)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .hint_text("What is the product? Describe it plainly."),
            );

            ui.add_space(SP_MD);

            field_label(ui, "WHO YOU THINK THE CUSTOMER IS");
            ui.add_space(SP_XS);
            ui.add(
                TextEdit::multiline(&mut self.audience_text)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .hint_text("Who needs it, and what is going on in their world?"),
            );

            ui.add_space(SP_MD);

            // The button is disabled while a request is pending or while either
            // input is empty. The accent color is reserved for this one action.
            let busy = matches!(self.state, UiState::Loading);
            let ready = !self.product_text.trim().is_empty()
                && !self.audience_text.trim().is_empty();

            ui.horizontal(|ui| {
                let label = if busy { "Compressing" } else { "Compress" };
                let button = Button::new(
                    RichText::new(label).size(TS_BODY).strong().color(ON_ACCENT),
                )
                .fill(ACCENT)
                .rounding(Rounding::same(RADIUS_SM))
                .min_size(Vec2::new(150.0, 40.0));

                if ui.add_enabled(ready && !busy, button).clicked() {
                    self.submit(ctx);
                }

                if busy {
                    ui.add_space(SP_SM);
                    ui.add(Spinner::new().color(ACCENT));
                }
            });
        });
    }

    /// Render the output area based on the current state.
    ///
    /// We match on `&self.state` (a shared borrow), so this arm reads the state
    /// without mutating it. All four states are handled, which the compiler
    /// enforces because `UiState` is an `enum`.
    fn output(&self, ui: &mut egui::Ui) {
        match &self.state {
            UiState::Idle => {
                note(ui, "Paste a product and an audience, then compress.", MUTED);
            }
            UiState::Loading => {
                note(ui, "Compressing the abstraction stack...", MUTED);
            }
            UiState::Error(message) => {
                card(ui, |ui| {
                    field_label_colored(ui, "COULD NOT COMPRESS", DANGER);
                    ui.add_space(SP_XS);
                    ui.label(RichText::new(message).size(TS_BODY).color(TEXT));
                });
            }
            UiState::Result(compression) => {
                render_result(ui, compression);
            }
        }
    }
}

/// Draw the full compressed result: need, promise, then the three placements.
fn render_result(ui: &mut egui::Ui, c: &Compression) {
    // Core need.
    card(ui, |ui| {
        field_label(ui, "CORE NEED");
        ui.add_space(SP_XS);
        ui.label(RichText::new(c.core_need.as_str()).size(TS_HEADING).color(TEXT));
    });
    ui.add_space(SP_MD);

    // Promise: the one honest sentence, given the accent eyebrow to mark it as
    // the hinge between need and copy.
    card(ui, |ui| {
        field_label_colored(ui, "THE PROMISE", ACCENT);
        ui.add_space(SP_XS);
        ui.label(RichText::new(c.promise.as_str()).size(TS_HEADING).color(TEXT));
    });
    ui.add_space(SP_LG);

    ui.label(RichText::new("EXPRESSED FOR").size(TS_EYEBROW).strong().color(MUTED));
    ui.add_space(SP_SM);

    // The three placements, rendered uniformly from the typed pairs.
    for (placement, content) in c.placements() {
        // Precompute the copy payload before borrowing into the closure.
        let copy_text = match &content {
            PlacementContent::Single(s) => (*s).to_owned(),
            PlacementContent::Set(items) => items.join("\n"),
        };

        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(placement.title())
                            .size(TS_BODY)
                            .strong()
                            .color(TEXT),
                    );
                    ui.label(
                        RichText::new(placement.subtitle())
                            .size(TS_SMALL)
                            .color(MUTED),
                    );
                });
                // Copy button pinned to the right edge.
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let copy = Button::new(RichText::new("Copy").size(TS_SMALL).color(MUTED))
                        .fill(SURFACE_ALT)
                        .rounding(Rounding::same(RADIUS_SM));
                    if ui.add(copy).clicked() {
                        // Write to the system clipboard via egui's output.
                        ui.output_mut(|o| o.copied_text = copy_text.clone());
                    }
                });
            });

            ui.add_space(SP_SM);

            match content {
                PlacementContent::Single(s) => {
                    ui.label(RichText::new(s).size(TS_BODY).color(TEXT));
                }
                PlacementContent::Set(items) => {
                    for (i, headline) in items.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("{}", i + 1))
                                    .monospace()
                                    .color(ACCENT),
                            );
                            ui.add_space(SP_SM);
                            ui.label(RichText::new(headline).size(TS_BODY).color(TEXT));
                        });
                        ui.add_space(SP_XS);
                    }
                }
            }
        });
        ui.add_space(SP_MD);
    }
}

// --- Small UI helpers -----------------------------------------------------

/// A rounded, bordered surface with consistent padding. Takes a closure that
/// draws the card body (`impl FnOnce(&mut Ui)`), so callers compose freely.
fn card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    Frame::none()
        .fill(SURFACE)
        .rounding(Rounding::same(RADIUS))
        .stroke(Stroke::new(1.0, BORDER))
        .inner_margin(Margin::same(SP_MD + SP_XS))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui);
        });
}

/// An uppercase eyebrow label in the muted color.
fn field_label(ui: &mut egui::Ui, text: &str) {
    field_label_colored(ui, text, MUTED);
}

/// An uppercase eyebrow label in an explicit color.
fn field_label_colored(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.label(RichText::new(text).size(TS_EYEBROW).strong().color(color));
}

/// A plain, low emphasis note inside a card (used for idle and loading states).
fn note(ui: &mut egui::Ui, text: &str, color: Color32) {
    card(ui, |ui| {
        ui.label(RichText::new(text).size(TS_BODY).color(color));
    });
}
