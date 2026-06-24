//! engine::hybrid: the model writes, the deterministic layer keeps it honest.
//!
//! Hybrid is the demonstration of the whole thesis at once. It takes the LLM's
//! copy (more fluent than the heuristic draft) but attaches the deterministic
//! explainability trace and runs the groundedness scorer over the model's
//! promise, surfacing any claim the model introduced that is not supported by
//! the product. Using AI, and keeping it honest, in one pass.
//!
//! Rust primitives used here:
//!   - `async fn` + `.await`: it awaits the LLM engine.
//!   - composition: it reuses `heuristic::compress_sync` and the `analysis`
//!     scorer rather than reimplementing them.
//!   - borrow before move: the grounded source is computed from the inputs
//!     before they are moved into the LLM call.

use crate::analysis::Grounded;
use crate::domain::{AudienceInput, ProductInput};

use super::{heuristic, CompressionEngine, EngineError, EngineOutput, LlmEngine, Trace};

/// The hybrid engine. A unit struct: it holds no state.
pub struct HybridEngine;

impl CompressionEngine for HybridEngine {
    async fn compress(
        &self,
        product: ProductInput,
        audience: AudienceInput,
    ) -> Result<EngineOutput, EngineError> {
        // Compute the deterministic trace and the grounded vocabulary first,
        // while we still have the inputs by reference.
        let heuristic_trace = heuristic::compress_sync(&product, &audience)
            .ok()
            .and_then(|out| out.trace);
        let combined = format!("{}\n{}", product.as_str(), audience.as_str());
        let grounded = Grounded::build(&combined);

        // Then let the model write the copy (this consumes the inputs).
        let llm_output = LlmEngine.compress(product, audience).await?;
        let compression = llm_output.compression;

        // Audit the model's promise against what the product literally is.
        let report = grounded.report(compression.promise.as_str());
        let mut notes = vec![format!(
            "Model promise is {}% grounded ({} of {} claim terms supported by the product).",
            report.score, report.supported, report.total
        )];
        if report.unsupported.is_empty() {
            notes.push("No unsupported claims detected in the model promise.".to_owned());
        } else {
            notes.push(format!(
                "Unsupported claims the model introduced: {}.",
                report.unsupported.join(", ")
            ));
        }

        // Attach the deterministic trace (with the audit notes) for transparency.
        let trace = match heuristic_trace {
            Some(mut t) => {
                t.engine = "hybrid";
                t.notes = notes;
                Some(t)
            }
            None => Some(Trace {
                engine: "hybrid",
                need_source: String::new(),
                need_score: 0.0,
                template: String::new(),
                candidates: Vec::new(),
                notes,
            }),
        };

        Ok(EngineOutput { compression, trace })
    }
}
