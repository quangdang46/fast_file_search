//! Scry tool helpers for the MCP server: lazily-built `ffs-engine` shared
//! across all `scry_*` tool calls and the parameter / response shapes.
//!
//! Existing FFF tools (`find_files`, `grep`, `multi_grep`) are untouched.
//! The scry tools are additive: they expose the symbol index, call-graph,
//! and token-budgeted read APIs from `ffs-engine` to MCP clients.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use once_cell::sync::OnceCell;
use parking_lot::Mutex;

use ffs_budget::FilterLevel;
use ffs_engine::dispatch::DispatchResult;
use ffs_engine::{Engine, EngineConfig, PreFilterStack};
use ffs_symbol::symbol_index::SymbolLocation;

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
pub struct ScryRefsParams {
    /// Symbol name to find definitions + single-hop usages for.
    pub name: String,
    /// Maximum usages returned (default 100). Definitions are always full.
    #[serde(rename = "maxResults")]
    pub max_results: Option<f64>,
    /// Skip this many usages before starting the page (default 0).
    pub offset: Option<f64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ScryFlowParams {
    /// Symbol name to drill down on.
    pub name: String,
    /// Maximum cards returned (default 10). One card per definition.
    #[serde(rename = "maxResults")]
    pub max_results: Option<f64>,
    /// Skip this many cards before starting the page (default 0).
    pub offset: Option<f64>,
    /// Maximum callees listed per card (default 5).
    #[serde(rename = "calleesTop")]
    pub callees_top: Option<f64>,
    /// Maximum callers listed per card (default 5).
    #[serde(rename = "callersTop")]
    pub callers_top: Option<f64>,
    /// Token budget for body excerpts (default 10000).
    pub budget: Option<f64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ScryImpactParams {
    /// Symbol name to score impact for.
    pub name: String,
    /// Maximum rows returned (default 20).
    #[serde(rename = "maxResults")]
    pub max_results: Option<f64>,
    /// Skip this many rows before starting the page (default 0).
    pub offset: Option<f64>,
    /// BFS depth for the transitive signal (default 3, capped at 3).
    pub hops: Option<f64>,
    /// Hub-guard threshold mirroring `scry callers` (default 50).
    #[serde(rename = "hubGuard")]
    pub hub_guard: Option<f64>,
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

// Invoke `scry <subcommand>` against `root` and return stdout. The MCP server
// runs inside the scry binary, so `current_exe()` is the scry binary itself.
// Used by the additive `scry_refs` / `scry_flow` / `scry_impact` tools that
// were too heavy to reimplement directly on top of the shared engine.
pub fn run_scry_subprocess(
    subcommand: &str,
    root: &Path,
    args: &[String],
) -> std::io::Result<String> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--root")
        .arg(root)
        .arg("--format")
        .arg("json")
        .arg(subcommand);
    for a in args {
        cmd.arg(a);
    }
    let out = cmd.output()?;
    if !out.status.success() {
        return Err(std::io::Error::other(format!(
            "scry {subcommand} exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn scry_refs_params_parse_minimal() {
        let p: ScryRefsParams = serde_json::from_value(json!({ "name": "foo" })).unwrap();
        assert_eq!(p.name, "foo");
        assert!(p.max_results.is_none());
        assert!(p.offset.is_none());
    }

    #[test]
    fn scry_refs_params_parse_full() {
        let p: ScryRefsParams =
            serde_json::from_value(json!({ "name": "foo", "maxResults": 25, "offset": 50 }))
                .unwrap();
        assert_eq!(p.max_results, Some(25.0));
        assert_eq!(p.offset, Some(50.0));
    }

    #[test]
    fn scry_flow_params_parse_full() {
        let p: ScryFlowParams = serde_json::from_value(json!({
            "name": "bar",
            "maxResults": 3,
            "offset": 1,
            "calleesTop": 7,
            "callersTop": 8,
            "budget": 5000,
        }))
        .unwrap();
        assert_eq!(p.name, "bar");
        assert_eq!(p.callees_top, Some(7.0));
        assert_eq!(p.callers_top, Some(8.0));
        assert_eq!(p.budget, Some(5000.0));
    }

    #[test]
    fn scry_impact_params_parse_full() {
        let p: ScryImpactParams = serde_json::from_value(json!({
            "name": "baz",
            "maxResults": 10,
            "offset": 0,
            "hops": 2,
            "hubGuard": 30,
        }))
        .unwrap();
        assert_eq!(p.name, "baz");
        assert_eq!(p.hops, Some(2.0));
        assert_eq!(p.hub_guard, Some(30.0));
    }

    #[test]
    fn scry_refs_params_rejects_missing_name() {
        let r: Result<ScryRefsParams, _> = serde_json::from_value(json!({ "maxResults": 1 }));
        assert!(r.is_err());
    }
}
