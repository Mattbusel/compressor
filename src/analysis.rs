//! analysis: the deterministic, model free measurement layer.
//!
//! This module is where the thesis becomes math. It does not generate anything.
//! It measures: how central a sentence is (TextRank), how grounded a piece of
//! copy is in the source vocabulary (the groundedness scorer), how readable it
//! is (Flesch reading ease), and whether it violates basic marketing
//! constraints (hype words, headline length). The groundedness scorer sits
//! outside both engines and grades whatever either produces.
//!
//! Rust primitives used here:
//!   - `HashSet` / `HashMap`: vocabulary sets and frequency tables.
//!   - power iteration: a plain `Vec<Vec<f32>>` weight matrix and a loop, which
//!     is all TextRank (PageRank on a sentence similarity graph) needs.
//!   - struct results (`GroundednessReport`, `Readability`, `Flag`): typed,
//!     explainable outputs rather than bare numbers.
//!   - `#[cfg(test)]` module: unit tests for the algorithms.

use std::collections::HashSet;

use crate::domain::Compression;
use crate::text;

// --- TextRank -------------------------------------------------------------

/// Score every sentence by centrality using TextRank.
///
/// TextRank is PageRank over a sentence similarity graph: sentences that share
/// vocabulary with many other central sentences score highest. We build a
/// weighted adjacency matrix from token overlap, then run power iteration with
/// the standard damping factor until the scores converge.
pub fn rank_sentences(sentences: &[String]) -> Vec<f32> {
    let n = sentences.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![1.0];
    }

    // Each sentence becomes a set of stemmed content tokens.
    let tokens: Vec<HashSet<String>> = sentences
        .iter()
        .map(|s| text::content_words(s).into_iter().map(|w| text::stem(&w)).collect())
        .collect();

    // Weighted similarity: shared tokens normalized by the log lengths, the
    // classic TextRank sentence similarity measure.
    let mut weight = vec![vec![0.0f32; n]; n];
    for i in 0..n {
        for j in 0..n {
            if i == j {
                continue;
            }
            let common = tokens[i].intersection(&tokens[j]).count();
            let (li, lj) = (tokens[i].len(), tokens[j].len());
            if common > 0 && li > 1 && lj > 1 {
                let denom = (li as f32).ln() + (lj as f32).ln();
                if denom > 0.0 {
                    weight[i][j] = common as f32 / denom;
                }
            }
        }
    }

    let out_weight: Vec<f32> = weight.iter().map(|row| row.iter().sum()).collect();

    // Power iteration.
    let damping = 0.85f32;
    let mut score = vec![1.0f32; n];
    for _ in 0..50 {
        let mut next = vec![1.0 - damping; n];
        for i in 0..n {
            let mut acc = 0.0f32;
            for j in 0..n {
                if weight[j][i] > 0.0 && out_weight[j] > 0.0 {
                    acc += weight[j][i] / out_weight[j] * score[j];
                }
            }
            next[i] += damping * acc;
        }
        let delta: f32 = (0..n).map(|i| (next[i] - score[i]).abs()).sum();
        score = next;
        if delta < 1e-4 {
            break;
        }
    }
    score
}

/// Words that signal a need or problem. The core need selector prefers a
/// sentence that is both central (TextRank) and need shaped.
const NEED_CUES: &[&str] = &[
    "need", "want", "struggle", "problem", "hard", "difficult", "slow", "manual", "manually",
    "time", "hours", "waste", "wasting", "cannot", "lack", "without", "tedious", "repetitive",
    "error", "mistake", "miss", "losing", "lose", "overwhelmed", "busy", "juggling", "spend",
    "spending", "stuck", "behind", "chaos", "messy", "scattered",
];

fn need_cue_count(sentence: &str) -> usize {
    let cues: HashSet<&str> = NEED_CUES.iter().copied().collect();
    text::words(sentence)
        .into_iter()
        .filter(|w| cues.contains(w.as_str()))
        .count()
}

/// Pick the index of the sentence that best names the core need.
///
/// We blend the normalized TextRank score (centrality) with a need cue score
/// (problem shape), so the chosen sentence is both important and about a need.
pub fn select_need(sentences: &[String], scores: &[f32]) -> Option<usize> {
    if sentences.is_empty() {
        return None;
    }
    let max_score = scores.iter().cloned().fold(0.0f32, f32::max).max(1e-6);

    let mut best = 0usize;
    let mut best_value = f32::MIN;
    for (i, sentence) in sentences.iter().enumerate() {
        let centrality = scores.get(i).copied().unwrap_or(0.0) / max_score; // 0..1
        let cue = (need_cue_count(sentence).min(3) as f32) / 3.0; // 0..1
        let value = centrality * 0.6 + cue * 0.4;
        if value > best_value {
            best_value = value;
            best = i;
        }
    }
    Some(best)
}

