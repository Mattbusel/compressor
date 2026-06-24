# analysis

The deterministic, model free measurement layer. This module does not generate
anything. It measures, and the measurements are where the thesis becomes math.

Source: [`src/analysis.rs`](../src/analysis.rs)

## TextRank: extractive centrality

`rank_sentences(&[String]) -> Vec<f32>` scores every sentence by centrality.
TextRank is PageRank over a sentence similarity graph: sentences that share
vocabulary with many other central sentences score highest.

The implementation is deliberately direct, no graph library:

1. Each sentence becomes a set of stemmed content tokens.
2. The edge weight between two sentences is their shared token count normalized
   by the log of their lengths, the classic TextRank similarity.
3. Scores are found by power iteration with the standard 0.85 damping factor,
   iterating until the total change drops below a small epsilon.

```rust
let mut next = vec![1.0 - damping; n];
for i in 0..n {
    let mut acc = 0.0;
    for j in 0..n {
        if weight[j][i] > 0.0 && out_weight[j] > 0.0 {
            acc += weight[j][i] / out_weight[j] * score[j];
        }
    }
    next[i] += damping * acc;
}
```

`select_need` then blends the normalized TextRank score (centrality) with a need
cue score (does the sentence contain problem words like "slow", "manual",
"waste"), so the chosen core need sentence is both important and actually about a
need.

## The groundedness scorer (the crown jewel)

This quantifies how far any copy has drifted from what the product literally is.

`Grounded::build(source)` constructs a vocabulary from the source text (product
plus audience): the stemmed content words, plus their four character prefixes.
The prefix set catches word family variants the stemmer alone would miss.

`Grounded::report(copy)` grades a piece of copy:

```rust
pub struct GroundednessReport {
    pub score: u8,            // 0 to 100 coverage
    pub supported: usize,
    pub total: usize,
    pub unsupported: Vec<String>,  // the exact terms with no support
}
```

Each distinct claim bearing term in the copy counts once. A term is supported if
its stem is in the grounded set, or its four character prefix matches a grounded
prefix. The score is the share supported. The `unsupported` list is what makes
the number actionable: it names the specific words the product does not back, so
"88% grounded" comes with "the unsupported claim is `effortless`."

This is the measurement an API wrapper cannot replicate, and it works on the
deterministic output and the model output alike. The hybrid engine uses it to
audit the model; the app shows it for every engine.

A note on honesty about the method: the stemmer is a small suffix stripper and
the prefix match is a heuristic, so the score is directional rather than exact.
That is the right tradeoff for a fast, explainable, dependency free signal, and
the unsupported term list lets a human verify any individual call.

## Readability: Flesch reading ease

`readability(text)` computes the Flesch reading ease score, a closed form over
word, sentence, and syllable counts:

```
206.835 - 1.015 * (words / sentences) - 84.6 * (syllables / words)
```

Higher is easier. The result carries both the number and a band label ("Plain",
"Hard", and so on). Syllables come from the heuristic counter in the `text`
module. The deterministic engine uses this to break ties between equally grounded
promise candidates, preferring the one that reads more easily.

## Constraint checks

`constraint_flags(&Compression)` runs the marketing quality checks:

- Every Google headline must be at most 30 characters; any overrun is flagged.
- Hype words (a small banned list: "revolutionary", "seamless", "guaranteed",
  and so on) anywhere in the copy are flagged.
- An exclamation mark in the promise is flagged.

`hype_count(copy)` is the same hype detection as a count, reused by the
deterministic engine to penalize hype in its candidate scoring.

## String fitting

`fit_chars(s, max)` truncates to at most `max` characters while breaking on a word
boundary. This is how the deterministic engine enforces the Google headline
budget by construction rather than after the fact.

## Tests

The module ships unit tests for the parts that must be correct: groundedness
flags unsupported terms and scores fully grounded copy at 100, `fit_chars`
respects both the budget and word boundaries, readability returns a valid band,
and TextRank scores every sentence and yields a need selection.

## Rust primitives used in this module

- **`HashSet` / `HashMap`**: vocabulary sets and the dedup of counted terms.
- **power iteration** over a `Vec<Vec<f32>>`: TextRank with no graph dependency.
- **struct results** (`GroundednessReport`, `Readability`, `Flag`): typed,
  explainable outputs.
- **`#[cfg(test)]` module**: unit tests for the algorithms.
- **iterator pipelines** (`map`, `filter`, `fold`): the measurement transforms.
