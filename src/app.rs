//! app: the egui interface, the Pressroom theme, and the async to UI bridge.
//!
//! The look is "Pressroom": a letterpress instrument. A deep ink ground, warm
//! bone paper cards, a single vermilion ink accent, spaced caps, monospace
//! labels, registration marks, and custom painted motifs (a rule, an ink
//! coverage meter for groundedness, an inked platen for the Compress button).
//! The API key is entered in the window itself, masked, with an optional
//! remember on this device, and falls back to the environment variable.
//!
//! Rust primitives used here:
//!   - `enum` sum types for state and engine choice (`UiState`, `EngineKind`).
//!   - `mpsc` channel + `try_recv`: non blocking worker to UI handoff.
//!   - tokio runtime + boxed futures: run any selected engine uniformly.
//!   - `egui::Painter`: custom drawing for the motifs and the meter.
//!   - design tokens: named `const` values for the whole Pressroom palette.
//!   - std file IO: remember the key on this device, no extra dependencies.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};

use eframe::egui;
use egui::{
    pos2, vec2, Align, Align2, Button, Color32, CollapsingHeader, FontFamily, FontId, Frame, Id,
    Key, Layout, Margin, Rect, RichText, Rounding, ScrollArea, Sense, Spinner, Stroke, TextEdit,
    TextStyle, Vec2,
};

use crate::analysis::{self, Flag, GroundednessReport, Readability};
use crate::domain::{AudienceInput, Compression, PlacementContent, ProductInput};
use crate::engine::{self, EngineError, EngineKind, EngineOutput};
use crate::text;

// --- Pressroom design tokens ----------------------------------------------
//
// Ink ground, bone paper cards, one vermilion ink. Warm throughout (no cold
// slate). Text is dark ink on paper, paper on ink.

const INK: Color32 = Color32::from_rgb(0x17, 0x13, 0x0E); // app ground
const INK_2: Color32 = Color32::from_rgb(0x21, 0x1A, 0x12); // secondary ground (key bar)
const INK_LINE: Color32 = Color32::from_rgb(0x33, 0x2A, 0x1C); // hairline on ink

const PAPER: Color32 = Color32::from_rgb(0xEC, 0xE3, 0xD0); // card fill
const PAPER_WELL: Color32 = Color32::from_rgb(0xDD, 0xCE, 0xAF); // input wells, warm
const PAPER_EDGE: Color32 = Color32::from_rgb(0xC9, 0xBA, 0x9A); // card / input borders

const INK_TEXT: Color32 = Color32::from_rgb(0x24, 0x1C, 0x12); // text on paper
const INK_MUTED: Color32 = Color32::from_rgb(0x6F, 0x63, 0x4F); // muted text on paper
const PAPER_ON_INK: Color32 = Color32::from_rgb(0xEC, 0xE3, 0xD0); // text on ink ground
const PAPER_MUTED: Color32 = Color32::from_rgb(0xA0, 0x93, 0x7B); // muted text on ink

const VERMILION: Color32 = Color32::from_rgb(0xCC, 0x45, 0x27); // the one accent
const ON_VERMILION: Color32 = Color32::from_rgb(0xF6, 0xEC, 0xDC); // text on the accent

const GOOD: Color32 = Color32::from_rgb(0x3F, 0x7A, 0x4F); // printed green
const WARN: Color32 = Color32::from_rgb(0xB5, 0x80, 0x22); // ochre
const DANGER: Color32 = Color32::from_rgb(0xBE, 0x3A, 0x22); // misprint red

const SP_XS: f32 = 4.0;
const SP_SM: f32 = 8.0;
const SP_MD: f32 = 16.0;
const SP_LG: f32 = 24.0;
const SP_XL: f32 = 32.0;

const TS_WORDMARK: f32 = 34.0;
const TS_HEADING: f32 = 20.0;
const TS_BODY: f32 = 15.0;
const TS_SMALL: f32 = 13.0;
const TS_EYEBROW: f32 = 12.0;

const RADIUS: f32 = 4.0; // crisp, printed block corners
const RADIUS_SM: f32 = 3.0;

