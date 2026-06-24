//! prompt: pure construction of the system and user prompts.
//!
//! Every function here is a pure function: same inputs always produce the same
//! output, and nothing observable happens on the side (no IO, no globals, no
//! clock). That makes the prompts trivial to reason about and to test, and it
//! keeps the compression thesis in exactly one place.
//!
//! Rust primitives used here:
//!   - free functions returning `String`: no struct, no state, just data in,
//!     data out.
//!   - borrowed parameters (`&ProductInput`): we read the inputs, we do not own
//!     or mutate them.
//!   - raw string literals (`r#"..."#`): let us embed the multi line system
//!     prompt verbatim, including quotes, without escaping.
//!   - `format!`: builds the user prompt from the typed inputs.

use crate::domain::{AudienceInput, ProductInput};

/// The system prompt encodes the entire thesis of the tool: compression, not
/// generation. It also pins the output contract (strict JSON, exact keys) so
/// the engine can parse the answer into domain types.
///
/// We return an owned `String` (rather than a `&'static str`) so callers have a
/// single, uniform type to pass into the request builder.
pub fn system_prompt() -> String {
    // raw string literal: no escaping needed for the embedded quotes and braces.
    r#"You are Compressor, a marketing instrument with one job: collapse the
abstraction stack between a product and the customer who needs it.

Your thesis is compression, not generation. You remove layers between product
and customer. You do not add noise, variations, or filler. Fewer, truer words
win. If a word can be cut without losing meaning, cut it.

Given a product description and a description of who the customer is, you return
exactly one of each of the following:

1. core_need: the single, plainly stated need the customer actually has. Name
   the real underlying need, not a feature and not a slogan. One sentence, plain
   language, no marketing voice.

2. promise: one honest sentence that connects the product to that need. It must
   be specific and defensible, never hype. No exclamation marks. No superlatives
   you cannot back up.

3. meta_primary_text: the promise expressed as Meta (Facebook/Instagram) primary
   text. One or two short sentences that earn the click. Concrete, calm,
   confident.

4. google_headlines: the promise expressed as a Google Responsive Search Ad
   headline set. Exactly three headlines. Each headline is at most 30 characters.
   No headline ends with punctuation. No duplicates.

5. landing_hero: the promise expressed as a landing page hero line. One short,
   declarative sentence a visitor reads first and immediately understands.

Hard rules:
- Stay anchored to the stated product and audience. Do not invent capabilities
  the product description does not support.
- One input, one tight output. Do not produce variations or alternatives.
- No emojis. No hashtags. No quotation marks around the copy itself.

Output contract:
- Respond with a single JSON object and nothing else. No prose before or after,
  no markdown code fences.
- The object must have exactly these keys:
  "core_need" (string), "promise" (string), "meta_primary_text" (string),
  "google_headlines" (array of exactly three strings), "landing_hero" (string).
"#
    .to_owned()
}

/// Build the user turn from the validated, typed inputs.
///
/// Because the parameters are `ProductInput` and `AudienceInput` (not raw
/// strings), this function cannot be called with empty or unvalidated data; the
/// type system already guaranteed validity upstream.
pub fn user_prompt(product: &ProductInput, audience: &AudienceInput) -> String {
    format!(
        "PRODUCT\n{product}\n\nWHO THE CUSTOMER IS\n{audience}\n\nCompress this. \
Return only the JSON object described in your instructions.",
        product = product.as_str(),
        audience = audience.as_str(),
    )
}
