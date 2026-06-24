//! app: the egui interface, the custom theme, and the async to UI bridge.
//!
//! The UI is modeled as an explicit state machine ([`UiState`]). The user picks
//! an engine (deterministic, hybrid, or model backed), the chosen engine runs on
//! a tokio runtime, and its result returns over an `mpsc` channel polled once
//! per frame, so the interface never freezes. After a result arrives, the
//! groundedness scorer grades it (outside the engine) and the explainability
//! panel shows the deterministic engine's work.
//!
//! Rust primitives used here:
//!   - `enum` sum type for state and engine choice (`UiState`, `EngineKind`).
//!   - `mpsc` channel + `try_recv`: non blocking worker to UI handoff.
//!   - tokio runtime + boxed futures: run any selected engine uniformly.
//!   - `Clone` on `egui::Context`: wake the UI exactly when the result is ready.
//!   - trait impl (`impl eframe::App`): plug into the event loop.
//!   - design tokens: named `const` values for color, spacing, and type scale.

use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;
use egui::{
    Align, Button, Color32, CollapsingHeader, FontFamily, FontId, Frame, Layout, Margin, RichText,
    Rounding, ScrollArea, Spinner, Stroke, TextEdit, TextStyle, Vec2,
};

use crate::analysis::{self, Flag, GroundednessReport, Readability};
use crate::domain::{AudienceInput, Compression, PlacementContent, ProductInput};
use crate::engine::{self, EngineError, EngineKind, EngineOutput};

// --- Design tokens --------------------------------------------------------

const BG: Color32 = Color32::from_rgb(0x0F, 0x11, 0x16);
const SURFACE: Color32 = Color32::from_rgb(0x16, 0x19, 0x20);
const SURFACE_ALT: Color32 = Color32::from_rgb(0x1E, 0x22, 0x2B);
const BORDER: Color32 = Color32::from_rgb(0x2A, 0x2F, 0x3A);
const TEXT: Color32 = Color32::from_rgb(0xE6, 0xE8, 0xEC);
const MUTED: Color32 = Color32::from_rgb(0x97, 0x9F, 0xAD);
const ACCENT: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x3D);
const ON_ACCENT: Color32 = Color32::from_rgb(0x16, 0x0A, 0x05);
const GOOD: Color32 = Color32::from_rgb(0x4A, 0xD6, 0x91);
const WARN: Color32 = Color32::from_rgb(0xF0, 0xB4, 0x29);
const DANGER: Color32 = Color32::from_rgb(0xFF, 0x6B, 0x6B);

const SP_XS: f32 = 4.0;
const SP_SM: f32 = 8.0;
const SP_MD: f32 = 16.0;
const SP_LG: f32 = 24.0;
const SP_XL: f32 = 32.0;

const TS_DISPLAY: f32 = 30.0;
const TS_HEADING: f32 = 20.0;
const TS_BODY: f32 = 15.0;
const TS_SMALL: f32 = 13.0;
const TS_EYEBROW: f32 = 12.0;

const RADIUS: f32 = 12.0;
const RADIUS_SM: f32 = 8.0;

/// Every engine the picker offers, in display order.
const ENGINES: [EngineKind; 3] = [EngineKind::Heuristic, EngineKind::Hybrid, EngineKind::Llm];

