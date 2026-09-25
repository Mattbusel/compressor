//! Compressor: a desktop marketing tool that collapses the abstraction stack
//! between a product and the customer who needs it.
//!
//! The crate is split into modules, each with a single responsibility:
//!   - `domain`: the typed vocabulary (newtypes, enums, the result struct).
//!   - `text`: dependency free text primitives (tokenize, stem, syllables).
//!   - `analysis`: the deterministic measurement layer (TextRank, groundedness, readability, constraints).
//!   - `prompt`: pure construction of the system and user prompts.
//!   - `engine`: the `CompressionEngine` trait and its three implementations (heuristic, hybrid, LLM).
//!   - `app`: the egui UI, the custom theme, and the async to UI bridge.
//!
//! Rust primitives used here:
//!   - `#![cfg_attr(...)]`: a conditional crate attribute. In release builds it
//!     sets the Windows subsystem to "windows", which hides the console window
//!     behind the GUI. In debug builds the console stays so logs are visible.
//!   - `mod`: declares the four modules that make up the program.
//!   - `fn main() -> eframe::Result`: returning a `Result` lets startup errors
//!     propagate cleanly instead of panicking.

// Hide the console window on Windows in release builds only.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod analysis;
mod app;
mod domain;
mod engine;
mod prompt;
mod text;

use eframe::egui;

const HELP: &str = "Compressor: collapse the abstraction stack between a product and its customer.

Usage: compressor [--help | --version]

With no arguments, opens the desktop window. The Heuristic engine runs with no
API key; the Hybrid and LLM engines need an Anthropic API key, pasted into the
window or set in the ANTHROPIC_API_KEY environment variable.";

fn main() -> eframe::Result {
    // Tiny flag handling so the binary can be checked from a terminal without
    // opening a window.
    if let Some(arg) = std::env::args().nth(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{HELP}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("compressor {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ => {}
        }
    }

    // Window configuration: a comfortable default size with a sensible minimum.
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 780.0])
            .with_min_inner_size([720.0, 600.0])
            .with_title("Compressor"),
        ..Default::default()
    };

    // Hand control to eframe. The closure builds our app once the GUI context
    // exists; `Ok(...)` wraps it in the `Result` the creator must return.
    eframe::run_native(
        "Compressor",
        native_options,
        Box::new(|cc| Ok(Box::new(app::CompressorApp::new(cc)))),
    )
}