const ENGINES: [EngineKind; 3] = [EngineKind::Heuristic, EngineKind::Hybrid, EngineKind::Llm];

/// A built in sample so the tool can be demoed in one click.
const EXAMPLE_PRODUCT: &str = "Ledger is invoicing software for freelance designers. It tracks the hours you spend in your design tools and turns them into client-ready invoices in one click. Most freelancers lose money by forgetting to bill small tasks, and waste hours rebuilding their timesheets at the end of every month.";
const EXAMPLE_AUDIENCE: &str =
    "Freelance designers who juggle several clients at once and would rather be designing than doing admin.";

/// Install the Pressroom theme onto the egui context.
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
    v.dark_mode = false;
    // Default text is dark ink (most text sits on paper cards); on ink elements
    // we color explicitly, and an explicit RichText color always wins.
    v.override_text_color = Some(INK_TEXT);
    v.panel_fill = INK;
    v.window_fill = PAPER;
    v.extreme_bg_color = PAPER_WELL; // text edit wells
    v.faint_bg_color = PAPER;
    v.window_stroke = Stroke::new(1.0, PAPER_EDGE);
    v.selection.bg_fill = Color32::from_rgba_unmultiplied(0xCC, 0x45, 0x27, 0x44);
    v.selection.stroke = Stroke::new(1.0, VERMILION);

    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.rounding = Rounding::same(RADIUS_SM);
        w.bg_fill = PAPER_WELL;
        w.weak_bg_fill = PAPER_WELL;
        w.fg_stroke = Stroke::new(1.0, INK_TEXT);
        w.bg_stroke = Stroke::new(1.0, PAPER_EDGE);
    }
    v.window_rounding = Rounding::same(RADIUS);

    style.spacing.item_spacing = Vec2::new(SP_SM, SP_SM);
    style.spacing.button_padding = Vec2::new(SP_MD, SP_SM);

    ctx.set_style(style);
}

/// A graded result, ready to render.
struct ResultBundle {
    output: EngineOutput,
    promise_ground: GroundednessReport,
    readability: Readability,
    flags: Vec<Flag>,
    input_words: usize,
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
    api_key: String,
    remember_key: bool,
    show_key: bool,
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

        // If a key was remembered on this device, load it and tick the box.
        let saved = keystore::load();
        let remember_key = saved.is_some();
        let api_key = saved.unwrap_or_default();

        Self {
            runtime,
            engine: EngineKind::Heuristic,
            product_text: String::new(),
            audience_text: String::new(),
            api_key,
            remember_key,
            show_key: false,
            state: UiState::Idle,
            rx: None,
        }
    }

    /// The effective key from the window: the field if non blank, else `None`
    /// (the engine then falls back to the environment variable).
    fn effective_key(&self) -> Option<String> {
        let trimmed = self.api_key.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    }

    /// Is a key available from any source (window field or environment)?
    fn key_available(&self) -> bool {
        !self.api_key.trim().is_empty()
            || std::env::var("ANTHROPIC_API_KEY").map(|v| !v.trim().is_empty()).unwrap_or(false)
    }

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

        // Persist the key if the user asked us to remember it.
        if self.remember_key && !self.api_key.trim().is_empty() {
            keystore::save(self.api_key.trim());
        }

        let (tx, rx) = std::sync::mpsc::channel();
        self.rx = Some(rx);
        self.state = UiState::Loading;

        let kind = self.engine;
        let api_key = self.effective_key();
        let grounded_src = format!("{}\n{}", product.as_str(), audience.as_str());
        let ctx = ctx.clone();

        self.runtime.spawn(async move {
            let input_words = text::words(&grounded_src).len();
            let result = engine::run(kind, api_key, product, audience).await.map(|output| {
                let grounded = analysis::Grounded::build(&grounded_src);
                let promise_ground = grounded.report(output.compression.promise.as_str());
                let readability = analysis::readability(&output.compression.meta_primary_text);
                let flags = analysis::constraint_flags(&output.compression);
                ResultBundle {
                    output,
                    promise_ground,
                    readability,
                    flags,
                    input_words,
                }
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

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

        // Ctrl + Enter compresses when both fields are filled and nothing is in
        // flight, so the tool is usable without reaching for the mouse.
        let busy = matches!(self.state, UiState::Loading);
        let ready =
            !self.product_text.trim().is_empty() && !self.audience_text.trim().is_empty();
        if ready && !busy && ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::Enter)) {
            self.submit(ctx);
        }

        egui::CentralPanel::default()
            .frame(
                Frame::none()
                    .fill(INK)
                    .inner_margin(Margin::symmetric(SP_XL, SP_LG)),
            )
            .show(ctx, |ui| {
                ScrollArea::vertical().show(ui, |ui| {
                    // Center the content in a fixed max width column so lines do
                    // not run the full width of a large monitor.
                    let avail = ui.available_width();
                    let max_w = 1040.0_f32.min(avail);
                    let pad = ((avail - max_w) / 2.0).max(0.0);
                    ui.horizontal(|ui| {
                        ui.add_space(pad);
                        ui.vertical(|ui| {
                            ui.set_width(max_w);
                            self.key_bar(ui);
                            ui.add_space(SP_MD);
                            self.header(ui);
                            ui.add_space(SP_LG);
                            self.inputs(ui, ctx);
                            ui.add_space(SP_LG);
                            self.output(ui);
                        });
                    });
                });
            });
    }
}

