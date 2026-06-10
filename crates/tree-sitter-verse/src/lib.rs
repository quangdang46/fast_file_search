//! Tree-sitter grammar for [Verse](https://dev.epicgames.com/documentation/en-us/uefn/verse-language-reference).
//!
//! Grammar source: <https://github.com/taku25/tree-sitter-verse> (MIT).

mod bindings;

pub use bindings::LANGUAGE;

/// Shorthand for [`LANGUAGE`] converted to a [`tree_sitter::Language`].
#[must_use]
pub fn language() -> tree_sitter::Language {
    LANGUAGE.into()
}
