//! engine: the async Anthropic Messages API client.
//!
//! This module owns every fallible interaction with the outside world: reading
//! the API key, building and sending the HTTP request, and parsing the model's
//! JSON answer into domain types. Each distinct failure has its own variant in
//! [`EngineError`], so the UI can report precisely what went wrong.
//!
//! Rust primitives used here:
//!   - `async fn` + `.await`: the network call is asynchronous; `.await` yields
//!     control back to the tokio runtime instead of blocking a thread.
//!   - serde `Serialize`/`Deserialize` derives: turn Rust structs into the
//!     request JSON and the response JSON into Rust structs, all type checked.
//!   - lifetimes on the request struct (`MessagesRequest<'a>`): the request
//!     borrows the prompt strings instead of cloning them.
//!   - `enum` sum type for errors with `thiserror`: one exhaustive `EngineError`
//!     covering every failure mode.
//!   - `?` operator and `#[from]`: propagate errors upward, auto converting
//!     `reqwest::Error` into `EngineError::Http`.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{AudienceInput, Compression, CoreNeed, ProductInput, Promise};
use crate::prompt;

/// Environment variable that holds the secret. Never hardcode the key.
const API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
/// The Messages API endpoint.
const API_URL: &str = "https://api.anthropic.com/v1/messages";
/// Required API version header value.
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// The model used for compression. Anthropic's most capable Opus tier model.
const DEFAULT_MODEL: &str = "claude-opus-4-8";
/// The copy is short, so a small output ceiling is plenty.
const MAX_TOKENS: u32 = 1024;

/// Every way a compression request can fail.
///
/// `enum` sum type: an `EngineError` is exactly one of these. This is the
/// "exhaustive error enum covering every failure mode" the design calls for.
#[derive(Debug, Error)]
pub enum EngineError {
    /// The `ANTHROPIC_API_KEY` environment variable was not set.
    #[error("the {API_KEY_ENV} environment variable is not set")]
    MissingApiKey,

    /// A transport level failure (DNS, TLS, connection, body read).
    ///
    /// `#[from]` generates `From<reqwest::Error>`, so the `?` operator can turn
    /// a `reqwest::Error` into this variant automatically.
    #[error("network error talking to the API: {0}")]
    Http(#[from] reqwest::Error),

    /// The API responded, but with a non success HTTP status.
    #[error("the API returned status {status}: {body}")]
    BadStatus { status: u16, body: String },

    /// The response contained no text content block to read.
    #[error("the API response contained no text content")]
    NoContent,

    /// The response, or the JSON the model produced, did not parse.
    #[error("could not parse the model output as the expected JSON: {0}")]
    MalformedJson(String),
}

// --- Request payload types (serialized to JSON) ---------------------------

/// The request body. Borrows the prompt strings via the `'a` lifetime.
///
/// `#[derive(Serialize)]` lets serde turn this into the JSON the API expects.
#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: Vec<RequestMessage<'a>>,
}

/// A single chat message in the request.
#[derive(Serialize)]
struct RequestMessage<'a> {
    role: &'a str,
    content: &'a str,
}

// --- Response payload types (deserialized from JSON) ----------------------

/// The top level response envelope. We only care about `content`.
#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

/// One content block in the response. The API tags each block with a `type`;
/// we map that JSON key to `kind` and only read `text` blocks.
#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

/// The shape of the JSON the model is instructed to emit. This is the contract
/// from `prompt::system_prompt`, expressed as a Rust type so serde validates it.
#[derive(Deserialize)]
struct RawCompression {
    core_need: String,
    promise: String,
    meta_primary_text: String,
    google_headlines: Vec<String>,
    landing_hero: String,
}