impl CompressorApp {
    /// The API key entry, sitting on the ink ground above the wordmark.
    fn key_bar(&mut self, ui: &mut egui::Ui) {
        Frame::none()
            .fill(INK_2)
            .rounding(Rounding::same(RADIUS))
            .stroke(Stroke::new(1.0, INK_LINE))
            .inner_margin(Margin::symmetric(SP_MD, SP_SM))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("ANTHROPIC KEY")
                            .monospace()
                            .size(TS_EYEBROW)
                            .color(PAPER_MUTED),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let available = self.key_available();
                        let (dot, label, color) = if available {
                            ("\u{25CF}", "KEY LOADED", VERMILION)
                        } else {
                            ("\u{25CB}", "NO KEY", PAPER_MUTED)
                        };
                        ui.label(RichText::new(label).monospace().size(TS_EYEBROW).color(PAPER_MUTED));
                        ui.label(RichText::new(dot).size(TS_SMALL).color(color));
                    });
                });

                ui.add_space(SP_XS);

                ui.horizontal(|ui| {
                    // Show / hide toggle.
                    let eye = if self.show_key { "hide" } else { "show" };
                    if ui
                        .add(
                            Button::new(RichText::new(eye).size(TS_SMALL).color(INK_MUTED))
                                .fill(PAPER_WELL)
                                .rounding(Rounding::same(RADIUS_SM)),
                        )
                        .clicked()
                    {
                        self.show_key = !self.show_key;
                    }

                    // Remember toggle, persisted to disk.
                    let before = self.remember_key;
                    ui.checkbox(
                        &mut self.remember_key,
                        RichText::new("remember on this device").size(TS_SMALL).color(PAPER_MUTED),
                    );
                    if self.remember_key != before {
                        if self.remember_key && !self.api_key.trim().is_empty() {
                            keystore::save(self.api_key.trim());
                        } else if !self.remember_key {
                            keystore::clear();
                        }
                    }

                    // The masked field fills the remaining width.
                    ui.add(
                        TextEdit::singleline(&mut self.api_key)
                            .password(!self.show_key)
                            .hint_text("sk-ant-...")
                            .text_color(INK_TEXT)
                            .desired_width(f32::INFINITY),
                    );
                });

                if self.remember_key {
                    ui.add_space(SP_XS);
                    ui.label(
                        RichText::new("stored in plain text under your user profile")
                            .size(TS_EYEBROW)
                            .color(PAPER_MUTED),
                    );
                }
            });
    }

    /// The wordmark, the vermilion rule with registration marks, the strapline.
    fn header(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(space_caps("COMPRESSOR"))
                    .size(TS_WORDMARK)
                    .strong()
                    .color(PAPER_ON_INK),
            );
            ui.with_layout(Layout::right_to_left(Align::BOTTOM), |ui| {
                ui.label(
                    RichText::new("PRESSROOM EDITION")
                        .monospace()
                        .size(TS_EYEBROW)
                        .color(PAPER_MUTED),
                );
            });
        });

        // The rule: a vermilion bar capped with printer's registration marks.
        let width = ui.available_width();
        let (resp, painter) = ui.allocate_painter(vec2(width, 16.0), Sense::hover());
        let r = resp.rect;
        let y = r.center().y;
        painter.line_segment(
            [pos2(r.left() + 12.0, y), pos2(r.right() - 12.0, y)],
            Stroke::new(2.0, VERMILION),
        );
        draw_regmark(&painter, pos2(r.left() + 5.0, y), 4.0, PAPER_MUTED);
        draw_regmark(&painter, pos2(r.right() - 5.0, y), 4.0, PAPER_MUTED);

        ui.add_space(SP_XS);
        ui.label(
            RichText::new("the press: product to the customer who needs it")
                .italics()
                .size(TS_BODY)
                .color(PAPER_MUTED),
        );
    }

    fn inputs(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        paper_card(ui, |ui| {
            ui.horizontal(|ui| {
                eyebrow(ui, "SET THE ENGINE");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let load = Button::new(RichText::new("load example").size(TS_SMALL).color(INK_MUTED))
                        .fill(PAPER_WELL)
                        .rounding(Rounding::same(RADIUS_SM));
                    if ui.add(load).clicked() {
                        self.product_text = EXAMPLE_PRODUCT.to_owned();
                        self.audience_text = EXAMPLE_AUDIENCE.to_owned();
                    }
                });
            });
            ui.add_space(SP_XS);
            ui.horizontal(|ui| {
                for kind in ENGINES {
                    if engine_chip(ui, kind.label(), self.engine == kind).clicked() {
                        self.engine = kind;
                    }
                }
            });
            ui.add_space(SP_XS);
            ui.label(RichText::new(self.engine.tagline()).size(TS_SMALL).color(INK_MUTED));

            ui.add_space(SP_MD);

            eyebrow(ui, "PRODUCT");
            ui.add_space(SP_XS);
            ui.add(
                TextEdit::multiline(&mut self.product_text)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .text_color(INK_TEXT)
                    .hint_text("What is the product? Describe it plainly."),
            );

            ui.add_space(SP_MD);

            eyebrow(ui, "WHO YOU THINK THE CUSTOMER IS");
            ui.add_space(SP_XS);
            ui.add(
                TextEdit::multiline(&mut self.audience_text)
                    .desired_rows(4)
                    .desired_width(f32::INFINITY)
                    .text_color(INK_TEXT)
                    .hint_text("Who needs it, and what is going on in their world?"),
            );

            ui.add_space(SP_MD);

            let busy = matches!(self.state, UiState::Loading);
            let ready =
                !self.product_text.trim().is_empty() && !self.audience_text.trim().is_empty();

            ui.horizontal(|ui| {
                // The inked platen.
                let label = if busy { space_caps("PRESSING") } else { space_caps("COMPRESS") };
                let button = Button::new(RichText::new(label).size(TS_BODY).strong().color(ON_VERMILION))
                    .fill(VERMILION)
                    .rounding(Rounding::same(RADIUS))
                    .min_size(Vec2::new(200.0, 46.0));

                if ui.add_enabled(ready && !busy, button).clicked() {
                    self.submit(ctx);
                }

                if busy {
                    ui.add_space(SP_SM);
                    ui.add(Spinner::new().color(VERMILION));
                }

                if self.engine.needs_api_key() && !self.key_available() {
                    ui.add_space(SP_SM);
                    ui.label(
                        RichText::new("paste a key above to use this engine")
                            .size(TS_SMALL)
                            .color(DANGER),
                    );
                } else if !busy {
                    ui.add_space(SP_SM);
                    ui.label(RichText::new("or press Ctrl + Enter").size(TS_SMALL).color(INK_MUTED));
                }
            });
        });
    }

    fn output(&self, ui: &mut egui::Ui) {
        match &self.state {
            UiState::Idle => {
                note(ui, "Set the type. Paste a product and an audience, then press Compress.");
            }
            UiState::Loading => {
                note(ui, "Inking the plate...");
            }
            UiState::Error(message) => {
                paper_card(ui, |ui| {
                    eyebrow_colored(ui, "MISPRINT", DANGER);
                    ui.add_space(SP_XS);
                    ui.label(RichText::new(message).size(TS_BODY).color(INK_TEXT));
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

    // The thesis as a headline number: how far the input was compressed.
    let promise_words = text::words(c.promise.as_str()).len();
    compression_readout(ui, bundle.input_words, promise_words);
    ui.add_space(SP_MD);

    paper_card(ui, |ui| {
        eyebrow(ui, "CORE NEED");
        ui.add_space(SP_XS);
        ui.label(RichText::new(c.core_need.as_str()).size(TS_HEADING).color(INK_TEXT));
    });
    ui.add_space(SP_MD);

    // The promise, with an ink coverage meter for groundedness.
    paper_card(ui, |ui| {
        eyebrow_colored(ui, "THE PROMISE", VERMILION);
        ui.add_space(SP_XS);
        ui.label(RichText::new(c.promise.as_str()).size(TS_HEADING).color(INK_TEXT));

        ui.add_space(SP_SM);
        ink_meter(ui, bundle.promise_ground.score);

        if !bundle.promise_ground.unsupported.is_empty() {
            ui.add_space(SP_XS);
            ui.label(
                RichText::new(format!(
                    "Thin ink (unsupported by the product): {}",
                    bundle.promise_ground.unsupported.join(", ")
                ))
                .size(TS_SMALL)
                .color(WARN),
            );
        }
    });
    ui.add_space(SP_MD);

    paper_card(ui, |ui| {
        eyebrow(ui, "QUALITY CHECKS");
        ui.add_space(SP_SM);
        ui.horizontal_wrapped(|ui| {
            chip(
                ui,
                &format!(
                    "Readability {:.0} ({})",
                    bundle.readability.flesch, bundle.readability.grade
                ),
            );
            chip(ui, &format!("Promise {}% grounded", bundle.promise_ground.score));
        });
        ui.add_space(SP_SM);
        if bundle.flags.is_empty() {
            ui.label(
                RichText::new("Clean proof. Headlines fit 30 characters, no hype words.")
                    .size(TS_SMALL)
                    .color(GOOD),
            );
        } else {
            for flag in &bundle.flags {
                ui.label(RichText::new(format!("- {}", flag.message)).size(TS_SMALL).color(WARN));
            }
        }
    });
    ui.add_space(SP_LG);

    eyebrow(ui, "EXPRESSED FOR");
    ui.add_space(SP_SM);
    render_placements(ui, c);

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

        paper_card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(placement.title()).size(TS_BODY).strong().color(INK_TEXT));
                    ui.label(RichText::new(placement.subtitle()).size(TS_SMALL).color(INK_MUTED));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let copy = Button::new(RichText::new("Copy").size(TS_SMALL).color(INK_MUTED))
                        .fill(PAPER_WELL)
                        .rounding(Rounding::same(RADIUS_SM));
                    if ui.add(copy).clicked() {
                        ui.output_mut(|o| o.copied_text = copy_text.clone());
                    }
                });
            });

            ui.add_space(SP_SM);

            match content {
                PlacementContent::Single(s) => {
                    ui.label(RichText::new(s).size(TS_BODY).color(INK_TEXT));
                }
                PlacementContent::Set(items) => {
                    for (i, headline) in items.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("{}", i + 1)).monospace().color(VERMILION));
                            ui.add_space(SP_SM);
                            ui.label(RichText::new(headline).size(TS_BODY).color(INK_TEXT));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let len = headline.chars().count();
                                let color = if len <= 30 { INK_MUTED } else { DANGER };
                                ui.label(
                                    RichText::new(format!("{len}/30")).monospace().size(TS_SMALL).color(color),
                                );
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

