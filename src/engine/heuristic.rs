//! engine::heuristic: the deterministic compression engine.
//!
//! No model is involved. The core need is extracted (TextRank, not generated).
//! The promise is built by slotting input derived terms into a small grammar of
//! templates, scoring every candidate, and keeping the winner (disciplined A/B,
//! not variation spam). The placements are pure string transforms with the
//! Google 30 character budget enforced by construction. Because every slot is
//! lifted from the input, the engine cannot hallucinate a capability, and it can
//! show its work.
//!
//! Rust primitives used here:
//!   - trait impl over a sync core (`compress` just awaits no IO).
//!   - `HashMap` frequency tables for slot extraction.
//!   - iterator pipelines and `fold` for ranking candidates.
//!   - pure functions for every formatter, so each is independently testable.

use std::collections::HashMap;

use crate::analysis::{self, Grounded};
use crate::domain::{AudienceInput, Compression, CoreNeed, ProductInput, Promise};
use crate::text;

use super::{Candidate, CompressionEngine, EngineError, EngineOutput, Trace};

/// Call to action options, in ranked preference order.
const CTAS: &[&str] = &["Start free.", "See how it works.", "Get started.", "Learn more."];

/// Benefit verbs we recognize as outcomes.
const BENEFIT_VERBS: &[&str] = &[
    "save", "grow", "scale", "automate", "simplify", "speed", "reduce", "increase", "launch",
    "cut", "streamline", "boost", "convert", "close", "ship", "build", "manage", "track",
    "organize", "find", "sell", "plan", "forecast", "sync",
];

/// Objects a benefit verb commonly acts on.
const BENEFIT_OBJECTS: &[&str] = &[
    "time", "hours", "money", "revenue", "sales", "leads", "costs", "work", "effort", "inventory",
    "orders", "customers", "tasks", "data", "reports", "campaigns", "stock", "invoices",
];

/// Short, neutral headlines used to pad the Google set if needed.
const HEADLINE_BANK: &[&str] = &["Get started today", "Less busywork", "See it in action"];

/// The deterministic engine. A unit struct: it holds no state.
pub struct HeuristicEngine;

impl CompressionEngine for HeuristicEngine {
    async fn compress(
        &self,
        product: ProductInput,
        audience: AudienceInput,
    ) -> Result<EngineOutput, EngineError> {
        // All the work is synchronous; the async signature just satisfies the
        // shared trait so the UI can treat every engine the same way.
        compress_sync(&product, &audience)
    }
}

/// The synchronous core, reused by the hybrid engine for its explainability
/// trace. `pub(crate)` so siblings in the engine module can call it.
pub(crate) fn compress_sync(
    product: &ProductInput,
    audience: &AudienceInput,
) -> Result<EngineOutput, EngineError> {
    let combined = format!("{}\n{}", product.as_str(), audience.as_str());

    // 1. Extract the core need with TextRank plus a need shape preference.
    let sentences = text::sentences(&combined);
    if sentences.is_empty() {
        return Err(EngineError::InsufficientInput("no sentences found".into()));
    }
    let scores = analysis::rank_sentences(&sentences);
    let need_idx = analysis::select_need(&sentences, &scores)
        .ok_or_else(|| EngineError::InsufficientInput("could not extract a need".into()))?;
    let need_source = sentences[need_idx].clone();
    let max_score = scores.iter().cloned().fold(0.0f32, f32::max).max(1e-6);
    let need_score = scores.get(need_idx).copied().unwrap_or(0.0) / max_score;
    let core_need_text = normalize_sentence(&need_source);

    // 2. Extract promise slots from the inputs (never invented).
    let audience_phrase = extract_audience(audience.as_str());
    let (outcome, mechanism) = extract_product_slots(product.as_str());

    // 3. Build candidate promises and score each against the grounded vocab.
    let grounded = Grounded::build(&combined);
    let raw = build_candidates(&audience_phrase, &outcome, &mechanism);
    let mut candidates: Vec<Candidate> = raw
        .iter()
        .map(|(_, text)| {
            let g = grounded.report(text);
            let r = analysis::readability(text);
            let hype = analysis::hype_count(text);
            let score = g.score as f32 + readability_bonus(r.flesch)
                - (hype as f32) * 15.0
                - length_penalty(text);
            Candidate {
                text: text.clone(),
                groundedness: g.score,
                readability: r.flesch,
                score,
                chosen: false,
            }
        })
        .collect();

    // 4. Keep the winner (the disciplined version of variation).
    let best = argmax(&candidates);
    candidates[best].chosen = true;
    let promise_text = candidates[best].text.clone();
    let template = raw[best].0.to_owned();

    // 5. Reshape the promise and need into the three placements.
    let meta = format_meta(&promise_text);
    let headlines = format_headlines(&outcome, &mechanism, &audience_phrase);
    let hero = format_hero(&promise_text);

    let compression = Compression {
        core_need: CoreNeed::new(core_need_text),
        promise: Promise::new(promise_text),
        meta_primary_text: meta,
        google_headlines: headlines,
        landing_hero: hero,
    };

    let trace = Trace {
        engine: "heuristic",
        need_source,
        need_score,
        template,
        candidates,
        notes: Vec::new(),
    };

    Ok(EngineOutput {
        compression,
        trace: Some(trace),
    })
}

