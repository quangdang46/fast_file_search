//! Scry tool helpers for the MCP server: lazily-built `fff-engine` shared
//! across all `scry_*` tool calls and the parameter / response shapes.
//!
//! Existing FFF tools (`find_files`, `grep`, `multi_grep`) are untouched.
//! The scry tools are additive: they expose the symbol index, call-graph,
//! and token-budgeted read APIs from `fff-engine` to MCP clients.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use once_cell::sync::OnceCell;
use parking_lot::Mutex;

use fff_budget::FilterLevel;
use fff_engine::dispatch::DispatchResult;
use fff_engine::{Engine, EngineConfig, PreFilterStack};
use fff_symbol::symbol_index::SymbolLocation;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ScryDispatchParams {
    /// Free-form query. Auto-classified into file-path / glob / symbol / concept routing.
    pub query: String,
    /// Token budget for the response (default 25000).
    #[serde(rename = "maxTokens")]
    pub max_tokens: Option<f64>,
    /// Maximum hits returned (default 50).
    #[serde(rename = "maxResults")]
    pub max_results: Option<f64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ScrySymbolParams {
    /// Symbol name to look up. Trailing `*` switches to prefix search.
    pub name: String,
    /// Maximum hits returned (default 50).
    #[serde(rename = "maxResults")]
    pub max_results: Option<f64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ScryCallParams {
    /// Symbol name whose callers (or callees) should be located.
    pub name: String,
    /// Maximum hits returned (default 100).
    #[serde(rename = "maxResults")]
    pub max_results: Option<f64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ScryReadParams {
    /// Path to read, relative to the repository root or absolute.
    /// `path:line` is accepted; the line marker is currently informational.
    pub path: String,
    /// Token budget for the response (default 25000).
    #[serde(rename = "maxTokens")]
    pub max_tokens: Option<f64>,
    /// Filter intensity: "none", "minimal" (default), or "aggressive".
    pub filter: Option<String>,
}

/// Lazy holder for the shared `Engine`. The first scry call spends the cold
/// scan; subsequent calls hit the warm caches.
pub struct ScryEngineHolder {
    engine: OnceCell<Arc<Engine>>,
    // Default token budget propagated to every Engine that we build.
    // We rebuild the engine if the cwd or token budget materially differs,
    // but for now a single engine per server lifetime is enough.
    init_lock: Mutex<()>,
}

impl Default for ScryEngineHolder {
    fn default() -> Self {
        Self::new()
    }
}

impl ScryEngineHolder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            engine: OnceCell::new(),
            init_lock: Mutex::new(()),
        }
    }

    /// Return the engine, building it (and running an index pass over `root`)
    /// the first time.
    pub fn get_or_build(&self, root: &Path, total_token_budget: u64) -> Arc<Engine> {
        if let Some(e) = self.engine.get() {
            return e.clone();
        }
        let _g = self.init_lock.lock();
        if let Some(e) = self.engine.get() {
            return e.clone();
        }
        let cfg = EngineConfig {
            total_token_budget,
            ..EngineConfig::default()
        };
        let engine = Arc::new(Engine::new(cfg));
        engine.index(root);
        let _ = self.engine.set(engine.clone());
        engine
    }
}

#[derive(Debug, serde::Serialize)]
pub struct CallHit {
    pub path: String,
    pub line: u32,
    pub text: String,
}

/// Find call sites for `symbol`, narrowed by `BloomFilterCache` before the
/// final `String::contains` confirmation.
pub fn find_call_sites(engine: &Engine, root: &Path, symbol: &str, limit: usize) -> Vec<CallHit> {
    let definitions = engine.handles.symbols.lookup_exact(symbol);
    let definition_lines: Vec<(PathBuf, u32)> = definitions
        .iter()
        .map(|d| (d.path.clone(), d.line))
        .collect();

    let stack = PreFilterStack::new(engine.handles.bloom.clone());

    let mut candidates: Vec<(PathBuf, SystemTime, String)> = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .standard_filters(true)
        .follow_links(false)
        .build();
    for entry in walker.flatten() {
        if let Some(ft) = entry.file_type() {
            if !ft.is_file() {
                continue;
            }
        } else {
            continue;
        }
        let path = entry.into_path();
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        candidates.push((path, mtime, content));
    }

    let survivors = stack.confirm_symbol(
        &candidates
            .iter()
            .map(|(p, m, c)| (p.clone(), *m, c.clone()))
            .collect::<Vec<_>>(),
        symbol,
    );

    let mut survivor_set = std::collections::HashSet::new();
    for s in &survivors {
        survivor_set.insert(s.path.clone());
    }

    let mut hits = Vec::new();
    for (path, _mtime, content) in &candidates {
        if !survivor_set.contains(path) {
            continue;
        }
        let path_str = path.display().to_string();
        for (lineno, line) in content.lines().enumerate() {
            let lineno = (lineno + 1) as u32;
            if !line.contains(symbol) {
                continue;
            }
            if definition_lines
                .iter()
                .any(|(p, l)| p == path && *l == lineno)
            {
                continue;
            }
            hits.push(CallHit {
                path: path_str.clone(),
                line: lineno,
                text: line.to_string(),
            });
            if hits.len() >= limit {
                return hits;
            }
        }
    }
    hits
}