/// The proof sheet: the deterministic engine showing its work.
fn render_trace(ui: &mut egui::Ui, trace: &engine::Trace) {
    paper_card(ui, |ui| {
        CollapsingHeader::new(RichText::new("Proof sheet").size(TS_BODY).strong().color(INK_TEXT))
            .default_open(true)
            .show(ui, |ui| {
                ui.add_space(SP_SM);

                if !trace.need_source.is_empty() {
                    kv(
                        ui,
                        "Core need pulled from",
                        &format!("\"{}\"  (TextRank {:.2})", trace.need_source, trace.need_score),
                    );
                }
                if !trace.template.is_empty() {
                    kv(ui, "Promise template", trace.template.as_str());
                }

                if !trace.candidates.is_empty() {
                    ui.add_space(SP_SM);
                    eyebrow(ui, "PROMISE IMPRESSIONS (RANKED)");
                    ui.add_space(SP_XS);
                    for cand in &trace.candidates {
                        let (marker_color, text_color) = if cand.chosen {
                            (VERMILION, INK_TEXT)
                        } else {
                            (PAPER_EDGE, INK_MUTED)
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("\u{25A0}").size(TS_SMALL).color(marker_color));
                            ui.add_space(SP_XS);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&cand.text).size(TS_SMALL).color(text_color));
                                ui.label(
                                    RichText::new(format!(
                                        "inked {}%   readability {:.0}   score {:.0}",
                                        cand.groundedness, cand.readability, cand.score
                                    ))
                                    .monospace()
                                    .size(TS_EYEBROW)
                                    .color(INK_MUTED),
                                );
                            });
                        });
                        ui.add_space(SP_XS);
                    }
                }

                if !trace.notes.is_empty() {
                    ui.add_space(SP_SM);
                    for n in &trace.notes {
                        ui.label(RichText::new(n).size(TS_SMALL).color(INK_MUTED));
                    }
                }
            });
    });
}