// --- Slot extraction ------------------------------------------------------

/// Pull a short audience phrase: the first run of content words, skipping
/// leading stopwords. Lifted verbatim from the audience input, so it is always
/// grounded.
fn extract_audience(audience: &str) -> String {
    let mut phrase: Vec<String> = Vec::new();
    for token in audience.split_whitespace() {
        let clean: String = token
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-')
            .collect();
        if clean.is_empty() {
            continue;
        }
        let lower = clean.to_lowercase();
        if phrase.is_empty() && text::is_stopword(&lower) {
            continue; // skip leading stopwords
        }
        if !phrase.is_empty() && text::is_stopword(&lower) {
            break; // stop at the first stopword after the phrase begins
        }
        phrase.push(lower);
        if phrase.len() >= 4 {
            break;
        }
    }
    if phrase.is_empty() {
        "teams".to_owned()
    } else {
        phrase.join(" ")
    }
}

/// Extract `(outcome, mechanism)` from the product text.
///
/// outcome: a benefit verb present in the product, paired with a benefit object
/// if one is also present (for example "save hours"). mechanism: the most
/// frequent content word that is not itself a benefit verb or object, displayed
/// in its commonest surface form (for example "inventory").
fn extract_product_slots(product: &str) -> (String, String) {
    // Frequency of each stemmed content word, keeping a display surface form.
    let mut freq: HashMap<String, (usize, String)> = HashMap::new();
    for word in text::content_words(product) {
        let stemmed = text::stem(&word);
        let entry = freq.entry(stemmed).or_insert((0, word.clone()));
        entry.0 += 1;
    }

    let benefit_stems: Vec<String> = BENEFIT_VERBS.iter().map(|v| text::stem(v)).collect();
    let object_stems: Vec<String> = BENEFIT_OBJECTS.iter().map(|o| text::stem(o)).collect();

    // Outcome verb: first benefit verb (in product order) whose stem appears.
    let product_words = text::content_words(product);
    let verb = BENEFIT_VERBS
        .iter()
        .find(|v| {
            let vs = text::stem(v);
            product_words.iter().any(|w| text::stem(w) == vs)
        })
        .copied();

    // Outcome object: first benefit object whose stem appears.
    let object = BENEFIT_OBJECTS
        .iter()
        .find(|o| {
            let os = text::stem(o);
            product_words.iter().any(|w| text::stem(w) == os)
        })
        .copied();

    let outcome = match (verb, object) {
        (Some(v), Some(o)) => format!("{v} {o}"),
        (Some(v), None) => v.to_owned(),
        (None, Some(o)) => format!("get more from your {o}"),
        (None, None) => "get more done".to_owned(),
    };

    // Mechanism: the most frequent content word that is not a benefit verb or
    // object, displayed in its surface form. Falls back to the first content
    // word, then to a neutral placeholder.
    let mechanism = freq
        .iter()
        .filter(|(stem, _)| !benefit_stems.contains(stem) && !object_stems.contains(stem))
        .max_by_key(|(_, (count, _))| *count)
        .map(|(_, (_, surface))| surface.clone())
        .or_else(|| product_words.first().cloned())
        .unwrap_or_else(|| "your workflow".to_owned());

    (outcome, mechanism)
}

// --- Promise templates ----------------------------------------------------

