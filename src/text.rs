//! text: dependency free text primitives shared by the analysis and engine
//! modules. Everything here is pure: tokenization, sentence splitting,
//! stopword filtering, a light stemmer, and a syllable estimator. These are the
//! building blocks the deterministic engine and the groundedness scorer stand
//! on, so they live in one place and are unit tested.
//!
//! Rust primitives used here:
//!   - `OnceLock`: build the stopword set once, lazily, and share it.
//!   - iterator adapters (`split`, `filter`, `map`, `collect`): the whole module
//!     is small transformations over `&str`.
//!   - `&'static str` slices: the word lists are baked into the binary.

use std::collections::HashSet;
use std::sync::OnceLock;

/// Split text into sentences on terminal punctuation and line breaks.
///
/// Deliberately simple: it does not try to be a full sentence segmenter. It
/// breaks on `.`, `!`, `?`, and newlines, and keeps non empty trimmed pieces.
pub fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if ch == '\n' || ch == '\r' {
            push_trimmed(&mut out, &mut current);
            continue;
        }
        current.push(ch);
        if ch == '.' || ch == '!' || ch == '?' {
            push_trimmed(&mut out, &mut current);
        }
    }
    push_trimmed(&mut out, &mut current);
    out
}

fn push_trimmed(out: &mut Vec<String>, buf: &mut String) {
    let trimmed = buf.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_owned());
    }
    buf.clear();
}

/// Lowercased word tokens, split on any non alphanumeric boundary.
pub fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect()
}

/// Content words: tokens that carry meaning. Drops stopwords, very short
/// tokens, and pure numbers. This is the vocabulary the scorer reasons about.
pub fn content_words(text: &str) -> Vec<String> {
    words(text)
        .into_iter()
        .filter(|w| w.len() >= 2 && !is_stopword(w) && w.chars().any(|c| c.is_alphabetic()))
        .collect()
}

/// A conservative suffix stripping stemmer.
///
/// This is not Porter; it is a small, predictable suffix remover so that word
/// families (automate / automation / automating) tend to collapse to a shared
/// root. Both the grounded vocabulary and the copy under test are stemmed the
/// same way, so the comparison is consistent even when the stem is not a real
/// word. Longer, more specific suffixes are tried first.
pub fn stem(word: &str) -> String {
    let w = word.to_lowercase();
    // Derivational and verb suffixes, longest first. Plurals are handled
    // separately below so that "saves" stems to "save", not "sav".
    const SUFFIXES: &[&str] = &[
        "izations", "ization", "ational", "ations", "ation", "ingly", "edly", "ings", "ing",
        "edness", "ness", "ments", "ment", "tions", "tion", "ers", "er", "ed", "ly",
    ];
    for suffix in SUFFIXES {
        // Keep at least three characters of root so we do not over strip.
        if w.len() > suffix.len() + 2 && w.ends_with(suffix) {
            let root = w[..w.len() - suffix.len()].to_owned();
            // "running" -> "run", "stopping" -> "stop" (but keep "spelling").
            if *suffix == "ing" || *suffix == "ed" {
                return undouble(root);
            }
            return root;
        }
    }
    // Plurals and third person singular.
    if w.len() > 4 && w.ends_with("ies") {
        return format!("{}y", &w[..w.len() - 3]); // categories -> category
    }
    if w.len() > 3 && w.ends_with("es") {
        let before = w[..w.len() - 2].chars().last();
        let needs_es = matches!(before, Some('s' | 'x' | 'z' | 'o'))
            || w.ends_with("ches")
            || w.ends_with("shes");
        if needs_es {
            return w[..w.len() - 2].to_owned(); // boxes -> box, watches -> watch
        }
        return w[..w.len() - 1].to_owned(); // saves -> save, invoices -> invoice
    }
    if w.len() > 3 && w.ends_with('s') && !w.ends_with("ss") {
        return w[..w.len() - 1].to_owned(); // counts -> count
    }
    w
}

/// Collapse a final doubled consonant left by stripping "ing"/"ed", except for
/// l, s, z, so "running" becomes "run" while "spelling" stays "spell". Only
/// applied when at least three characters remain.
fn undouble(root: String) -> String {
    let chars: Vec<char> = root.chars().collect();
    let n = chars.len();
    if n >= 4 {
        let last = chars[n - 1];
        let prev = chars[n - 2];
        let vowel = matches!(last, 'a' | 'e' | 'i' | 'o' | 'u');
        if last == prev && !vowel && !matches!(last, 'l' | 's' | 'z') {
            return chars[..n - 1].iter().collect();
        }
    }
    root
}

/// Estimate the number of syllables in a word.
///
/// Heuristic: count vowel groups, then subtract one for a trailing silent `e`.
/// Good enough for the Flesch reading ease formula, which only needs aggregate
/// syllable counts.
pub fn syllables(word: &str) -> usize {
    let cleaned: String = word.chars().filter(|c| c.is_alphabetic()).collect();
    if cleaned.is_empty() {
        return 0;
    }
    let cleaned = cleaned.to_lowercase();
    let is_vowel = |c: char| matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y');

    let mut count = 0usize;
    let mut prev_vowel = false;
    for c in cleaned.chars() {
        let vowel = is_vowel(c);
        if vowel && !prev_vowel {
            count += 1;
        }
        prev_vowel = vowel;
    }
    if cleaned.ends_with('e') && count > 1 {
        count -= 1;
    }
    count.max(1)
}

/// Is this lowercased token a stopword?
pub fn is_stopword(word: &str) -> bool {
    stopword_set().contains(word)
}

/// Build the stopword set once and reuse it (the `OnceLock` pattern).
fn stopword_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| STOPWORDS.iter().copied().collect())
}

/// A compact English stopword list. Common function words that carry little
/// claim bearing meaning, so they are excluded from groundedness and ranking.
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "but", "if", "then", "else", "when", "while", "of", "to",
    "in", "on", "at", "by", "for", "with", "about", "as", "into", "from", "up", "down", "out",
    "over", "under", "is", "are", "was", "were", "be", "been", "being", "am", "do", "does",
    "did", "doing", "have", "has", "had", "having", "this", "that", "these", "those", "it",
    "its", "they", "them", "their", "we", "us", "our", "you", "your", "i", "me", "my", "he",
    "she", "his", "her", "who", "whom", "which", "what", "where", "why", "how", "all", "any",
    "both", "each", "few", "more", "most", "other", "some", "such", "no", "nor", "not", "only",
    "own", "same", "so", "than", "too", "very", "can", "will", "just", "should", "now", "also",
    "get", "got", "make", "made", "use", "used", "using", "via", "per", "etc", "really",
    // Structural function words: present in copy but not claim bearing, so they
    // should not count for or against groundedness.
    "without", "within", "into", "onto", "across", "between", "through", "around", "upon",
    "your", "every", "because", "however", "therefore", "still", "rather", "would",
    "less", "more", "stop", "much", "many", "lot",
];