// --- Custom painting ------------------------------------------------------

/// A printer's registration mark: a thin circle with a centered cross.
fn draw_regmark(painter: &egui::Painter, center: egui::Pos2, r: f32, color: Color32) {
    let stroke = Stroke::new(1.0, color);
    painter.circle_stroke(center, r, stroke);
    painter.line_segment(
        [pos2(center.x - r - 2.0, center.y), pos2(center.x + r + 2.0, center.y)],
        stroke,
    );
    painter.line_segment(
        [pos2(center.x, center.y - r - 2.0), pos2(center.x, center.y + r + 2.0)],
        stroke,
    );
}

/// The ink coverage meter: a paper track filled to the groundedness fraction in
/// the band color, with the printed percent beside it. Groundedness as ink.
fn ink_meter(ui: &mut egui::Ui, score: u8) {
    let band = ground_color(score);
    // Animate the fill so the meter inks in when a result appears.
    let target = (score as f32 / 100.0).clamp(0.0, 1.0);
    let fraction = ui.ctx().animate_value_with_time(Id::new("ink_meter_fill"), target, 0.5);

    // Allocate the full row; reserve a fixed strip on the right for the printed
    // value, and right anchor the label so it never clips at the edge.
    let full_width = ui.available_width();
    let (resp, painter) = ui.allocate_painter(vec2(full_width, 18.0), Sense::hover());
    let full = resp.rect;
    let label_strip = 96.0;
    let meter_width = (full.width() - label_strip).max(60.0);
    let track = Rect::from_min_size(full.min, vec2(meter_width, 16.0));

    painter.rect_filled(track, Rounding::same(RADIUS_SM), PAPER_WELL);
    painter.rect_stroke(track, Rounding::same(RADIUS_SM), Stroke::new(1.0, PAPER_EDGE));
    let fill = Rect::from_min_size(track.min, vec2(track.width() * fraction, track.height()));
    painter.rect_filled(fill, Rounding::same(RADIUS_SM), band);

    painter.text(
        pos2(full.right(), track.center().y),
        Align2::RIGHT_CENTER,
        format!("{score}% inked"),
        FontId::monospace(TS_SMALL),
        band,
    );
}

