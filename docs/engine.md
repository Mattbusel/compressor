# engine

The compression engines and the interface they share. This is where the central
architectural move lives: one trait, three implementations, and a dispatcher the
UI calls without caring which engine ran.

Source: [`src/engine/`](../src/engine/) (`mod.rs`, `heuristic.rs`, `hybrid.rs`,
`llm.rs`)

## The interface

```rust
#[allow(async_fn_in_trait)]
pub trait CompressionEngine {
    async fn compress(
        &self,
        product: ProductInput,
        audience: AudienceInput,
    ) -> Result<EngineOutput, EngineError>;
}
```

`async fn` in a trait has been stable since Rust 1.75. We use it through concrete
types, never through `dyn`, which sidesteps the usual object safety and `Send`
caveats. Each engine takes its inputs by value so the returned future owns
everything it needs and can be moved onto the runtime.

Every engine returns the same shape:

```rust
pub struct EngineOutput {
    pub compression: Compression,
    pub trace: Option<Trace>,
}
```

The `Trace` is the explainability payload. The deterministic and hybrid engines
fill it; the pure LLM engine leaves it `None`, because a model leaves no
deterministic work to show.

## Runtime dispatch with boxed futures

The UI picks an engine at runtime, so we need a single type to spawn regardless
of choice. Each engine's `compress` returns a different concrete future, so we
box them into one type:

```rust
pub type BoxedCompression =
    Pin<Box<dyn Future<Output = Result<EngineOutput, EngineError>> + Send>>;

pub fn run(kind: EngineKind, product: ProductInput, audience: AudienceInput) -> BoxedCompression {
    match kind {
        EngineKind::Heuristic => Box::pin(HeuristicEngine.compress(product, audience)),
        EngineKind::Hybrid => Box::pin(HybridEngine.compress(product, audience)),
        EngineKind::Llm => Box::pin(LlmEngine.compress(product, audience)),
    }
}
```

Because each concrete future is known to be `Send`, boxing it into a trait object
is sound, and the app can `spawn` the result uniformly. `EngineKind` is an enum
sum type, so the `match` is exhaustively checked.

## The exhaustive error enum

`EngineError` covers every failure mode across all three engines:

```rust
pub enum EngineError {
    MissingApiKey,                          // env var not set (model engines)
    Http(#[from] reqwest::Error),           // transport failure
    BadStatus { status: u16, body: String },// non success HTTP status
    NoContent,                              // no text block in the response
    MalformedJson(String),                  // response or model JSON did not parse
    InsufficientInput(String),              // heuristic could not extract enough
}
```

`#[from] reqwest::Error` lets `?` convert transport errors automatically.
`InsufficientInput` is new for the deterministic engine, which can fail if the
input is too sparse to extract a need or a slot from.

## HeuristicEngine: deterministic compression

Source: [`src/engine/heuristic.rs`](../src/engine/heuristic.rs)

No model is involved. The pipeline:

1. **Extract the core need.** Split the combined input into sentences, score them
   with TextRank (see [docs/analysis.md](analysis.md)), and pick the sentence
   that is both central and need shaped. The need is extracted, not generated.
2. **Extract promise slots.** The audience phrase is the first run of content
   words from the audience input. The outcome is a benefit verb present in the
   product (paired with a benefit object if one is also present, for example
   "save hours"). The mechanism is the product's most frequent content noun.
   Every slot is lifted from the input, so the engine cannot invent a capability.
3. **Build and score candidates.** Fill a small grammar of promise templates,
   then score each candidate with the groundedness scorer, readability, and a
   hype penalty. Keep the winner. This is the disciplined version of variation:
   not ten options dumped on the user, but the best one with its reasoning.
4. **Format the placements.** Pure string transforms. The Google headlines are
   built within the 30 character budget by construction (via `analysis::fit_chars`
   truncating at a word boundary), not hoped to fit.
5. **Record the trace.** The need source sentence and its TextRank score, the
   chosen template, and every candidate with its scores, for the explainability
   panel.

The synchronous core is `compress_sync`, exposed `pub(crate)` so the hybrid
engine can reuse it for the explainability trace.

## LlmEngine: model backed compression

Source: [`src/engine/llm.rs`](../src/engine/llm.rs)

The Anthropic Messages API client. It builds the request from the pure `prompt`
module, calls `claude-opus-4-8`, reads the secret from `ANTHROPIC_API_KEY`, and
parses the model's JSON answer into the domain types. Typed serde request and
response payloads, defensive JSON extraction (slice from the first `{` to the
last `}`), and validation into `Compression`. It returns `trace: None`.

## HybridEngine: AI, kept honest

Source: [`src/engine/hybrid.rs`](../src/engine/hybrid.rs)

The demonstration of the whole thesis in one pass. It computes the deterministic
trace and the grounded vocabulary from the inputs first (by reference), then lets
the model write the copy (consuming the inputs). It runs the groundedness scorer
over the model's promise and writes the audit into the trace notes:

> Model promise is 88% grounded (7 of 8 claim terms supported by the product).
> Unsupported claims the model introduced: effortless.

So the user gets the model's fluency, the deterministic engine's transparency,
and an explicit flag on any claim the model added that the product does not back.

## A note on where grading happens

None of the engines grade themselves. The groundedness scorer lives in
`analysis` and is run by the app (and, for its audit notes, by the hybrid engine)
on the finished `Compression`. This keeps the measurement independent of the
thing being measured, and lets the same score be shown for every engine.

## Rust primitives used in this module

- **trait with `async fn`** (`CompressionEngine`): the shared interface.
- **trait impls** on unit structs (`HeuristicEngine`, `LlmEngine`, `HybridEngine`).
- **boxed futures** (`Pin<Box<dyn Future + Send>>`): uniform runtime dispatch.
- **enum sum types** (`EngineKind`, `EngineError`): closed engine set, exhaustive
  failures.
- **`?` and `#[from]`**: error propagation and auto conversion.
- **`pub(crate)`**: share `compress_sync` across the engine module only.
- **composition**: hybrid reuses the heuristic core and the analysis scorer.
