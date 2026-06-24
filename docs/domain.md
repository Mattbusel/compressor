# domain

The typed vocabulary of Compressor. This module exists for one reason: to make
illegal states unrepresentable. If a value has a name in the problem (a product
description, a core need, a placement), it has a type here.

Source: [`src/domain.rs`](../src/domain.rs)

## Why a vocabulary module

Most of the bugs in a small tool like this are not algorithmic, they are about
data that is in the wrong shape: an empty string where a description was
expected, a result with three placements in one place and two in another, a
"need" and a "promise" accidentally swapped because both are `String`. The
`domain` module removes those bugs by construction. Once the rest of the program
holds a `ProductInput`, it does not need to defensively check whether it is
empty, because the only way to make one is through a validating constructor.

## The newtype pattern

A newtype is a tuple struct wrapping a single value:

```rust
pub struct ProductInput(String);
```

The inner `String` is private. There is no way to build a `ProductInput` except
through `ProductInput::new`, which trims whitespace and rejects empty input:

```rust
pub fn new(raw: &str) -> Result<Self, DomainError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(DomainError::EmptyProduct);
    }
    Ok(Self(trimmed.to_owned()))
}
```

This costs nothing at runtime (the wrapper compiles away) and buys two things:
the value is always valid, and the type name documents intent at every call
site. A function taking `&ProductInput` cannot be handed an `AudienceInput` by
mistake, even though both wrap a `String`.

The four newtypes:

- `ProductInput` and `AudienceInput` are **input side**: validated from raw user
  text, guaranteed non empty and trimmed.
- `CoreNeed` and `Promise` are **output side**: they carry the model's answer
  through the program with names that document meaning. Their constructors take
  an owned `String` (the engine has already validated non emptiness before
  building them).

Each exposes `as_str(&self) -> &str`, borrowing the inner text without cloning.

## Enum sum types for closed sets

A `Placement` is exactly one of three things:

```rust
pub enum Placement {
    MetaPrimaryText,
    GoogleHeadlines,
    LandingHero,
}
```

This is a sum type: a value is one variant at a time. The payoff is exhaustive
matching. Every `match` over `Placement` must handle all three variants, so if a
fourth placement is ever added, the compiler points at every place that needs
updating. `Placement` is `Copy` (it is just a tag), so its methods take `self`
by value:

```rust
pub fn title(self) -> &'static str { ... }
pub fn subtitle(self) -> &'static str { ... }
```

These return `&'static str` (string data baked into the binary) for the card
heading and the one line of context the UI shows under it.

## Lifetimes: borrowing the content

The content of a placement is either a single string or a set of headlines:

```rust
pub enum PlacementContent<'a> {
    Single(&'a str),
    Set(&'a [String]),
}
```

The `'a` is a lifetime parameter. It ties the borrowed data to the
`Compression` it came from, so the borrow checker guarantees a
`PlacementContent` can never outlive the result it points into. This lets the UI
iterate the placements with zero allocation, instead of cloning strings just to
display them.

## The result struct

`Compression` is the single value the engine produces and the UI renders:

```rust
pub struct Compression {
    pub core_need: CoreNeed,
    pub promise: Promise,
    pub meta_primary_text: String,
    pub google_headlines: Vec<String>,
    pub landing_hero: String,
}
```

It pairs typed values (`CoreNeed`, `Promise`) with the raw placement strings.
Its one method exposes the placements as a fixed size array:

```rust
pub fn placements(&self) -> [(Placement, PlacementContent<'_>); 3] { ... }
```

Returning `[_; 3]` (not a `Vec`) encodes at the type level that there are always
exactly three placements. The UI loops over this array to render every placement
the same way, which is why adding or reordering placements is a one place change.

## Errors

`DomainError` is a small enum of the two validation failures, deriving
`thiserror::Error` so each variant carries its own user facing message:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    #[error("the product description is empty; ...")]
    EmptyProduct,
    #[error("the audience description is empty; ...")]
    EmptyAudience,
}
```

These surface directly in the app's error card when the user tries to compress
with an empty field.

## Rust primitives used in this module

- **newtype pattern**: `ProductInput`, `AudienceInput`, `CoreNeed`, `Promise`.
- **enum sum type**: `Placement`, `PlacementContent`, `DomainError`.
- **lifetimes**: `PlacementContent<'a>` borrows from `Compression`.
- **`Result<T, E>`**: validating constructors return `Result`.
- **`thiserror::Error` derive**: generates `Display` + `Error` for `DomainError`.
- **fixed size array type** (`[_; 3]`): encodes the count of placements.