/// A headline readout of how far the input compressed, styled as a press stat.
fn compression_readout(ui: &mut egui::Ui, words_in: usize, words_out: usize) {
    let reduction = if words_in > 0 {
        ((1.0 - words_out as f32 / words_in as f32) * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    paper_card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                eyebrow(ui, "COMPRESSION");
                ui.add_space(SP_XS);
                ui.label(
                    RichText::new(format!(
                        "{words_in} words in, down to a {words_out}-word promise"
                    ))
                    .size(TS_BODY)
                    .color(INK_TEXT),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(format!("{reduction:.0}%")).size(32.0).strong().color(VERMILION));
            });
        });
    });
}

// --- Small UI helpers -----------------------------------------------------

/// A bone paper card with crisp printed corners.
fn paper_card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    Frame::none()
        .fill(PAPER)
        .rounding(Rounding::same(RADIUS))
        .stroke(Stroke::new(1.0, PAPER_EDGE))
        .inner_margin(Margin::same(SP_MD + SP_XS))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui);
        });
}

/// A selectable engine chip: vermilion ink when chosen, paper otherwise.
fn engine_chip(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let (bg, fg) = if selected {
        (VERMILION, ON_VERMILION)
    } else {
        (PAPER_WELL, INK_MUTED)
    };
    ui.add(
        Button::new(RichText::new(label).size(TS_SMALL).strong().color(fg))
            .fill(bg)
            .rounding(Rounding::same(RADIUS_SM))
            .min_size(Vec2::new(98.0, 30.0)),
    )
}

