# engine

The async Anthropic Messages API client. This module owns every fallible
interaction with the outside world: reading the API key, building and sending
the HTTP request, and parsing the model's answer into domain types.

Source: [`src/engine.rs`](../src/engine.rs)

## The single entry point

```rust
pub async fn compress(
    product: ProductInput,
    audience: AudienceInput,
) -> Result<Compression, EngineError>
```

`async fn` means this returns a future rather than running immediately. The
caller drives it with `.await` on a tokio runtime (see [docs/app.md](app.md) for
how the UI does that without blocking). The function takes its inputs **by
value** (owned `ProductInput` / `AudienceInput`) so the future can be moved into
a spawned task with no borrowed data tying it to the caller's stack.

The body reads top to bottom as the happy path, with each failure short
circuiting through the `?` operator:

1. Read `ANTHROPIC_API_KEY` from the environment, or return `MissingApiKey`.
2. Build the system and user prompts (pure, from the `prompt` module).
3. Serialize and POST the request.
4. Check the HTTP status; a non success status returns `BadStatus`.
5. Parse the response envelope, find the text block, slice out the JSON object,
   and parse it into the typed result.

## Configuration constants

```rust
const API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MODEL: &str = "claude-opus-4-8";
const MAX_TOKENS: u32 = 1024;
```

The model is Anthropic's most capable Opus tier model. The copy is short, so a
small `max_tokens` ceiling is plenty. The version string is the value the
Messages API requires in the `anthropic-version` header. The key is read from
the environment and never hardcoded.

## The exhaustive error enum

`EngineError` is the heart of the module's contract. It is an enum sum type with
one variant per failure mode, deriving `thiserror::Error` so each carries a
`Display` message:

```rust
pub enum EngineError {
    MissingApiKey,                         // env var not set
    Http(#[from] reqwest::Error),          // transport failure
    BadStatus { status: u16, body: String }, // non success HTTP status
    NoContent,                             // no text block to read
    MalformedJson(String),                 // response or model JSON did not parse
}
```

Two details worth calling out:

- `#[from] reqwest::Error` generates a `From<reqwest::Error>` impl, which is what
  lets `?` automatically convert a transport error into `EngineError::Http`. You
  write `client.post(...).send().await?` and the conversion is implicit.
- `BadStatus` keeps both the numeric status and the response body, so the error
  card in the UI can show exactly what the API said.

Because the UI matches on the result type and the engine returns
`Result<Compression, EngineError>`, every one of these failures has a precise,
typed path to a message the user can read.

## Typed request and response payloads

The request is a `#[derive(Serialize)]` struct that **borrows** the prompt
strings via a lifetime, so building it copies nothing:

```rust
#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: Vec<RequestMessage<'a>>,
}
```

The response is `#[derive(Deserialize)]`. The API tags each content block with a
`type` field, which we map to a Rust field named `kind`:

```rust
#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}
```

`RawCompression` is the Rust mirror of the prompt's output contract. Its field
names are exactly the JSON keys the system prompt demands, so serde validates the
model's answer for us:

```rust
#[derive(Deserialize)]
struct RawCompression {
    core_need: String,
    promise: String,
    meta_primary_text: String,
    google_headlines: Vec<String>,
    landing_hero: String,
}
```

## Parsing, defensively

We read the raw response body and parse it ourselves with `serde_json::from_str`
so JSON failures map to `MalformedJson` rather than a generic transport error.
We then find the first `text` block (consuming the vec with `into_iter` so the
`String` moves out without cloning) or return `NoContent`.

Before parsing the model's JSON, `extract_json_object` slices from the first `{`
to the last `}`:

```rust
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end > start { Some(&text[start..=end]) } else { None }
}
```

This keeps the engine robust against a model that occasionally wraps the JSON in
a sentence or a code fence, without dragging in a full tolerant parser.

## Validation into the domain

`build_compression` lifts the parsed JSON into the `domain::Compression` type and
enforces the invariants serde cannot:

- `core_need`, `promise`, `meta_primary_text`, and `landing_hero` are trimmed and
  must be non empty, otherwise `MalformedJson`.
- `google_headlines` is trimmed and filtered; at least one headline must remain.

Only after these checks does it construct `CoreNeed::new(...)` and
`Promise::new(...)`, so a `Compression` value is always a usable result.

## TLS and the single binary

reqwest is configured with `rustls-tls` (and default features off), which avoids
a system OpenSSL dependency. That matters for shipping one self contained
Windows `.exe` with no external runtime requirements.

## Rust primitives used in this module

- **`async fn` + `.await`**: non blocking network call.
- **serde `Serialize` / `Deserialize` derives**: typed JSON in and out.
- **lifetimes** (`MessagesRequest<'a>`): borrow the prompt strings, no clone.
- **enum sum type + `thiserror`**: one exhaustive `EngineError`.
- **`?` operator and `#[from]`**: propagate and auto convert errors.
- **`#[serde(rename = "type")]`**: map a reserved JSON key to a Rust field.
- **`into_iter` move semantics**: take the `String` out of the response vec.
