//! Symbol indexing and AST-driven outline extraction for scry.
//!
//! Three primary services exposed by this crate:
//!
//! * [`bloom`] — a per-file probabilistic membership filter for fast
//!   "does file X contain symbol Y?" pre-checks before tree-sitter parsing.
//! * [`symbol_index`] — a concurrent map from `symbol_name -> Vec<Location>`
//!   built by walking files in parallel with `ignore` + `rayon`.
//! * [`outline`] — tree-sitter driven structural extraction (functions,
//!   classes, traits, modules, …) usable for outline view modes.
//!
//! All public types live in [`types`]. Language detection and tree-sitter
//! grammar lookup live in [`lang`] and [`treesitter`].

pub mod artifact;
pub mod batch;
pub mod bloom;
pub mod detection;
pub mod lang;
pub mod outline;
pub mod outline_cache;
pub mod symbol_index;
pub mod treesitter;
pub mod types;

pub use bloom::{BloomFilter, BloomFilterCache};
pub use lang::{detect_file_type, package_root};
pub use outline::{get_outline_entries, outline_language};
pub use outline_cache::OutlineCache;
pub use symbol_index::{SymbolIndex, SymbolLocation};
pub use treesitter::{
    definition_weight, extract_definition_name, extract_impl_trait, extract_impl_type,
    extract_implemented_interfaces, DEFINITION_KINDS,
};
pub use types::{
    estimate_tokens, is_test_file, truncate_str, FileType, Lang, Match, OutlineEntry, OutlineKind,
    QueryType, SearchResult, ViewMode,
};