// --- Groundedness scorer (the crown jewel) --------------------------------

/// A grounded vocabulary built from the source text (product plus audience).
///
/// It holds stemmed content words plus their four character prefixes. A copy
/// term counts as supported if its stem is in the set, or its prefix matches a
/// grounded prefix (which catches word family variants the stemmer misses).
pub struct Grounded {
    stems: HashSet<String>,
    prefixes: HashSet<String>,
}

impl Grounded {
    /// Build the vocabulary from the source text the copy must stay true to.
    pub fn build(source: &str) -> Self {
        let mut stems = HashSet::new();
        let mut prefixes = HashSet::new();
        for word in text::content_words(source) {
            let stemmed = text::stem(&word);
            if stemmed.len() >= 4 {
                prefixes.insert(stemmed[..4].to_owned());
            }
            stems.insert(stemmed);
        }
        Self { stems, prefixes }
    }

    /// Grade a piece of copy against the grounded vocabulary.
    ///
    /// Returns a 0 to 100 coverage score (the share of unique claim bearing
    /// terms that are supported by the source) plus the exact list of
    /// unsupported terms, which is what makes the score actionable.
    pub fn report(&self, copy: &str) -> GroundednessReport {
        let mut seen = HashSet::new();
        let mut total = 0usize;
        let mut supported = 0usize;
        let mut unsupported = Vec::new();

        for word in text::content_words(copy) {
            let stemmed = text::stem(&word);
            if !seen.insert(stemmed.clone()) {
                continue; // count each distinct term once
            }
            total += 1;
            let ok = self.stems.contains(&stemmed)
                || (stemmed.len() >= 4 && self.prefixes.contains(&stemmed[..4]));
            if ok {
                supported += 1;
            } else {
                unsupported.push(word);
            }
        }

        let score = if total == 0 {
            100
        } else {
            ((supported as f32 / total as f32) * 100.0).round() as u8
        };
        GroundednessReport {
            score,
            supported,
            total,
            unsupported,
        }
    }
}

/// The result of grading copy for groundedness.
#[derive(Debug, Clone)]
pub struct GroundednessReport {
    pub score: u8,
    pub supported: usize,
    pub total: usize,
    pub unsupported: Vec<String>,
}

// --- Readability ----------------------------------------------------------

/// Flesch reading ease for a piece of copy.
#[derive(Debug, Clone)]
pub struct Readability {
    pub flesch: f32,
    pub grade: &'static str,
}

/// Compute Flesch reading ease: a closed form over word, sentence, and syllable
/// counts. Higher is easier. We clamp to a sane display range.
pub fn readability(text_in: &str) -> Readability {
    let sentence_count = text::sentences(text_in).len().max(1);
    let words = text::words(text_in);
    let word_count = words.len().max(1);
    let syllable_count: usize = words.iter().map(|w| text::syllables(w)).sum();

    let flesch = 206.835
        - 1.015 * (word_count as f32 / sentence_count as f32)
        - 84.6 * (syllable_count as f32 / word_count as f32);
    let flesch = flesch.clamp(0.0, 120.0);

    Readability {
        flesch,
        grade: band(flesch),
    }
}

fn band(flesch: f32) -> &'static str {
    match flesch {
        f if f >= 90.0 => "Very easy",
        f if f >= 70.0 => "Easy",
        f if f >= 60.0 => "Plain",
        f if f >= 50.0 => "Fairly hard",
        f if f >= 30.0 => "Hard",
        _ => "Very hard",
    }
}

// --- Constraints ----------------------------------------------------------

/// Marketing hype words the copy should avoid. The deterministic engine never
/// emits these; the scorer flags them in any copy, including the model's.
pub const HYPE_WORDS: &[&str] = &[
    "revolutionary", "gamechanger", "best", "world-class", "cutting-edge", "unleash",
    "supercharge", "ultimate", "amazing", "incredible", "unbeatable", "unrivaled", "disrupt",
    "disruptive", "seamless", "effortless", "magical", "insane", "blazing", "skyrocket",
    "guaranteed", "10x", "limitless",
];

/// The hype words present in a piece of copy.
///
/// Plain alphabetic hype words must match as whole words (so "best" does not
/// fire inside "bestsellers"); hyphenated or numeric entries like "world-class"
/// are matched as substrings because tokenization would split them.
pub fn hype_hits(copy: &str) -> Vec<&'static str> {
    let lower = copy.to_lowercase();
    let tokens: HashSet<String> = text::words(copy).into_iter().collect();
    HYPE_WORDS
        .iter()
        .copied()
        .filter(|h| {
            if h.contains('-') || h.chars().any(|c| c.is_ascii_digit()) {
                lower.contains(h)
            } else {
                tokens.contains(*h)
            }
        })
        .collect()
}