/// Run one compression: build the prompts, call the API, parse the answer.
///
/// `async fn`: this returns a future. Callers drive it with `.await` on a tokio
/// runtime. The function takes ownership of the typed inputs so it can be moved
/// into a spawned task without lifetime entanglements.
pub async fn compress(
    product: ProductInput,
    audience: AudienceInput,
) -> Result<Compression, EngineError> {
    // Read the secret from the environment. `map_err` converts the std
    // `VarError` into our domain specific `MissingApiKey`.
    let api_key = std::env::var(API_KEY_ENV).map_err(|_| EngineError::MissingApiKey)?;

    // Pure prompt construction. No IO, fully testable.
    let system = prompt::system_prompt();
    let user = prompt::user_prompt(&product, &audience);

    let request = MessagesRequest {
        model: DEFAULT_MODEL,
        max_tokens: MAX_TOKENS,
        system: &system,
        messages: vec![RequestMessage {
            role: "user",
            content: &user,
        }],
    };

    // Send the request. Each `?` short circuits on a transport error, which the
    // `#[from]` impl turns into `EngineError::Http`.
    let client = reqwest::Client::new();
    let response = client
        .post(API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await?;

    // Distinguish a non success status from a transport failure: read the body
    // for context and return a typed `BadStatus`.
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(EngineError::BadStatus {
            status: status.as_u16(),
            body,
        });
    }

    // Read the raw body, then parse the envelope ourselves so JSON failures map
    // to `MalformedJson` rather than a generic transport error.
    let raw_body = response.text().await?;
    let envelope: MessagesResponse =
        serde_json::from_str(&raw_body).map_err(|e| EngineError::MalformedJson(e.to_string()))?;

    // Find the first text block. `into_iter` consumes the vec so we can move the
    // `String` out without cloning. `?`-style propagation via `ok_or`.
    let text = envelope
        .content
        .into_iter()
        .find(|block| block.kind == "text")
        .and_then(|block| block.text)
        .ok_or(EngineError::NoContent)?;

    // Defensive: if the model wrapped the JSON in prose or code fences, slice
    // out the object before parsing.
    let json_slice = extract_json_object(&text)
        .ok_or_else(|| EngineError::MalformedJson("no JSON object found in the response".into()))?;

    let raw: RawCompression =
        serde_json::from_str(json_slice).map_err(|e| EngineError::MalformedJson(e.to_string()))?;

    build_compression(raw)
}

/// Slice the substring from the first `{` to the last `}` inclusive.
///
/// Returns `None` if there is no plausible object. This keeps the engine robust
/// against a model that occasionally adds a sentence or a code fence around the
/// JSON, without resorting to a full tolerant parser.
fn extract_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end > start {
        Some(&text[start..=end])
    } else {
        None
    }
}

/// Validate the parsed JSON and lift it into domain types.
///
/// Empty required fields are treated as a malformed result, because an empty
/// core need or promise is not a usable compression.
fn build_compression(raw: RawCompression) -> Result<Compression, EngineError> {
    let core_need = non_empty(raw.core_need, "core_need")?;
    let promise = non_empty(raw.promise, "promise")?;
    let meta_primary_text = non_empty(raw.meta_primary_text, "meta_primary_text")?;
    let landing_hero = non_empty(raw.landing_hero, "landing_hero")?;

    // Keep only non empty headlines; require at least one.
    let google_headlines: Vec<String> = raw
        .google_headlines
        .into_iter()
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
        .collect();
    if google_headlines.is_empty() {
        return Err(EngineError::MalformedJson(
            "google_headlines was empty".into(),
        ));
    }

    Ok(Compression {
        core_need: CoreNeed::new(core_need),
        promise: Promise::new(promise),
        meta_primary_text,
        google_headlines,
        landing_hero,
    })
}

/// Trim a field and reject it if nothing remains.
fn non_empty(value: String, field: &str) -> Result<String, EngineError> {
    let trimmed = value.trim().to_owned();
    if trimmed.is_empty() {
        Err(EngineError::MalformedJson(format!("{field} was empty")))
    } else {
        Ok(trimmed)
    }
}