/// Find callees: symbols that the body of `symbol` references.
pub fn find_callee_sites(
    engine: &Engine,
    _root: &Path,
    symbol: &str,
    limit: usize,
) -> Vec<CallHit> {
    let definitions = engine.handles.symbols.lookup_exact(symbol);
    if definitions.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for def in definitions {
        let Ok(content) = std::fs::read_to_string(&def.path) else {
            continue;
        };
        let path_str = def.path.display().to_string();
        for (idx, line) in content.lines().enumerate() {
            let lineno = (idx + 1) as u32;
            if lineno < def.line || lineno > def.end_line {
                continue;
            }
            for tok in line.split(|c: char| !c.is_alphanumeric() && c != '_') {
                if tok.is_empty() || tok == symbol {
                    continue;
                }
                let candidates = engine.handles.symbols.lookup_exact(tok);
                if candidates.is_empty() {
                    continue;
                }
                hits.push(CallHit {
                    path: path_str.clone(),
                    line: lineno,
                    text: format!("{tok} ({})", candidates[0].kind),
                });
                if hits.len() >= limit {
                    return hits;
                }
            }
        }
    }
    hits
}

pub fn parse_filter_level(raw: Option<&str>) -> FilterLevel {
    match raw {
        Some("none") => FilterLevel::None,
        Some("aggressive") => FilterLevel::Aggressive,
        _ => FilterLevel::Minimal,
    }
}

pub fn format_symbol_hits(hits: &[SymbolLocation], name: &str) -> String {
    if hits.is_empty() {
        return format!("[no definitions found for '{name}']\n");
    }
    let mut out = String::new();
    for hit in hits {
        out.push_str(&format!(
            "{}:{}: [{}] (weight {})\n",
            hit.path.display(),
            hit.line,
            hit.kind,
            hit.weight,
        ));
    }
    out
}

pub fn format_call_hits(hits: &[CallHit], header: &str) -> String {
    if hits.is_empty() {
        return format!("[no {header} found]\n");
    }
    let mut out = String::new();
    for h in hits {
        out.push_str(&format!("{}:{}: {}\n", h.path, h.line, h.text));
    }
    out
}

pub fn format_dispatch(result: &DispatchResult) -> String {
    match result {
        DispatchResult::Symbol { hits, classified } => {
            let mut out = format!("[symbol] '{}' -> {} hits\n", classified.raw, hits.len());
            for h in hits.iter().take(50) {
                out.push_str(&format!("{}:{}: [{}]\n", h.path.display(), h.line, h.kind));
            }
            out
        }
        DispatchResult::SymbolGlob { hits, classified } => {
            let mut out = format!(
                "[symbol-glob] '{}' -> {} hits\n",
                classified.raw,
                hits.len()
            );
            for (name, h) in hits.iter().take(50) {
                out.push_str(&format!("{name}\t{}:{}\n", h.path.display(), h.line));
            }
            out
        }
        DispatchResult::Glob {
            classified,
            pattern,
        } => format!(
            "[glob] '{}' (pattern={pattern}) — use scry_find/scry_grep for full results\n",
            classified.raw,
        ),
        DispatchResult::FilePath { classified, path } => {
            format!("[file-path] '{}' -> {}\n", classified.raw, path.display(),)
        }
        DispatchResult::ContentFallback { classified } => format!(
            "[concept] '{}' — fall back to scry_grep for content search\n",
            classified.raw,
        ),
    }
}