/// Count hype words present in a piece of copy.
pub fn hype_count(copy: &str) -> usize {
    hype_hits(copy).len()
}

/// A single constraint issue, phrased for the user.
#[derive(Debug, Clone)]
pub struct Flag {
    pub message: String,
}

/// Run every constraint check over a finished compression.
pub fn constraint_flags(c: &Compression) -> Vec<Flag> {
    let mut flags = Vec::new();

    // Google headlines must be at most 30 characters each.
    for headline in &c.google_headlines {
        let len = headline.chars().count();
        if len > 30 {
            flags.push(Flag {
                message: format!("Google headline over 30 characters ({len}): \"{headline}\""),
            });
        }
    }

    // Hype words anywhere in the copy.
    let combined = format!(
        "{} {} {} {} {}",
        c.core_need.as_str(),
        c.promise.as_str(),
        c.meta_primary_text,
        c.google_headlines.join(" "),
        c.landing_hero,
    );
    let found = hype_hits(&combined);
    if !found.is_empty() {
        flags.push(Flag {
            message: format!("Hype language detected: {}", found.join(", ")),
        });
    }

    // The promise should not shout.
    if c.promise.as_str().contains('!') {
        flags.push(Flag {
            message: "The promise contains an exclamation mark".to_owned(),
        });
    }

    flags
}

// --- String fitting -------------------------------------------------------

/// Truncate to at most `max` characters, breaking on a word boundary.
///
/// Used to enforce the Google headline budget by construction rather than
/// hoping the copy happens to fit.
pub fn fit_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out = String::new();
    for word in s.split_whitespace() {
        let candidate_len = if out.is_empty() {
            word.chars().count()
        } else {
            out.chars().count() + 1 + word.chars().count()
        };
        if candidate_len > max {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    if out.is_empty() {
        out = s.chars().take(max).collect();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groundedness_flags_unsupported_terms() {
        let grounded = Grounded::build("inventory automation software for online stores");
        // "revolutionary" and "blockchain" are not in the source.
        let report = grounded.report("revolutionary blockchain inventory automation");
        assert!(report.total >= 3);
        assert!(report.unsupported.iter().any(|w| w == "revolutionary"));
        assert!(report.unsupported.iter().any(|w| w == "blockchain"));
        assert!(report.score < 100);
    }

    #[test]
    fn fully_grounded_copy_scores_high() {
        let grounded = Grounded::build("automated invoicing for freelancers");
        let report = grounded.report("automated invoicing for freelancers");
        assert_eq!(report.score, 100);
        assert!(report.unsupported.is_empty());
    }

    #[test]
    fn fit_chars_respects_word_boundary_and_budget() {
        let fit = fit_chars("Save hours on inventory management", 20);
        assert!(fit.chars().count() <= 20);
        assert!(!fit.ends_with(' '));
        // It should keep whole words.
        assert!("Save hours on inventory management".starts_with(&fit));
    }

    #[test]
    fn readability_returns_a_band() {
        let r = readability("This tool saves you time.");
        assert!(r.flesch >= 0.0 && r.flesch <= 120.0);
        assert!(!r.grade.is_empty());
    }

    #[test]
    fn hype_matches_whole_words_only() {
        // "best" must not fire inside "bestsellers".
        assert_eq!(hype_count("our bestsellers fly off the shelf"), 0);
        // standalone "best" should fire.
        assert!(hype_count("the best tool for the job") >= 1);
        // hyphenated entries still match as substrings.
        assert!(hype_count("a world-class, seamless tool") >= 1);
    }

    #[test]
    fn stemmer_matches_word_families() {
        // "running" -> "run", "saves" -> "save": copy that reuses the product's
        // own vocabulary in other forms should read as grounded.
        let grounded = Grounded::build("we run out of stock and save you hours");
        let report = grounded.report("running stock, saves hours");
        assert!(report.unsupported.is_empty(), "unexpected: {:?}", report.unsupported);
        assert_eq!(report.score, 100);
    }

    #[test]
    fn textrank_scores_every_sentence() {
        let sentences = vec![
            "Shop owners waste hours counting stock by hand.".to_owned(),
            "Our tool automates inventory counts for shop owners.".to_owned(),
            "It syncs stock levels across every store.".to_owned(),
        ];
        let scores = rank_sentences(&sentences);
        assert_eq!(scores.len(), 3);
        assert!(scores.iter().all(|s| *s > 0.0));
        assert!(select_need(&sentences, &scores).is_some());
    }
}
