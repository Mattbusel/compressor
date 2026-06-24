//! engine::llm: the async Anthropic Messages API client.
//!
//! This is the model backed engine. It builds the request from the pure
//! `prompt` module, calls the API, and parses the model's JSON answer into the
//! domain types. It is wrapped behind the shared `CompressionEngine` trait so
//! the UI treats it identically to the deterministic engine.
//!
//! Rust primitives used here:
//!   - `async fn` + `.await`: the network call yields to the runtime, never
//!     blocking the UI thread.
//!   - serde `Serialize` / `Deserialize`: typed request and response payloads.
//!   - lifetimes (`MessagesRequest<'a>`): borrow the prompt strings, no clone.
//!   - trait impl (`impl CompressionEngine for LlmEngine`).

use serde::{Deserialize, Serialize};

use crate::domain::{AudienceInput, Compression, CoreNeed, ProductInput, Promise};
use crate::prompt;

use super::{CompressionEngine, EngineError, EngineOutput};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_MODEL: &str = "claude-opus-4-8";
const MAX_TOKENS: u32 = 1024;

/// The model backed engine. A unit struct: it holds no state.
pub struct LlmEngine;

impl CompressionEngine for LlmEngine {
    async fn compress(
        &self,
        product: ProductInput,
        audience: AudienceInput,
    ) -> Result<EngineOutput, EngineError> {
        let compression = compress_via_api(&product, &audience).await?;
        // The model leaves no deterministic trace to show.
        Ok(EngineOutput {
            compression,
            trace: None,
        })
    }
}

// --- Request payload types (serialized to JSON) ---------------------------

#[derive(Serialize)]
struct MessagesRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    system: &'a str,
    messages: Vec<RequestMessage<'a>>,
}

#[derive(Serialize)]
struct RequestMessage<'a> {
    role: &'a str,
    content: &'a str,
}

// --- Response payload types (deserialized from JSON) ----------------------

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

/// The Rust mirror of the prompt's output contract. serde validates the keys.
#[derive(Deserialize)]
struct RawCompression {
    core_need: String,
    promise: String,
    meta_primary_text: String,
    google_headlines: Vec<String>,
    landing_hero: String,
}

/// The actual API round trip, kept as a free function for readability.
async fn compress_via_api(
    product: &ProductInput,
    audience: &AudienceInput,
) -> Result<Compression, EngineError> {
    let api_key = std::env::var(super::API_KEY_ENV).map_err(|_| EngineError::MissingApiKey)?;

    let system = prompt::system_prompt();
    let user = prompt::user_prompt(product, audience);

    let request = MessagesRequest {
        model: DEFAULT_MODEL,
        max_tokens: MAX_TOKENS,
        system: &system,
        messages: vec![RequestMessage {
            role: "user",
            content: &user,
        }],
    };

    let client = reqwest::Client::new();
    let response = client
        .post(API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(EngineError::BadStatus {
            status: status.as_u16(),
            body,
        });
    }

    let raw_body = response.text().await?;
    let envelope: MessagesResponse =
        serde_json::from_str(&raw_body).map_err(|e| EngineError::MalformedJson(e.to_string()))?;

    let text = envelope
        .content
        .into_iter()
        .find(|block| block.kind == "text")
        .and_then(|block| block.text)
        .ok_or(EngineError::NoContent)?;

    let json_slice = extract_json_object(&text)
        .ok_or_else(|| EngineError::MalformedJson("no JSON object found in the response".into()))?;

    let raw: RawCompression =
        serde_json::from_str(json_slice).map_err(|e| EngineError::MalformedJson(e.to_string()))?;

    build_compression(raw)
}

/// Slice from the first `{` to the last `}`, tolerating prose or fences.
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
fn build_compression(raw: RawCompression) -> Result<Compression, EngineError> {
    let core_need = non_empty(raw.core_need, "core_need")?;
    let promise = non_empty(raw.promise, "promise")?;
    let meta_primary_text = non_empty(raw.meta_primary_text, "meta_primary_text")?;
    let landing_hero = non_empty(raw.landing_hero, "landing_hero")?;

    let google_headlines: Vec<String> = raw
        .google_headlines
        .into_iter()
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
        .collect();
    if google_headlines.is_empty() {
        return Err(EngineError::MalformedJson("google_headlines was empty".into()));
    }

    Ok(Compression {
        core_need: CoreNeed::new(core_need),
        promise: Promise::new(promise),
        meta_primary_text,
        google_headlines,
        landing_hero,
    })
}

fn non_empty(value: String, field: &str) -> Result<String, EngineError> {
    let trimmed = value.trim().to_owned();
    if trimmed.is_empty() {
        Err(EngineError::MalformedJson(format!("{field} was empty")))
    } else {
        Ok(trimmed)
    }
}
