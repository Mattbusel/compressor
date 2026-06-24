//! engine: the compression engines and the interface they share.
//!
//! The central architectural move is here: a `CompressionEngine` trait,
//! implemented three times. The deterministic [`HeuristicEngine`] uses classical
//! NLP and never calls a model. The [`LlmEngine`] calls the Anthropic Messages
//! API. The [`HybridEngine`] takes the model's copy but attaches the
//! deterministic explainability trace and a groundedness audit, so the AI is
//! used and kept honest. The groundedness scorer (in `crate::analysis`) lives
//! outside all three and grades whatever any of them produces.
//!
//! Rust primitives used here:
//!   - trait with `async fn` (`CompressionEngine`): the shared interface.
//!   - `enum` sum type (`EngineKind`, `EngineError`): the closed set of engines
//!     and the exhaustive set of failure modes.
//!   - trait objects via boxed futures (`Pin<Box<dyn Future + Send>>`): lets the
//!     UI pick an engine at runtime and `spawn` the chosen future uniformly.
//!   - `thiserror::Error`: one message per `EngineError` variant.

mod heuristic;
mod hybrid;
mod llm;

pub use heuristic::HeuristicEngine;
pub use hybrid::HybridEngine;
pub use llm::LlmEngine;

use std::future::Future;
use std::pin::Pin;

use thiserror::Error;

use crate::domain::{AudienceInput, Compression, ProductInput};

/// The environment variable holding the secret, named once for error messages.
const API_KEY_ENV: &str = "ANTHROPIC_API_KEY";

/// Which engine the user selected.
///
/// `enum` sum type: a closed set, so the dispatch `match` in [`run`] is
/// exhaustively checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    Heuristic,
    Hybrid,
    Llm,
}

impl EngineKind {
    /// Short label for the engine picker chip.
    pub fn label(self) -> &'static str {
        match self {
            EngineKind::Heuristic => "Heuristic",
            EngineKind::Hybrid => "Hybrid",
            EngineKind::Llm => "LLM",
        }
    }

    /// One line describing the tradeoff, shown under the picker.
    pub fn tagline(self) -> &'static str {
        match self {
            EngineKind::Heuristic => {
                "Deterministic. Extractive, fully grounded, explainable. Runs offline, no API key."
            }
            EngineKind::Hybrid => {
                "Model writes the copy; the deterministic layer audits it and shows its work."
            }
            EngineKind::Llm => "The model writes the copy. The scorer still grades it for honesty.",
        }
    }

    /// Whether this engine needs the API key set.
    pub fn needs_api_key(self) -> bool {
        matches!(self, EngineKind::Hybrid | EngineKind::Llm)
    }
}

/// What an engine returns: the compression plus an optional explainability
/// trace. Only the deterministic and hybrid engines produce a trace.
#[derive(Debug, Clone)]
pub struct EngineOutput {
    pub compression: Compression,
    pub trace: Option<Trace>,
}

/// The deterministic engine's work, surfaced for the explainability panel.
///
/// No model can offer this transparency, which is the point of keeping it.
#[derive(Debug, Clone)]
pub struct Trace {
    /// Which engine produced this trace.
    pub engine: &'static str,
    /// The source sentence the core need was extracted from.
    pub need_source: String,
    /// The normalized TextRank centrality of that sentence (0 to 1).
    pub need_score: f32,
    /// The name of the promise template that won.
    pub template: String,
    /// Every promise candidate considered, with its scores.
    pub candidates: Vec<Candidate>,
    /// Free form notes (the hybrid audit lands here).
    pub notes: Vec<String>,
}

/// One promise candidate and why it scored the way it did.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub text: String,
    pub groundedness: u8,
    pub readability: f32,
    pub score: f32,
    pub chosen: bool,
}

/// Every way producing a compression can fail.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error("the {API_KEY_ENV} environment variable is not set")]
    MissingApiKey,

    #[error("network error talking to the API: {0}")]
    Http(#[from] reqwest::Error),

    #[error("the API returned status {status}: {body}")]
    BadStatus { status: u16, body: String },

    #[error("the API response contained no text content")]
    NoContent,

    #[error("could not parse the model output as the expected JSON: {0}")]
    MalformedJson(String),

    #[error("not enough input to compress: {0}")]
    InsufficientInput(String),
}

/// The shared interface every engine implements.
///
/// `async fn` in a trait: stable since Rust 1.75. We use it through concrete
/// types (never `dyn`), so the lint about unbounded `Send` on the returned
/// future does not apply; [`run`] boxes each concrete future explicitly.
#[allow(async_fn_in_trait)]
pub trait CompressionEngine {
    async fn compress(
        &self,
        product: ProductInput,
        audience: AudienceInput,
    ) -> Result<EngineOutput, EngineError>;
}

/// A boxed, sendable future producing an [`EngineOutput`].
pub type BoxedCompression =
    Pin<Box<dyn Future<Output = Result<EngineOutput, EngineError>> + Send>>;

/// Dispatch to the selected engine, returning a uniform boxed future.
///
/// Each concrete engine's future is known to be `Send`, so boxing it into a
/// single type lets the app `spawn` the result without caring which engine ran.
pub fn run(kind: EngineKind, product: ProductInput, audience: AudienceInput) -> BoxedCompression {
    match kind {
        EngineKind::Heuristic => Box::pin(HeuristicEngine.compress(product, audience)),
        EngineKind::Hybrid => Box::pin(HybridEngine.compress(product, audience)),
        EngineKind::Llm => Box::pin(LlmEngine.compress(product, audience)),
    }
}