/// Install the custom dark theme onto the egui context.
fn install_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    style.text_styles = [
        (TextStyle::Heading, FontId::new(TS_HEADING, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(TS_BODY, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(TS_BODY, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(TS_SMALL, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(TS_SMALL, FontFamily::Monospace)),
    ]
    .into();

    let v = &mut style.visuals;
    v.dark_mode = true;
    v.override_text_color = Some(TEXT);
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.extreme_bg_color = SURFACE_ALT;
    v.faint_bg_color = SURFACE;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(0xFF, 0x6B, 0x3D, 0x40);
    v.selection.stroke = Stroke::new(1.0, ACCENT);

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

    style.spacing.item_spacing = Vec2::new(SP_SM, SP_SM);
    style.spacing.button_padding = Vec2::new(SP_MD, SP_SM);

    ctx.set_style(style);
}

/// A graded result, ready to render. The groundedness, readability, and
/// constraint checks are computed outside the engine, on whatever it produced.
struct ResultBundle {
    output: EngineOutput,
    promise_ground: GroundednessReport,
    readability: Readability,
    flags: Vec<Flag>,
}

/// The explicit UI state machine.
enum UiState {
    Idle,
    Loading,
    Result(Box<ResultBundle>),
    Error(String),
}

/// The application.
pub struct CompressorApp {
    runtime: tokio::runtime::Runtime,
    engine: EngineKind,
    product_text: String,
    audience_text: String,
    state: UiState,
    rx: Option<Receiver<Result<ResultBundle, EngineError>>>,
}

impl CompressorApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        install_theme(&cc.egui_ctx);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to start the tokio runtime");

        Self {
            runtime,
            // Default to the deterministic engine: instant, offline, no key.
            engine: EngineKind::Heuristic,
            product_text: String::new(),
            audience_text: String::new(),
            state: UiState::Idle,
            rx: None,
        }
    }

    /// Validate the inputs and, if good, spawn the selected engine.
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

        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        self.state = UiState::Loading;

        let kind = self.engine;
        // The grounded source the scorer grades against. Captured before the
        // inputs move into the engine.
        let grounded_src = format!("{}\n{}", product.as_str(), audience.as_str());
        let ctx = ctx.clone();

        self.runtime.spawn(async move {
            let result = engine::run(kind, product, audience).await.map(|output| {
                // The scorer sits outside the engine and grades its output.
                let grounded = analysis::Grounded::build(&grounded_src);
                let promise_ground = grounded.report(output.compression.promise.as_str());
                let readability = analysis::readability(&output.compression.meta_primary_text);
                let flags = analysis::constraint_flags(&output.compression);
                ResultBundle {
                    output,
                    promise_ground,
                    readability,
                    flags,
                }
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    /// Poll the channel once per frame, never blocking.
    fn poll_result(&mut self) {
        let Some(rx) = &self.rx else {
            return;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.state = match result {
                    Ok(bundle) => UiState::Result(Box::new(bundle)),
                    Err(error) => UiState::Error(error.to_string()),
                };
                self.rx = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.state = UiState::Error("the worker task ended unexpectedly".to_owned());
                self.rx = None;
            }
        }
    }
}

impl eframe::App for CompressorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_result();

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
    fn header(&self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Compressor").size(TS_DISPLAY).strong().color(TEXT));
        ui.add_space(SP_XS);
        ui.label(
            RichText::new("Remove the layers between a product and the customer who needs it.")
                .size(TS_BODY)
                .color(MUTED),
        );
        ui.add_space(SP_SM);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(64.0, 3.0), egui::Sense::hover());
        ui.painter().rect_filled(rect, Rounding::same(2.0), ACCENT);
    }

    fn inputs(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        card(ui, |ui| {
            // Engine picker: a segmented control of chips.
            field_label(ui, "ENGINE");
            ui.add_space(SP_XS);
            ui.horizontal(|ui| {
                for kind in ENGINES {
                    if engine_chip(ui, kind.label(), self.engine == kind).clicked() {
                        self.engine = kind;
                    }
                }
            });
            ui.add_space(SP_XS);
            ui.label(RichText::new(self.engine.tagline()).size(TS_SMALL).color(MUTED));

            ui.add_space(SP_MD);

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

            let busy = matches!(self.state, UiState::Loading);
            let ready =
                !self.product_text.trim().is_empty() && !self.audience_text.trim().is_empty();

            ui.horizontal(|ui| {
                let label = if busy { "Compressing" } else { "Compress" };
                let button =
                    Button::new(RichText::new(label).size(TS_BODY).strong().color(ON_ACCENT))
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

                if self.engine.needs_api_key() {
                    ui.add_space(SP_SM);
                    ui.label(
                        RichText::new("needs ANTHROPIC_API_KEY")
                            .size(TS_SMALL)
                            .color(MUTED),
                    );
                }
            });
        });
    }

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
            UiState::Result(bundle) => {
                render_result(ui, bundle);
            }
        }
    }
}

