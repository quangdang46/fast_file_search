//! Cursor-aware @-mention candidate search (Phase A).
//!
//! This module is Phase A of the unified `@`-mention candidate search plan.
//! It exposes a single high-level entry point, [`MentionResolver`], that
//! host apps (TUIs, editors, MCP clients) can plug into a popup layer
//! without touching the core [`crate::FilePicker`] pipeline.
//!
//! ## What it does
//!
//! Given a buffer and a cursor offset, [`MentionResolver::search`] detects
//! the `@`-token under the cursor and returns a ranked list of
//! [`MentionCandidate`]s (up to `MentionOptions::max_candidates`) together
//! with the parsed [`MentionTrigger`] (start offset, query text, prefix
//! kind).
//!
//! ## Supported kinds
//!
//! [`MentionKind`] is intentionally a small enum:
//!
//! - [`MentionKind::File`] — a path returned by the existing
//!   `FilePicker::fuzzy_search` pipeline (SIMD `frizbee` + frecency boost).
//! - [`MentionKind::Directory`] — a path returned by
//!   `FilePicker::fuzzy_search_directories`.
//! - [`MentionKind::External`] — a tagged escape hatch (`External(&'static str)`)
//!   that host providers fill in during a later phase. `ffs-core` does not
//!   interpret the identifier.
//!
//! ## Reuse, not reimplementation
//!
//! Phase A deliberately reuses the production ranking path so mention
//! results stay consistent with the rest of the crate:
//!
//! - File and directory candidates come from the same
//!   [`crate::FilePicker::fuzzy_search`] /
//!   [`crate::FilePicker::fuzzy_search_directories`] calls the CLI uses.
//! - Frecency boosting is read from [`crate::SharedFrecency`]; no
//!   separate `frecency.jsonl` is written.
//! - Trigger detection ([`detect_trigger`]) is cursor- and Unicode-safe
//!   and is independently unit-tested in `trigger.rs`.
//!
//! ## Cross-references
//!
//! - Full research + rollout plan: `docs/MENTION_SYSTEM_PLAN.md`.
//! - `FilePicker` ranking: [`crate::file_picker::FilePicker`].
//! - Frecency store: [`crate::frecency::FrecencyTracker`] /
//!   [`crate::SharedFrecency`].
//!
//! ## Example
//!
//! ```no_run
//! use ffs_search::FilePicker;
//! use ffs_search::mention::{MentionOptions, MentionResolver};
//!
//! let picker: &FilePicker = todo!("host app owns the FilePicker");
//! let resolver = MentionResolver::new(picker)
//!     .with_options(MentionOptions { max_candidates: 10, ..Default::default() });
//! let result = resolver.search("review @src/co", 14);
//! if let Some(trigger) = result.trigger { /* show popup */ }
//! ```

mod resolver;
mod trigger;

/// Phase D: extensible @-mention provider protocol. See module docs.
pub mod provider;

pub use provider::{
    ExternalMentionCandidate, ExternalResolveResult, MentionProvider, ProviderError,
    ProviderRegistry,
};
pub use resolver::{MentionCandidate, MentionKind, MentionOptions, MentionResolver, MentionResult};
pub use trigger::{MentionTrigger, detect_trigger};
