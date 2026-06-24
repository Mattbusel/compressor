# prompt

Pure construction of the system and user prompts. This module is where the
compression thesis lives, and it is deliberately the simplest module in the
project: data in, data out, nothing else.

Source: [`src/prompt.rs`](../src/prompt.rs)

## Pure functions

Both functions here are pure: the same inputs always produce the same output,
and calling them has no observable side effect. There is no IO, no global state,
no clock, no randomness. That property matters for two reasons.

First, the prompts become trivial to reason about and to test. You can call
`system_prompt()` and assert on the exact string, or call `user_prompt(...)`
with known inputs and check the result, with no setup or mocking.

Second, it keeps the thesis in exactly one place. The entire opinion of the tool
(compress, do not generate; name the real need; one promise; the exact output
contract) is the text returned by `system_prompt`. If you want to change what
Compressor believes, you change one function.

## The system prompt

`system_prompt() -> String` returns the instrument's whole job description. It
is written as a raw string literal:

```rust
r#"You are Compressor, a marketing instrument with one job: ..."#
```

A raw string literal (`r#"..."#`) lets the multi line prompt be embedded
verbatim, including quotes and braces, with no escaping. The prompt encodes:

- **The thesis.** Compression, not generation. Remove layers, do not add noise.
  Fewer, truer words win.
- **The five required outputs.** core need, promise, and the promise expressed
  for Meta primary text, a Google headline set (exactly three headlines, each at
  most 30 characters), and a landing page hero.
- **Hard rules.** Stay anchored to the stated product and audience. One input,
  one tight output, no variations. No emojis, hashtags, or wrapping quotes.
- **The output contract.** Respond with a single JSON object and nothing else,
  with exactly these keys: `core_need`, `promise`, `meta_primary_text`,
  `google_headlines`, `landing_hero`.

That contract is the seam between this module and `engine`: the engine's
`RawCompression` type is the Rust mirror of those exact keys, so serde can parse
the model's answer directly into it.

We return an owned `String` rather than a `&'static str` so callers have one
uniform type to pass into the request builder.

## The user prompt

`user_prompt(product: &ProductInput, audience: &AudienceInput) -> String` builds
the user turn from the typed inputs:

```rust
format!(
    "PRODUCT\n{product}\n\nWHO THE CUSTOMER IS\n{audience}\n\nCompress this. ...",
    product = product.as_str(),
    audience = audience.as_str(),
)
```

The signature is the important part. Because the parameters are
`&ProductInput` and `&AudienceInput`, not raw `&str`, this function cannot be
called with empty or unvalidated data. The type system guaranteed validity
upstream in `domain`, so there is nothing to re check here. The parameters are
borrowed (`&`), because the function only reads them.

## Why the JSON demand lives in the prompt

The design puts the strict JSON contract in the system prompt and lets the
engine parse the result, rather than constraining the response format at the API
level. This keeps the request payload simple and the contract human readable in
one place. It is also why `EngineError::MalformedJson` exists: parsing can fail,
so the failure has a typed home. The README's "what I would build next" section
notes the natural hardening step, which is to move this contract into the API's
schema enforced output so malformed JSON becomes impossible rather than handled.

## Rust primitives used in this module

- **free functions returning `String`**: no struct, no state, just data.
- **borrowed parameters** (`&ProductInput`, `&AudienceInput`): read, do not own.
- **raw string literal** (`r#"..."#`): embeds the multi line prompt verbatim.
- **`format!` macro**: builds the user prompt from the typed inputs.