/// Render the full graded result.
fn render_result(ui: &mut egui::Ui, bundle: &ResultBundle) {
    let c = &bundle.output.compression;

    // Core need.
    card(ui, |ui| {
        field_label(ui, "CORE NEED");
        ui.add_space(SP_XS);
        ui.label(RichText::new(c.core_need.as_str()).size(TS_HEADING).color(TEXT));
    });
    ui.add_space(SP_MD);

    // Promise, with its groundedness graded right on the card.
    card(ui, |ui| {
        ui.horizontal(|ui| {
            field_label_colored(ui, "THE PROMISE", ACCENT);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let score = bundle.promise_ground.score;
                chip(
                    ui,
                    &format!("{score}% grounded"),
                    ON_ACCENT,
                    ground_color(score),
                );
            });
        });
        ui.add_space(SP_XS);
        ui.label(RichText::new(c.promise.as_str()).size(TS_HEADING).color(TEXT));
        if !bundle.promise_ground.unsupported.is_empty() {
            ui.add_space(SP_XS);
            ui.label(
                RichText::new(format!(
                    "Unsupported by the product: {}",
                    bundle.promise_ground.unsupported.join(", ")
                ))
                .size(TS_SMALL)
                .color(WARN),
            );
        }
    });
    ui.add_space(SP_MD);

    // Quality checks: readability and constraints.
    card(ui, |ui| {
        field_label(ui, "QUALITY CHECKS");
        ui.add_space(SP_SM);
        ui.horizontal_wrapped(|ui| {
            chip(
                ui,
                &format!(
                    "Readability {:.0} ({})",
                    bundle.readability.flesch, bundle.readability.grade
                ),
                MUTED,
                SURFACE_ALT,
            );
            chip(
                ui,
                &format!("Promise {}% grounded", bundle.promise_ground.score),
                MUTED,
                SURFACE_ALT,
            );
        });
        ui.add_space(SP_SM);
        if bundle.flags.is_empty() {
            ui.label(
                RichText::new("No constraint issues. Headlines fit 30 characters, no hype words.")
                    .size(TS_SMALL)
                    .color(GOOD),
            );
        } else {
            for flag in &bundle.flags {
                ui.label(
                    RichText::new(format!("- {}", flag.message))
                        .size(TS_SMALL)
                        .color(WARN),
                );
            }
        }
    });
    ui.add_space(SP_LG);

    // The three placements.
    ui.label(RichText::new("EXPRESSED FOR").size(TS_EYEBROW).strong().color(MUTED));
    ui.add_space(SP_SM);
    render_placements(ui, c);

    // The explainability panel, when the engine left a trace.
    if let Some(trace) = &bundle.output.trace {
        render_trace(ui, trace);
    }
}

fn render_placements(ui: &mut egui::Ui, c: &Compression) {
    for (placement, content) in c.placements() {
        let copy_text = match &content {
            PlacementContent::Single(s) => (*s).to_owned(),
            PlacementContent::Set(items) => items.join("\n"),
        };

        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(placement.title()).size(TS_BODY).strong().color(TEXT));
                    ui.label(RichText::new(placement.subtitle()).size(TS_SMALL).color(MUTED));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let copy = Button::new(RichText::new("Copy").size(TS_SMALL).color(MUTED))
                        .fill(SURFACE_ALT)
                        .rounding(Rounding::same(RADIUS_SM));
                    if ui.add(copy).clicked() {
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
                            ui.label(RichText::new(format!("{}", i + 1)).monospace().color(ACCENT));
                            ui.add_space(SP_SM);
                            ui.label(RichText::new(headline).size(TS_BODY).color(TEXT));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let len = headline.chars().count();
                                let color = if len <= 30 { MUTED } else { DANGER };
                                ui.label(RichText::new(format!("{len}/30")).size(TS_SMALL).color(color));
                            });
                        });
                        ui.add_space(SP_XS);
                    }
                }
            }
        });
        ui.add_space(SP_MD);
    }
}