/// A small static metric chip on a paper card.
fn chip(ui: &mut egui::Ui, label: &str) {
    Frame::none()
        .fill(PAPER_WELL)
        .rounding(Rounding::same(RADIUS_SM))
        .stroke(Stroke::new(1.0, PAPER_EDGE))
        .inner_margin(Margin::symmetric(SP_SM, SP_XS))
        .show(ui, |ui| {
            ui.label(RichText::new(label).size(TS_SMALL).strong().color(INK_MUTED));
        });
}

/// A label / value row used inside the proof sheet.
fn kv(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("{key}: ")).size(TS_SMALL).color(INK_MUTED));
        ui.label(RichText::new(value).size(TS_SMALL).color(INK_TEXT));
    });
    ui.add_space(SP_XS);
}

/// A monospace uppercase eyebrow label in the muted ink color.
fn eyebrow(ui: &mut egui::Ui, text: &str) {
    eyebrow_colored(ui, text, INK_MUTED);
}

fn eyebrow_colored(ui: &mut egui::Ui, text: &str, color: Color32) {
    ui.label(RichText::new(text).monospace().size(TS_EYEBROW).strong().color(color));
}

/// A low emphasis note card (idle and loading states).
fn note(ui: &mut egui::Ui, text: &str) {
    paper_card(ui, |ui| {
        ui.label(RichText::new(text).size(TS_BODY).color(INK_MUTED));
    });
}

/// Color for a groundedness score: well inked, thinning, or too thin.
fn ground_color(score: u8) -> Color32 {
    if score >= 85 {
        VERMILION
    } else if score >= 70 {
        WARN
    } else {
        DANGER
    }
}

/// Insert spaces between characters for a tracked, pressed caps wordmark.
fn space_caps(s: &str) -> String {
    s.chars().map(|c| c.to_string()).collect::<Vec<_>>().join(" ")
}

// --- Key persistence (std only, no extra dependencies) --------------------

/// Remember the key on this device, opt in, stored under the user profile.
mod keystore {
    use super::PathBuf;

    fn path() -> Option<PathBuf> {
        // %APPDATA%\Compressor\key.txt on Windows.
        std::env::var_os("APPDATA").map(|base| PathBuf::from(base).join("Compressor").join("key.txt"))
    }

    pub fn load() -> Option<String> {
        let p = path()?;
        std::fs::read_to_string(p)
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    }

    pub fn save(key: &str) {
        if let Some(p) = path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(p, key);
        }
    }

    pub fn clear() {
        if let Some(p) = path() {
            let _ = std::fs::remove_file(p);
        }
    }
}
