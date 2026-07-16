//! High-performance grep engine for live content search.
//!
//! Layout inspired by upstream fff `grep/` split (b4590ca):
//! - [`classify`] — definition/import line heuristics
//! - [`utils`] — pure helpers (regex detect, newline escapes, parse/strip)
//! - [`fuzzy_grep`] — neo_frizbee fuzzy path
//! - [`grep`] — plain/regex/multi matchers, sinks, orchestration

mod classify;
mod fuzzy_grep;
mod utils;

#[allow(clippy::module_inception)]
mod grep;
pub use classify::*;
pub use grep::*;
pub use utils::{
    has_regex_metacharacters, has_unescaped_newline_escape, parse_grep_query,
    replace_unescaped_newline_escapes,
};

#[cfg(test)]
mod grep_tests;