/// The explainability panel: the deterministic engine showing its work. No
/// model backed tool can offer this, which is exactly why it is here.
fn render_trace(ui: &mut egui::Ui, trace: &engine::Trace) {
    card(ui, |ui| {
        CollapsingHeader::new(RichText::new("Explainability").size(TS_BODY).strong().color(TEXT))
            .default_open(true)
            .show(ui, |ui| {
                ui.add_space(SP_SM);

                if !trace.need_source.is_empty() {
                    kv(
                        ui,
                        "Core need extracted from",
                        &format!("\"{}\"  (TextRank {:.2})", trace.need_source, trace.need_score),
                    );
                }
                if !trace.template.is_empty() {
                    kv(ui, "Promise template", trace.template.as_str());
                }

                if !trace.candidates.is_empty() {
                    ui.add_space(SP_SM);
                    field_label(ui, "PROMISE CANDIDATES (RANKED)");
                    ui.add_space(SP_XS);
                    for cand in &trace.candidates {
                        let (marker_color, text_color) = if cand.chosen {
                            (ACCENT, TEXT)
                        } else {
                            (BORDER, MUTED)
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("\u{25CF}").size(TS_SMALL).color(marker_color));
                            ui.add_space(SP_XS);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&cand.text).size(TS_SMALL).color(text_color));
                                ui.label(
                                    RichText::new(format!(
                                        "grounded {}%   readability {:.0}   score {:.0}",
                                        cand.groundedness, cand.readability, cand.score
                                    ))
                                    .size(TS_EYEBROW)
                                    .color(MUTED),
                                );
                            });
                        });
                        ui.add_space(SP_XS);
                    }
                }

                if !trace.notes.is_empty() {
                    ui.add_space(SP_SM);
                    for n in &trace.notes {
                        ui.label(RichText::new(n).size(TS_SMALL).color(MUTED));
                    }
                }
            });
    });
}

// --- Small UI helpers -----------------------------------------------------

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

/// A selectable engine chip. Accent fill when selected.
fn engine_chip(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let (bg, fg) = if selected {
        (ACCENT, ON_ACCENT)
    } else {
        (SURFACE_ALT, MUTED)
    };
    ui.add(
        Button::new(RichText::new(label).size(TS_SMALL).strong().color(fg))
            .fill(bg)
            .rounding(Rounding::same(RADIUS_SM))
            .min_size(Vec2::new(96.0, 30.0)),
    )
}

/// A small, static metric chip.
fn chip(ui: &mut egui::Ui, label: &str, fg: Color32, bg: Color32) {
    Frame::none()
        .fill(bg)
        .rounding(Rounding::same(RADIUS_SM))
        .inner_margin(Margin::symmetric(SP_SM, SP_XS))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(TS_SMALL).strong().color(fg));
        });
}

/// A label / value row used inside the explainability panel.
fn kv(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("{key}: ")).size(TS_SMALL).color(MUTED));
        ui.label(RichText::new(value).size(TS_SMALL).color(TEXT));
    });
    ui.add_space(SP_XS);
}

fn field_label(ui: &mut egui::Ui, text: &str) {
    field_label_colored(ui, text, MUTED);
}

fn field_label_colored(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.label(RichText::new(text).size(TS_EYEBROW).strong().color(color));
}

fn note(ui: &mut egui::Ui, text: &str, color: Color32) {
    card(ui, |ui| {
        ui.label(RichText::new(text).size(TS_BODY).color(color));
    });
}

/// Color for a groundedness score: green when high, amber mid, red when low.
fn ground_color(score: u8) -> Color32 {
    if score >= 85 {
        GOOD
    } else if score >= 70 {
        WARN
    } else {
        DANGER
    }
}