/// Fill the small grammar of promise templates with the extracted slots.
///
/// Returns `(template name, filled text)` pairs. Every word in every template
/// other than the connective glue comes from the input.
fn build_candidates(audience: &str, outcome: &str, mechanism: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "audience-first",
            format!("{} {} with {}.", capitalize(audience), outcome, mechanism),
        ),
        (
            "mechanism-first",
            format!("{} that helps {} {}.", capitalize(mechanism), audience, outcome),
        ),
        (
            "for-audience",
            format!("For {audience}: {outcome} with less busywork."),
        ),
        (
            "built-for",
            format!("{} built to help {} {}.", capitalize(mechanism), audience, outcome),
        ),
    ]
}

// --- Placement formatters (pure string transforms) ------------------------

/// Meta primary text: the promise followed by the top ranked call to action.
fn format_meta(promise: &str) -> String {
    format!("{} {}", promise, CTAS[0])
}

/// Google headline set: three distinct headlines, each within 30 characters,
/// enforced by `fit_chars` at construction time.
fn format_headlines(outcome: &str, mechanism: &str, audience: &str) -> Vec<String> {
    let mut headlines = vec![
        analysis::fit_chars(&capitalize(outcome), 30),
        analysis::fit_chars(&capitalize(mechanism), 30),
        analysis::fit_chars(&format!("Built for {audience}"), 30),
    ];

    // Drop empties and duplicates while preserving order.
    let mut seen = std::collections::HashSet::new();
    headlines.retain(|h| !h.is_empty() && seen.insert(h.to_lowercase()));

    // Pad to three from the neutral bank if extraction was thin.
    for filler in HEADLINE_BANK {
        if headlines.len() >= 3 {
            break;
        }
        let fitted = analysis::fit_chars(filler, 30);
        if seen.insert(fitted.to_lowercase()) {
            headlines.push(fitted);
        }
    }

    headlines.truncate(3);
    headlines
}

/// Landing hero: the promise as a declarative line, without the trailing period.
fn format_hero(promise: &str) -> String {
    promise.trim_end_matches('.').to_owned()
}

// --- Scoring helpers ------------------------------------------------------

/// A small readability contribution (0 to ~12) so candidates that read more
/// easily win ties between equally grounded options.
fn readability_bonus(flesch: f32) -> f32 {
    flesch.clamp(0.0, 120.0) / 10.0
}

/// Penalize overly long promises so the winner stays tight.
fn length_penalty(text: &str) -> f32 {
    let n = text.chars().count();
    if n > 120 {
        (n - 120) as f32 * 0.2
    } else {
        0.0
    }
}

/// Index of the highest scoring candidate.
fn argmax(candidates: &[Candidate]) -> usize {
    let mut best = 0usize;
    let mut best_score = f32::MIN;
    for (i, c) in candidates.iter().enumerate() {
        if c.score > best_score {
            best_score = c.score;
            best = i;
        }
    }
    best
}

// --- Small text helpers ---------------------------------------------------

/// Uppercase the first character, leave the rest unchanged.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Normalize an extracted sentence: capitalize the first letter and ensure it
/// ends with a period. The text itself is left as extracted (honest, not
/// reworded).
fn normalize_sentence(sentence: &str) -> String {
    let trimmed = sentence.trim();
    let capped = capitalize(trimmed);
    if capped.ends_with(['.', '!', '?']) {
        capped
    } else {
        format!("{capped}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{AudienceInput, ProductInput};

    #[test]
    fn produces_grounded_placements() {
        let product = ProductInput::new(
            "Inventory automation software that saves shop owners hours of manual stock counting.",
        )
        .unwrap();
        let audience = AudienceInput::new("Busy ecommerce shop owners who run lean teams.").unwrap();

        let out = compress_sync(&product, &audience).unwrap();
        let c = &out.compression;

        // Three headlines, each within the Google budget.
        assert_eq!(c.google_headlines.len(), 3);
        assert!(c.google_headlines.iter().all(|h| h.chars().count() <= 30));

        // A trace was produced with candidates and a chosen one.
        let trace = out.trace.unwrap();
        assert!(!trace.candidates.is_empty());
        assert_eq!(trace.candidates.iter().filter(|c| c.chosen).count(), 1);

        // The promise should be highly grounded (every slot came from input).
        let grounded = Grounded::build(&format!("{} {}", product.as_str(), audience.as_str()));
        assert!(grounded.report(c.promise.as_str()).score >= 70);
    }
}
