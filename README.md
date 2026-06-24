# Compressor

A native desktop tool that collapses the abstraction stack between a product and
the customer who needs it.

You paste in a product description and a description of who you think the
customer is, pick an engine, and Compressor returns one tight artifact:

- one plainly stated **core need**,
- one honest one sentence **promise** connecting the product to that need,
- and that promise expressed for three ad placements: **Meta primary text**, a
  **Google headline set**, and a **landing page hero**.

One input, one tight output. No variation spam. Every result is then **graded
for groundedness** (how much of its claim bearing vocabulary the product
actually supports), checked for readability and hype, and, for the deterministic
engine, shown with its full work.

---

## Three engines behind one interface

The core architectural move is a `CompressionEngine` trait with three
implementations the UI can pick between at runtime:

| Engine | What it is | Needs a key |
| --- | --- | --- |
| **Heuristic** | Deterministic, model free. Extractive core need (TextRank), templated promise slotting with scored candidate selection, pure placement formatters. Fully grounded by construction and explainable. Runs offline. | No |
| **Hybrid** | The model writes the copy; the deterministic layer audits it (groundedness) and attaches the explainability trace. Using AI and keeping it honest. | Yes |
| **LLM** | The Anthropic Messages API writes the copy. The scorer still grades it for honesty. | Yes |

The **groundedness scorer sits outside all three** and grades whatever any of
them produces. Run it on the model's promise and you can literally show "this
promise is 92% grounded; this one drifted to 60%, and here are the unsupported
claims."

---

## The three questions

### 1. What does the tool do?

Marketing tools mostly generate. You ask for ad copy and they hand back a wall
of options, then you spend an hour deciding which layer of abstraction to trust.
Compressor does the opposite. It takes the two things you actually know (what the
product is, and who you think needs it) and removes the layers between them until
one need and one promise are left standing, then renders that promise into the
three placements you ship most, at the lengths those placements demand.

Crucially, it does this two ways. The deterministic engine never calls a model:
it extracts the need, slots input derived terms into a promise, scores the
candidates, and shows its work. The model engines write more fluent copy. And the
same groundedness math grades all of them, so the AI output is held to the same
honesty standard as the deterministic one.

The thesis is compression, not generation. Removing layers, not adding noise.

### 2. Why build this one?

Because the failure mode of AI marketing copy is volume and drift. Twelve
headlines is not a decision, it is twelve more decisions, and a fluent promise
that quietly claims a capability the product does not have is worse than no copy
at all. Compressor encodes the discipline that actually moves conversion: name
the real need in plain language, make a single defensible promise, and measure
how far any copy has drifted from what the product literally is.

The groundedness scorer is the part an API wrapper structurally cannot match,
and the deterministic engine's explainability ("core need extracted from
sentence 3, TextRank 0.81; promise template 2 selected; 94% grounded") is a
transparency no model backed tool can offer. The constraint is the product.

### 3. What would you build next if it were your full time job?

The deterministic engine, the groundedness scorer, the hybrid audit, the A/B
candidate scoring, the readability and constraint checks, and the explainability
panel are all built. Next, in rough priority order:

- **Batch / CSV mode.** Media buyers work at scale. Drop in a CSV of products,
  get a compression per row, exported back to CSV, parallelized across rows with
  `rayon`. This reframes the tool from a toy to something a team runs Monday
  morning.
- **Schema enforced LLM output.** Move the model engines to the Messages API
  `output_config.format` with a JSON schema, so malformed output becomes
  structurally impossible rather than handled after the fact.
- **Streaming.** Stream the model response so the core need appears the instant
  it is ready.
- **Local persistence.** Save runs to SQLite (or a flat JSON history) so the tool
  remembers across sessions, and let the user diff two audiences for the same
  product side by side.
- **Destination exports.** A "copy for Meta Ads Manager" action and a Google RSA
  shaped export, because knowing the destination matters, not just the copy.
- **A real POS tagger** for slot extraction, to sharpen the deterministic
  promise's grammar beyond the current frequency and lexicon heuristics.

---

## Setting the API key

The Hybrid and LLM engines read the secret from the `ANTHROPIC_API_KEY`
environment variable. The Heuristic engine needs nothing and runs offline. The
key is never hardcoded and never written to disk by the app.

PowerShell (current session):

```powershell
$env:ANTHROPIC_API_KEY = "sk-ant-..."
```

PowerShell (persist for your user):

```powershell
setx ANTHROPIC_API_KEY "sk-ant-..."
```

Bash:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
```

If a model engine is selected without the key set, the app does not crash; it
reports `the ANTHROPIC_API_KEY environment variable is not set` in the error
card.

---

## Building and running

You need a stable Rust toolchain (built with Rust 1.91).

```bash
cargo run            # development
cargo run --release  # optimized
cargo test           # run the analysis and engine unit tests
```

The deterministic engine works with no key, so `cargo run` then selecting
**Heuristic** is the fastest way to see it work end to end.

---

## Producing the `.exe`

```bash
cargo build --release
```

The single self contained executable is written to
`target/release/compressor.exe` (roughly 4 MB, no runtime dependencies beyond the
system graphics stack). The release profile is tuned for a small binary: size
optimization (`opt-level = "z"`), link time optimization, a single codegen unit,
`panic = "abort"`, and symbol stripping. The Windows console is suppressed in
release via `windows_subsystem = "windows"`.

---

## Architecture

Each module has one responsibility and its own document in `docs/`.

| Module      | Responsibility                                              | Doc                                |
| ----------- | ---------------------------------------------------------- | ---------------------------------- |
| `domain`    | The typed vocabulary. Illegal states unrepresentable.     | [docs/domain.md](docs/domain.md)   |
| `text`      | Dependency free text primitives (tokenize, stem, syllables).| [docs/text.md](docs/text.md)       |
| `analysis`  | The deterministic measurement layer: TextRank, groundedness, readability, constraints. | [docs/analysis.md](docs/analysis.md) |
| `prompt`    | Pure construction of the system and user prompts.         | [docs/prompt.md](docs/prompt.md)   |
| `engine`    | The `CompressionEngine` trait and its three engines.      | [docs/engine.md](docs/engine.md)   |
| `app`       | The egui UI, the custom theme, the async to UI bridge.    | [docs/app.md](docs/app.md)         |

Data flows one direction. The `app` validates raw text into `domain` newtypes,
dispatches to the selected `engine` (which may use `analysis` and `text`), grades
the result with the `analysis` groundedness scorer, and renders it.

```
app (UI)  ->  domain (ProductInput, AudienceInput)
              |
              v
        engine::run(kind, ...)  ->  Heuristic | Hybrid | LLM   ->  EngineOutput
              |                          (uses analysis + text + prompt)
              v
     analysis::Grounded grades the output, outside the engine
              |
              v
        app renders the result, the score, and the explainability trace
```

---

## A note on the code as a teaching artifact

Throughout the source and these docs, the Rust primitives in use are named and
briefly explained where they appear: the newtype pattern, enum sum types,
`Result` and `thiserror`, traits and `async fn` in traits, boxed futures for
runtime dispatch, the `mpsc` channel, `OnceLock`, power iteration, lifetimes, and
the design token approach to theming.


