//! Top-level engine — owns shared caches, runs the scanner, and dispatches
//! queries to the right backend.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use fff_budget::{
    apply_preserving_footer, smart_truncate, AggressiveFilter, BudgetSplit, FilterLevel,
    FilterStrategy, MinimalFilter, NoFilter, TruncationOutcome,
};
use fff_symbol::outline_cache::OutlineCache;
use fff_symbol::symbol_index::SymbolLocation;
use fff_symbol::types::QueryType;
use fff_symbol::{BloomFilterCache, SymbolIndex};

use crate::classify::{classify_query, ClassifiedQuery};
use crate::memory::{MemoryGuard, RepoSize};
use crate::scanner::{ScanReport, UnifiedScanner};

/// Configuration knobs for engine behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    pub max_results: usize,
    pub max_bytes_per_result: usize,
    pub filter_level: FilterLevel,
    pub total_token_budget: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_results: 50,
            max_bytes_per_result: 64 * 1024,
            filter_level: FilterLevel::Minimal,
            total_token_budget: 25_000,
        }
    }
}

/// Bundle of shared caches managed by the engine.
#[derive(Clone)]
pub struct EngineHandles {
    pub bloom: Arc<BloomFilterCache>,
    pub symbols: Arc<SymbolIndex>,
    pub outlines: Arc<OutlineCache>,
}

/// The unified search engine.
pub struct Engine {
    pub config: EngineConfig,
    pub handles: EngineHandles,
    pub guard: MemoryGuard,
    pub scanner: UnifiedScanner,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new(EngineConfig::default())
    }
}

impl Engine {
    /// Build a new engine with empty caches and the given config.
    #[must_use]
    pub fn new(config: EngineConfig) -> Self {
        let scanner = UnifiedScanner::new();
        let handles = EngineHandles {
            bloom: scanner.bloom.clone(),
            symbols: scanner.symbols.clone(),
            outlines: scanner.outlines.clone(),
        };
        let guard = MemoryGuard::new(
            scanner.bloom.clone(),
            scanner.symbols.clone(),
            scanner.outlines.clone(),
        );
        Self {
            config,
            handles,
            guard,
            scanner,
        }
    }

    /// Run the unified scanner over `root`, then apply the matching budget rules.
    pub fn index(&self, root: &Path) -> ScanReport {
        let report = self.scanner.scan(root);
        self.guard
            .apply(RepoSize::from_file_count(report.files_indexed));
        report
    }

    /// Classify and dispatch a free-form query to the best-matching backend.
    pub fn dispatch(&self, raw: &str, cwd: &Path) -> DispatchResult {
        let classified = classify_query(raw, cwd);
        match &classified.query {
            QueryType::Symbol(name) => DispatchResult::Symbol {
                classified: classified.clone(),
                hits: self.handles.symbols.lookup_exact(name),
            },
            QueryType::SymbolGlob(prefix) => DispatchResult::SymbolGlob {
                classified: classified.clone(),
                hits: self
                    .handles
                    .symbols
                    .lookup_prefix(prefix.trim_end_matches('*')),
            },
            QueryType::Concept(_) | QueryType::Fallthrough(_) => DispatchResult::ContentFallback {
                classified: classified.clone(),
            },
            QueryType::FilePath(p) | QueryType::FilePathLine(p, _) => DispatchResult::FilePath {
                classified: classified.clone(),
                path: p.clone(),
            },
            QueryType::Glob(pattern) => DispatchResult::Glob {
                classified: classified.clone(),
                pattern: pattern.clone(),
            },
        }
    }

    /// Read a file with token-budget-aware filtering and truncation.
    pub fn read(&self, path: &Path) -> ReadResult {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                return ReadResult {
                    path: path.to_path_buf(),
                    body: format!("[error reading file: {e}]"),
                    outcome: TruncationOutcome {
                        kept_lines: 0,
                        dropped_lines: 0,
                        kept_bytes: 0,
                        footer_bytes: 0,
                    },
                }
            }
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();

        let filter: Box<dyn FilterStrategy> = match self.config.filter_level {
            FilterLevel::None => Box::new(NoFilter),
            FilterLevel::Minimal => Box::new(MinimalFilter),
            FilterLevel::Aggressive => Box::new(AggressiveFilter),
        };
        let filtered = filter.apply(&text);

        let split = BudgetSplit::default_for(self.config.total_token_budget);
        let body_budget_bytes = (split.body * 4) as usize;

        let max_bytes = body_budget_bytes.min(self.config.max_bytes_per_result);
        let mut buf = String::new();
        let footer = "[truncated to budget]\n";
        let outcome = if filtered.len() <= max_bytes {
            let (out, oc) = smart_truncate(&filtered, max_bytes);
            buf.push_str(&out);
            oc
        } else {
            apply_preserving_footer(&mut buf, max_bytes, footer, |target, budget| {
                let take = filtered.len().min(budget);
                target.push_str(&filtered[..take]);
                take
            })
        };

        ReadResult {
            path: path.to_path_buf(),
            body: buf,
            outcome,
        }
    }
}

/// Result of [`Engine::dispatch`] — points to the right backend.
#[derive(Debug, Clone)]
pub enum DispatchResult {
    Symbol {
        classified: ClassifiedQuery,
        hits: Vec<SymbolLocation>,
    },
    SymbolGlob {
        classified: ClassifiedQuery,
        hits: Vec<(String, SymbolLocation)>,
    },
    Glob {
        classified: ClassifiedQuery,
        pattern: String,
    },
    FilePath {
        classified: ClassifiedQuery,
        path: PathBuf,
    },
    ContentFallback {
        classified: ClassifiedQuery,
    },
}

/// Output of [`Engine::read`].
#[derive(Debug, Clone)]
pub struct ReadResult {
    pub path: PathBuf,
    pub body: String,
    pub outcome: TruncationOutcome,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_indexes_and_dispatches_symbol() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn alpha() {}\nfn beta() {}").unwrap();
        let engine = Engine::default();
        let report = engine.index(dir.path());
        assert!(report.files_indexed >= 1);

        let result = engine.dispatch("alpha", dir.path());
        match result {
            DispatchResult::Symbol { hits, .. } => {
                assert_eq!(hits.len(), 1);
                assert!(hits[0].path.ends_with("a.rs"));
            }
            other => panic!("expected Symbol dispatch, got {other:?}"),
        }
    }

    #[test]
    fn engine_reads_with_budget() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.rs");
        let content: String = (0..1000).map(|i| format!("// line {i}\n")).collect();
        std::fs::write(&path, &content).unwrap();
        let engine = Engine::new(EngineConfig {
            max_bytes_per_result: 200,
            total_token_budget: 50,
            ..EngineConfig::default()
        });
        let res = engine.read(&path);
        assert!(res.body.len() <= 200 + "[truncated to budget]\n".len());
    }
}
