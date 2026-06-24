# Compressor

A native desktop tool that collapses the abstraction stack between a product and
the customer who needs it.

You paste in a product description and a description of who you think the
customer is. Compressor returns one tight artifact:

- one plainly stated **core need**,
- one honest one sentence **promise** connecting the product to that need,
- and that promise expressed for three ad placements: **Meta primary text**, a
  **Google headline set**, and a **landing page hero**.

One input, one tight output. No variation spam.

---

## The three questions

### 1. What does the tool do?

Marketing tools mostly generate. You ask for ad copy and they hand back a wall
of options, then you spend an hour deciding which layer of abstraction to trust:
the feature, the benefit, the benefit of the benefit, the persona, the funnel
stage. Compressor does the opposite. It takes the two things you actually know
(what the product is, and who you think needs it) and removes the layers between
them until one need and one promise are left standing. The promise is then
rendered into exactly the three placements you ship most, at the lengths those
placements demand. Nothing more.

The thesis is compression, not generation. Removing layers, not adding noise.

### 2. Why build this one?

Because the failure mode of AI marketing copy is volume. A model will happily
give you twelve headlines, and twelve headlines is not a decision, it is twelve
more decisions. The discipline that actually moves conversion is naming the real
need in plain language and making a single promise you can defend. That is hard
to do and easy to avoid, so a tool that forces it (one need, one promise, no
alternatives) is doing the part people skip. Compressor is opinionated on
purpose: the constraint is the product.

It is also a faithful, small piece of engineering. Everything is typed
(`ProductInput`, `CoreNeed`, `Promise`, a `Placement` enum, a `Compression`
result), every fallible path returns a `Result`, the network call never blocks
the UI, and the interface is a designed dark theme rather than default widgets.
It is meant to read as something another engineer could pick up and extend.

### 3. What would you build next if it were your full time job?

In rough priority order:

- **Schema enforced output.** Switch the engine from "parse the JSON the model
  promised" to the Messages API `output_config.format` with a JSON schema, so
  malformed output becomes structurally impossible rather than handled after the
  fact. (Today the `EngineError::MalformedJson` path exists precisely because we
  do not do this yet.)
- **Streaming.** Stream the response so the core need appears the instant it is
  ready, with the placements filling in after.
- **A compression score.** Show why this is the compressed form: a readout of
  what layers were removed (feature to benefit to need), so the user trusts the
  collapse instead of taking it on faith.
- **Saved runs and diffing.** Keep a local history and let the user compare two
  audiences for the same product side by side, since "who you think the customer
  is" is the input most worth testing.
- **Character budget awareness in the UI.** Surface the Google 30 character
  limit live, and flag any headline that overruns.
- **Bring your own model and key management.** A small settings panel for the
  model id and key storage, instead of relying solely on the environment.

---

## Setting the API key

Compressor reads the secret from the `ANTHROPIC_API_KEY` environment variable.
The key is never hardcoded and never written to disk by the app.

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

If the variable is missing, the app does not crash. It reports
`the ANTHROPIC_API_KEY environment variable is not set` in the error card.

---

## Building and running

You need a stable Rust toolchain (the project was built with Rust 1.91).

Run in development:

```bash
cargo run
```

Build and run optimized:

```bash
cargo run --release
```

---

## Producing the `.exe`

```bash
cargo build --release
```

The single self contained executable is written to:

```
target/release/compressor.exe
```

It is one file (roughly 4 MB) with no runtime dependencies beyond the system
graphics stack. The release profile is tuned for a small binary: size
optimization (`opt-level = "z"`), link time optimization, a single codegen unit,
`panic = "abort"`, and symbol stripping. The Windows console window is suppressed
in release builds via `windows_subsystem = "windows"`, so launching the `.exe`
shows only the GUI.

---

## Architecture

Four modules, each with one responsibility and its own document in `docs/`:

| Module      | Responsibility                                             | Doc                          |
| ----------- | ---------------------------------------------------------- | ---------------------------- |
| `domain`    | The typed vocabulary. Illegal states unrepresentable.     | [docs/domain.md](docs/domain.md)   |
| `prompt`    | Pure construction of the system and user prompts.         | [docs/prompt.md](docs/prompt.md)   |
| `engine`    | Async Anthropic Messages API client and JSON parsing.     | [docs/engine.md](docs/engine.md)   |
| `app`       | The egui UI, the custom theme, the async to UI bridge.    | [docs/app.md](docs/app.md)         |

The data flows one direction. The `app` validates raw text into `domain`
newtypes, hands them to `engine`, which uses `prompt` to build the request,
calls the API, and parses the answer back into a `domain::Compression` that the
`app` renders.

```
app (UI)  ->  domain (ProductInput, AudienceInput)
              |
              v
           engine.compress(...)  ->  prompt (system + user)  ->  Anthropic API
              |
              v
           domain::Compression   ->  app renders the result
```

---

## A note on the code as a teaching artifact

Throughout the source and these docs, the Rust primitives in use are named and
briefly explained where they appear: the newtype pattern, enum sum types,
`Result` and `thiserror`, `async fn` and `.await`, the `mpsc` channel, trait
impls, lifetimes, and the design token approach to theming. The goal is that
reading the code teaches why each construct is there, not just what it does.


