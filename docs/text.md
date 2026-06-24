# text

Dependency free text primitives shared by the `analysis` and `engine` modules.
Small, pure, and unit tested, so the algorithms above them stand on a known
foundation.

Source: [`src/text.rs`](../src/text.rs)

## What it provides

- `sentences(text)`: split into sentences on terminal punctuation and line
  breaks. Deliberately simple, not a full segmenter.
- `words(text)`: lowercased tokens split on any non alphanumeric boundary.
- `content_words(text)`: tokens that carry meaning, with stopwords, very short
  tokens, and pure numbers removed. This is the vocabulary the scorer and
  TextRank reason about.
- `stem(word)`: a conservative suffix stripping stemmer.
- `syllables(word)`: a syllable estimate for the readability formula.
- `is_stopword(word)`: membership in a compact English stopword list.

## The stemmer

This is not Porter. It is a small, predictable suffix remover that tries longer,
more specific suffixes first and keeps at least three characters of root:

```rust
const SUFFIXES: &[&str] = &[
    "izations", "ization", "ational", "ations", "ation", "ingly", "edly",
    "ings", "ing", "edness", "ness", "ments", "ment", "tions", "tion",
    "ers", "ies", "ied", "er", "ed", "ly", "es", "s",
];
```

The goal is that word families (automate / automation / automating) tend to
collapse to a shared root. The stem does not have to be a real word; it only has
to be consistent, because both the grounded vocabulary and the copy under test
are stemmed the same way. The groundedness scorer adds a four character prefix
match on top, to catch families the stemmer alone would miss.

## Syllable counting

`syllables` counts vowel groups and subtracts one for a trailing silent `e`. It
is an estimate, which is all the Flesch reading ease formula needs, since that
formula works on aggregate syllable counts rather than exact per word values.

## The stopword set

`is_stopword` is backed by a `HashSet` built once via `OnceLock` and shared for
the life of the program. The list is the common English function words that carry
little claim bearing meaning, so they are excluded from groundedness scoring and
sentence ranking.

## Rust primitives used in this module

- **`OnceLock`**: build the stopword set once, lazily, and share it.
- **iterator adapters** (`split`, `filter`, `map`, `collect`): the whole module
  is small transformations over `&str`.
- **`&'static str` slices**: the suffix and stopword lists are baked into the
  binary.
- **`matches!`**: the vowel test in the syllable counter.
