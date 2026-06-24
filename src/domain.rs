//! domain: the typed vocabulary of Compressor.
//!
//! This module exists to make illegal states unrepresentable. Instead of
//! passing raw `String` values around (where "empty product description" is a
//! perfectly valid `String`), we wrap meaningful values in newtypes that can
//! only be constructed through a validating constructor. Once you hold a
//! `ProductInput`, you know it is non empty, trimmed, and ready to use.
//!
//! Rust primitives used here:
//!   - newtype pattern: a tuple struct wrapping a single field, e.g.
//!     `struct ProductInput(String)`. Zero runtime cost, full type safety.
//!   - enum sum type: `Placement` and `PlacementContent` model closed sets of
//!     possibilities, so a `match` over them is exhaustively checked.
//!   - `Result<T, E>`: validating constructors return `Result` so callers must
//!     handle the failure case rather than silently accepting bad data.
//!   - `thiserror::Error`: derives `std::error::Error` + `Display` for our
//!     `DomainError` enum with one annotation per variant.
//!   - lifetimes: `PlacementContent<'a>` borrows from a `Compression` rather
//!     than cloning, so rendering the result allocates nothing.

use thiserror::Error;

/// Every way a domain value can fail to be constructed.
///
/// `enum` sum type: a `DomainError` is exactly one of these variants. The
/// `#[error("...")]` attribute (from `thiserror`) supplies the `Display` text.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    #[error("the product description is empty; describe what the product is")]
    EmptyProduct,
    #[error("the audience description is empty; describe who the customer is")]
    EmptyAudience,
}

/// A validated product description supplied by the user.
///
/// newtype pattern: the inner `String` is private, so the only way to obtain a
/// `ProductInput` is through [`ProductInput::new`], which guarantees the value
/// is non empty and trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductInput(String);

impl ProductInput {
    /// Construct from arbitrary user text, trimming surrounding whitespace.
    ///
    /// Returns `Err(DomainError::EmptyProduct)` when nothing meaningful remains.
    pub fn new(raw: &str) -> Result<Self, DomainError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DomainError::EmptyProduct);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// Borrow the underlying text. `&str` here avoids cloning the `String`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A validated description of who the customer is believed to be.
///
/// Same newtype discipline as [`ProductInput`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudienceInput(String);

impl AudienceInput {
    pub fn new(raw: &str) -> Result<Self, DomainError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(DomainError::EmptyAudience);
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The single, plainly stated core need the engine distilled.
///
/// This is an output side newtype: it carries the model's answer through the
/// program with a name that documents its meaning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreNeed(String);

impl CoreNeed {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The one honest sentence connecting product to need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promise(String);

impl Promise {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The three ad placements the promise is expressed for.
///
/// enum sum type: a closed set. Adding a fourth placement here forces the
/// compiler to flag every `match` that does not yet handle it, which is exactly
/// the safety we want.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    MetaPrimaryText,
    GoogleHeadlines,
    LandingHero,
}

impl Placement {
    /// Human readable name shown as the card heading.
    ///
    /// `self` (by value) is fine because `Placement` is `Copy`.
    pub fn title(self) -> &'static str {
        match self {
            Placement::MetaPrimaryText => "Meta primary text",
            Placement::GoogleHeadlines => "Google headlines",
            Placement::LandingHero => "Landing page hero",
        }
    }

    /// One line of context about what this placement is for.
    pub fn subtitle(self) -> &'static str {
        match self {
            Placement::MetaPrimaryText => "Feed copy that earns the click",
            Placement::GoogleHeadlines => "Search headlines, 30 characters each",
            Placement::LandingHero => "The first promise above the fold",
        }
    }
}

/// The content rendered for a placement.
///
/// enum sum type with a lifetime: a placement is either a single string or a
/// set of headlines, and we borrow from the owning [`Compression`] rather than
/// copy. The `'a` lifetime ties the borrowed data to the `Compression` it came
/// from, so the borrow checker prevents it from outliving its source.
pub enum PlacementContent<'a> {
    Single(&'a str),
    Set(&'a [String]),
}

/// The complete compressed result: one need, one promise, three placements.
///
/// This is the single value the engine produces and the UI renders. Every
/// field is a typed domain value, not a loose `String`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compression {
    pub core_need: CoreNeed,
    pub promise: Promise,
    pub meta_primary_text: String,
    pub google_headlines: Vec<String>,
    pub landing_hero: String,
}

impl Compression {
    /// View the three placements as `(Placement, PlacementContent)` pairs.
    ///
    /// Returning a fixed size array `[_; 3]` documents at the type level that
    /// there are always exactly three placements. The UI iterates this to
    /// render each placement uniformly. The borrows (`&self`) mean no
    /// allocation happens here.
    pub fn placements(&self) -> [(Placement, PlacementContent<'_>); 3] {
        [
            (
                Placement::MetaPrimaryText,
                PlacementContent::Single(&self.meta_primary_text),
            ),
            (
                Placement::GoogleHeadlines,
                PlacementContent::Set(&self.google_headlines),
            ),
            (
                Placement::LandingHero,
                PlacementContent::Single(&self.landing_hero),
            ),
        ]
    }
}
