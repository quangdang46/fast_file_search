//! Engine tool helpers for the MCP server: lazily-built `ffs-engine` shared
//! across all `engine_*` tool calls and the parameter / response shapes.
//! Existing ffs tools (`ffs_find`, `ffs_grep`, `ffs_multi_grep`) are untouched.
//! The engine tools are additive: they expose the symbol index, call-graph,
//! and token-budgeted read APIs from `ffs-engine` to MCP clients.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use once_cell::sync::OnceCell;
use parking_lot::Mutex;

use ffs_budget::FilterLevel;
use ffs_engine::{Engine, EngineConfig, PreFilterStack};
use ffs_symbol::lang::detect_file_type;
use ffs_symbol::symbol_index::{SymbolIndex, SymbolLocation};
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EngineSymbolParams {
    /// Symbol name to look up. Trailing `*` switches to prefix search.
    pub name: String,
    /// Maximum hits returned (default 50).
    #[serde(rename = "maxResults")]
    pub max_results: Option<f64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EngineCallParams {
    /// Symbol name whose callers (or callees) should be located.
    pub name: String,
    /// Maximum hits returned (default 100).
    #[serde(rename = "maxResults")]
    pub max_results: Option<f64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EngineRefsParams {
    /// Symbol name to find definitions + single-hop usages for.
    pub name: String,
    /// Maximum usages returned (default 100). Definitions are always full.
    #[serde(rename = "maxResults")]
    pub max_results: Option<f64>,
    /// Skip this many usages before starting the page (default 0).
    pub offset: Option<f64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EngineFlowParams {
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
    /// Byte budget for body excerpts across all cards (default 10000, 0 = unlimited).
    pub budget: Option<f64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EngineImpactParams {
    /// Symbol name to score impact for.
    pub name: String,
    /// Maximum rows returned (default 20).
    #[serde(rename = "maxResults")]
    pub max_results: Option<f64>,
    /// Skip this many rows before starting the page (default 0).
    pub offset: Option<f64>,
    /// BFS depth for the transitive signal (default 3, capped at 3).
    pub hops: Option<f64>,
    /// Hub-guard threshold mirroring `ffs callers` (default 50).
    #[allow(dead_code)]
    #[serde(rename = "hubGuard")]
    pub hub_guard: Option<f64>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EngineReadParams {
    /// Path to read, relative to the repository root or absolute.
    /// `path:line` is accepted; the line marker is currently informational.
    pub path: String,
    /// Token budget for the response (default 25000).
    #[serde(rename = "maxTokens")]
    pub max_tokens: Option<f64>,
    /// Filter intensity: "none", "minimal" (default), or "aggressive".
    pub filter: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EngineSiblingsParams {
    /// Symbol whose siblings (peers in the same parent scope) should be listed.
    pub name: String,
    /// Maximum siblings returned (default 100).
    #[serde(rename = "maxResults")]
    pub max_results: Option<f64>,
    /// Skip this many siblings before starting the page (default 0).
    pub offset: Option<f64>,
    /// Include `Import` entries as siblings (default false).
    #[serde(rename = "includeImports")]
    pub include_imports: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EngineDepsParams {
    /// File to analyse, relative to the repository root.
    pub target: String,
    /// Maximum dependents returned (default 100).
    #[serde(rename = "maxResults")]
    pub max_results: Option<f64>,
    /// Skip this many dependents before starting the page (default 0).
    pub offset: Option<f64>,
    /// Skip the dependents walk; resolve imports only (default false).
    #[allow(dead_code)]
    #[serde(rename = "noDependents")]
    pub no_dependents: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EngineMapParams {
    /// Maximum tree depth to render. Beyond this, directories show as a
    /// single summary line (default 3).
    pub depth: Option<f64>,
    /// Annotate each file leaf with its top-N symbols by weight (default 0
    /// = no annotation).
    pub symbols: Option<f64>,
}

/// Lazy holder for the shared `Engine`. The first engine call spends the cold
/// scan; subsequent calls hit the warm caches.
pub struct EngineHolder {
    engine: OnceCell<Arc<Engine>>,
    // Default token budget propagated to every Engine that we build.
    // We rebuild the engine if the cwd or token budget materially differs,
    // but for now a single engine per server lifetime is enough.
    init_lock: Mutex<()>,
}

impl Default for EngineHolder {
    fn default() -> Self {
        Self::new()
    }
}

impl EngineHolder {
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

    /// Pre-warm the engine by building it (idempotent). Call this after
    /// the initial filesystem scan completes to avoid cold-start latency
    /// on the first engine tool call.
    pub fn warmup(&self, root: &Path, total_token_budget: u64) {
        self.get_or_build(root, total_token_budget);
    }
}

#[derive(Debug, serde::Serialize)]
pub struct CallHit {
    pub path: String,
    pub line: u32,
    pub text: String,
}

/// Resolve a symbol name, supporting qualified `"Type::method"` syntax.
/// Falls back to the bare method name when the qualified form isn't indexed.
fn resolve_symbol_name(name: &str, symbols: &SymbolIndex) -> Vec<SymbolLocation> {
    let defs = symbols.lookup_exact(name);
    if !defs.is_empty() {
        return defs;
    }
    if let Some((_type_name, method)) = name.rsplit_once("::") {
        let defs = symbols.lookup_exact(method);
        if !defs.is_empty() {
            return defs;
        }
    }
    Vec::new()
}

/// Render a path relative to the workspace root, falling back to the
/// absolute path when it isn't under `root` (e.g. symlinked out). Symbol/
/// caller/callee output repeats the path once per hit, so absolute paths
/// waste a lot of tokens on deep Windows checkouts — same fix as the
/// `--compact` grep mode (#93).
pub fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Find call sites for `symbol`, narrowed by `BloomFilterCache` before the
/// final `String::contains` confirmation.
pub fn find_call_sites(engine: &Engine, root: &Path, symbol: &str, limit: usize) -> Vec<CallHit> {
    let definitions = resolve_symbol_name(symbol, &engine.handles.symbols);
    let definition_lines: Vec<(PathBuf, u32)> = definitions
        .iter()
        .map(|d| (d.path.clone(), d.line))
        .collect();

    let stack = PreFilterStack::new(engine.handles.bloom.clone());

    let candidates = walk_code_files(root);

    let survivors = stack.confirm_symbol(&candidates, symbol);

    let mut survivor_set = std::collections::HashSet::new();
    for s in &survivors {
        survivor_set.insert(s.path.clone());
    }

    let mut hits = Vec::new();
    for (path, _mtime, content) in &candidates {
        if !survivor_set.contains(path) {
            continue;
        }
        let path_str = rel_path(root, path);
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
pub fn find_callee_sites(engine: &Engine, root: &Path, symbol: &str, limit: usize) -> Vec<CallHit> {
    let definitions = resolve_symbol_name(symbol, &engine.handles.symbols);
    if definitions.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for def in definitions {
        let Ok(content) = ffs::bom::read_file(&def.path) else {
            continue;
        };
        let path_str = rel_path(root, &def.path);
        for (idx, line) in content.lines().enumerate() {
            let lineno = (idx + 1) as u32;
            if lineno < def.line || lineno > def.end_line {
                continue;
            }
            for tok in line.split(|c: char| !c.is_alphanumeric() && c != '_') {
                if tok.is_empty() || tok == symbol {
                    continue;
                }
                let candidates = resolve_symbol_name(tok, &engine.handles.symbols);
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

/// Walk all code files under `root` with standard .gitignore filters,
/// returning `(path, mtime, content)`.
///
/// Only code files are read — matches CLI `walk_files`, whose callers also
/// filter by `detect_file_type() == Code(_)`; skipping non-code avoids binary
/// bloat and keeps callers/refs output consistent with the CLI.
///
/// NOTE: this materializes every code file's contents at once, so peak memory
/// scales with total source size (a large monorepo is tens of MB held live for
/// the duration of one callers/refs request). Fine for typical repos; if this
/// ever shows up in a profile, the fix is to probe the bloom cache per file
/// during the walk and retain only survivors' content.
fn walk_code_files(root: &Path) -> Vec<(std::path::PathBuf, SystemTime, String)> {
    use ignore::WalkBuilder;
    let mut out = Vec::new();
    for entry in WalkBuilder::new(root)
        .standard_filters(true)
        .follow_links(false)
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.into_path();
        if !matches!(
            detect_file_type(&path),
            ffs_symbol::types::FileType::Code(_)
        ) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let Ok(content) = ffs::bom::read_file(&path) else {
            continue;
        };
        out.push((path, mtime, content));
    }
    out
}

/// Find the innermost enclosing symbol from an outline for a given line.
fn enclosing_symbol(entries: &[ffs_symbol::types::OutlineEntry], line: u32) -> Option<String> {
    fn walk(
        entries: &[ffs_symbol::types::OutlineEntry],
        line: u32,
    ) -> Option<&ffs_symbol::types::OutlineEntry> {
        for e in entries {
            if line < e.start_line || line > e.end_line {
                continue;
            }
            if let Some(child) = walk(&e.children, line) {
                return Some(child);
            }
            return Some(e);
        }
        None
    }
    walk(entries, line).map(|e| e.name.clone())
}

/// Result of an in-process refs lookup. Mirrors the CLI's `RefUsage`.
pub struct RefUsage {
    pub path: String,
    pub line: u32,
    pub text: String,
    pub enclosing: Option<String>,
}

pub struct RefsResult {
    pub name: String,
    pub definitions: Vec<SymbolLocation>,
    pub usages: Vec<RefUsage>,
    pub total_usages: usize,
    pub offset: usize,
    pub subclasses: Vec<RefUsage>,
    pub has_more: bool,
}

/// Find definitions + usages for `name` in-process, matching the CLI's `ffs refs` command.
pub fn find_refs(
    engine: &Engine,
    root: &Path,
    name: &str,
    limit: usize,
    offset: usize,
) -> RefsResult {
    let mut definitions: Vec<SymbolLocation> = resolve_symbol_name(name, &engine.handles.symbols);
    definitions.sort_by_key(|b| std::cmp::Reverse(b.weight));
    let definition_line_set: std::collections::HashSet<(String, u32)> = definitions
        .iter()
        .map(|d| (d.path.to_string_lossy().to_string(), d.line))
        .collect();

    let candidates = walk_code_files(root);

    let stack = PreFilterStack::new(engine.handles.bloom.clone());
    let confirm_input: Vec<_> = candidates
        .iter()
        .map(|(p, m, c)| (p.clone(), *m, c.clone()))
        .collect();
    let survivors = stack.confirm_symbol(&confirm_input, name);
    let survivor_set: std::collections::HashSet<&std::path::Path> =
        survivors.iter().map(|s| s.path.as_path()).collect();
    let mut usages: Vec<RefUsage> = Vec::new();
    for (path, mtime, content) in &candidates {
        if !survivor_set.contains(path.as_path()) {
            continue;
        }
        // Definition-line skip set is keyed by absolute path; emit relative.
        let abs_str = path.to_string_lossy().to_string();
        let path_str = rel_path(root, path);
        // Detect language for outline computation
        let lang = match detect_file_type(path) {
            ffs_symbol::types::FileType::Code(l) => l,
            _ => continue,
        };
        let outline = engine
            .handles
            .outlines
            .get_or_compute(path, *mtime, content, lang);

        for (lineno, line) in content.lines().enumerate() {
            let lineno = (lineno + 1) as u32;
            if !line.contains(name) {
                continue;
            }
            // Skip definition lines
            if definition_line_set.contains(&(abs_str.clone(), lineno)) {
                continue;
            }
            usages.push(RefUsage {
                path: path_str.clone(),
                line: lineno,
                text: line.to_string(),
                enclosing: enclosing_symbol(&outline, lineno),
            });
        }
    }

    // Subclass detection: scan for `class child(parent)` or `name[extends](parent)`
    let mut subclasses: Vec<RefUsage> = Vec::new();
    for (path, _mtime, content) in &candidates {
        if !survivor_set.contains(path.as_path()) {
            continue;
        }
        let path_str = rel_path(root, path);
        for (lineno, line) in content.lines().enumerate() {
            let lineno = (lineno + 1) as u32;
            if !line.contains(name) {
                continue;
            }
            // Check for `class child(parent)` or `childName(parent,)` patterns
            let trimmed = line.trim();
            let lower = trimmed.to_lowercase();
            if !lower.starts_with("class ") && !lower.contains(": class ") && !trimmed.contains("<")
            {
                continue;
            }
            // Must be a subclass definition: class X(name) or class X[extends](name)
            // Look for name inside parentheses with a class-like prefix
            if let Some(paren_start) = trimmed.find('(')
                && let Some(paren_end) = trimmed[paren_start..].find(')')
            {
                let paren_content = &trimmed[paren_start + 1..paren_start + paren_end];
                if paren_content.contains(name) {
                    subclasses.push(RefUsage {
                        path: path_str.clone(),
                        line: lineno,
                        text: line.to_string(),
                        enclosing: None,
                    });
                }
            }
        }
    }

    let total_usages = usages.len();
    let has_more = offset + limit < total_usages;
    if offset > 0 && offset < total_usages {
        usages.drain(..offset.min(total_usages));
    }
    RefsResult {
        name: name.to_string(),
        definitions,
        usages,
        subclasses,
        total_usages,
        offset,
        has_more,
    }
}

/// Format a `RefsResult` as human-readable text (matching CLI `ffs refs` text format).
pub fn format_refs_result(r: &RefsResult, root: &Path) -> String {
    let mut out = String::new();
    out.push_str(&format!("Symbol: {}\n", r.name));
    out.push_str(&format!("Definitions ({}):\n", r.definitions.len()));
    if r.definitions.is_empty() {
        out.push_str("  [none]\n");
    } else {
        for d in &r.definitions {
            // Sorted by weight already; the number itself is internal noise.
            out.push_str(&format!(
                "  {}:{} ({})\n",
                rel_path(root, &d.path),
                d.line,
                d.kind,
            ));
        }
    }

    out.push_str(&format!("\nUsages ({}):\n", r.total_usages));
    if r.total_usages == 0 {
        out.push_str("  [none]\n");
    } else {
        for u in &r.usages {
            // Only claim an enclosing symbol when one resolved — `(in ?)`
            // is a placeholder that costs bytes and says "unknown".
            match u.enclosing.as_deref() {
                Some(encl) => out.push_str(&format!(
                    "  {}:{} (in {}): {}\n",
                    u.path, u.line, encl, u.text,
                )),
                None => out.push_str(&format!("  {}:{}: {}\n", u.path, u.line, u.text)),
            }
        }
        if r.has_more {
            out.push_str(&format!(
                "  ... and {} more (use offset={})\n",
                r.total_usages - r.offset - r.usages.len(),
                r.offset + r.usages.len(),
            ));
        }
    }

    if !r.subclasses.is_empty() {
        out.push_str(&format!("\nSubclasses ({}):\n", r.subclasses.len()));
        for s in &r.subclasses {
            out.push_str(&format!("  {}:{}: {}\n", s.path, s.line, s.text,));
        }
    }

    if r.definitions.is_empty() && r.total_usages == 0 {
        out.push_str("\n[no references found]\n");
    }
    out
}

pub fn parse_filter_level(raw: Option<&str>) -> FilterLevel {
    match raw {
        Some("none") => FilterLevel::None,
        Some("aggressive") => FilterLevel::Aggressive,
        _ => FilterLevel::Minimal,
    }
}

pub fn format_symbol_hits(hits: &[SymbolLocation], name: &str, root: &Path) -> String {
    if hits.is_empty() {
        return format!("[no definitions found for '{name}']\n");
    }
    let mut out = String::new();
    for hit in hits {
        // `weight` is the internal index ranking integer and the list is
        // already sorted by it, so printing it costs bytes and tells the
        // caller nothing it can act on.
        out.push_str(&format!(
            "{}:{}: [{}]\n",
            rel_path(root, &hit.path),
            hit.line,
            hit.kind,
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

fn format_outline_kind(kind: ffs_symbol::types::OutlineKind) -> &'static str {
    use ffs_symbol::types::OutlineKind;
    match kind {
        OutlineKind::Function => "fn",
        OutlineKind::Class => "class",
        OutlineKind::Struct => "struct",
        OutlineKind::Interface => "interface",
        OutlineKind::Enum => "enum",
        OutlineKind::Constant => "const",
        OutlineKind::Variable => "var",
        OutlineKind::Module => "mod",
        OutlineKind::Import => "import",
        OutlineKind::TypeAlias => "type",
        OutlineKind::Export => "export",
        OutlineKind::Property => "property",
        OutlineKind::Impl => "impl",
    }
}
pub fn find_siblings(
    engine: &Engine,
    root: &Path,
    name: &str,
    _include_imports: bool,
    limit: usize,
    offset: usize,
) -> String {
    let mut definitions = resolve_symbol_name(name, &engine.handles.symbols);
    definitions.sort_by_key(|b| std::cmp::Reverse(b.weight));
    let mut out = String::new();
    if definitions.is_empty() {
        out.push_str(&format!("[no definitions found for `{name}`]\n"));
        return out;
    }
    for (idx, def) in definitions.iter().enumerate() {
        out.push_str(&format!(
            "definition {}/{}: {}:{} ({})\n",
            idx + 1,
            definitions.len(),
            rel_path(root, &def.path),
            def.line,
            def.kind,
        ));
        // Load outline and find siblings
        let ft = ffs_symbol::lang::detect_file_type(&def.path);
        let lang = match ft {
            ffs_symbol::types::FileType::Code(l) => l,
            _ => continue,
        };
        let Ok(content) = ffs::bom::read_file(&def.path) else {
            continue;
        };
        let entries = ffs_symbol::outline::get_outline_entries(&content, lang);
        let siblings = collect_siblings(&entries, name, def.line);
        if siblings.is_empty() {
            out.push_str("  [no siblings found]\n");
        } else {
            let total = siblings.len();
            let page: Vec<_> = siblings.into_iter().skip(offset).take(limit).collect();
            for s in &page {
                out.push_str(&format!(
                    "  [{}:{}] {} {}\n",
                    s.start_line,
                    s.end_line,
                    format_outline_kind(s.kind),
                    s.name,
                ));
            }
            if offset + page.len() < total {
                out.push_str(&format!("  ... {} more\n", total - offset - page.len()));
            }
        }
    }
    out
}

/// Collect siblings of `target` at `target_line` from the outline.
fn collect_siblings(
    entries: &[ffs_symbol::types::OutlineEntry],
    target: &str,
    target_line: u32,
) -> Vec<ffs_symbol::types::OutlineEntry> {
    fn find_in_children(
        parent: &ffs_symbol::types::OutlineEntry,
        target: &str,
        target_line: u32,
    ) -> Option<Vec<ffs_symbol::types::OutlineEntry>> {
        for child in &parent.children {
            if child.name == target && child.start_line == target_line {
                return Some(parent.children.clone());
            }
            if child.start_line <= target_line && target_line <= child.end_line {
                return find_in_children(child, target, target_line);
            }
        }
        None
    }

    for entry in entries {
        if entry.name == target && entry.start_line == target_line {
            // Top-level: return all top-level entries (file-level siblings)
            return entries.to_vec();
        }
        if entry.start_line <= target_line && target_line <= entry.end_line {
            if let Some(sibs) = find_in_children(entry, target, target_line) {
                return sibs;
            }
            break;
        }
    }
    Vec::new()
}

/// Render a tree map of the workspace. Mirrors `ffs map`.
pub fn format_map(root: &Path, depth: u32, _symbols: u32) -> String {
    use ignore::WalkBuilder;
    use std::collections::BTreeMap;

    // Collect directory stats
    let mut dirs: BTreeMap<String, (u32, u64)> = BTreeMap::new(); // path -> (file_count, total_bytes)
    for entry in WalkBuilder::new(root)
        .standard_filters(true)
        .follow_links(false)
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.into_path();
        let parent = path.parent().unwrap_or(root);
        let rel = parent
            .strip_prefix(root)
            .unwrap_or(parent)
            .to_string_lossy()
            .to_string();
        let key = if rel.is_empty() { ".".to_string() } else { rel };
        let size = std::fs::metadata(&path).ok().map(|m| m.len()).unwrap_or(0);
        let entry = dirs.entry(key).or_default();
        entry.0 += 1;
        entry.1 += size;
    }

    let mut out = String::new();
    out.push_str(&format!("{} ({} entries)\n\n", root.display(), dirs.len()));
    for (rel, &(count, size)) in &dirs {
        let depth_actual = rel.split('/').count() as u32;
        let max_depth = if rel == "." { 0u32 } else { depth };
        if depth > 0 && depth_actual > max_depth + 1 {
            continue;
        }
        let indent = "  ".repeat(depth_actual.saturating_sub(1) as usize);
        let name = if rel == "." {
            "."
        } else {
            rel.split('/').next_back().unwrap_or(rel)
        };
        let tokens = ffs_symbol::types::estimate_tokens(size / u64::from(count));
        out.push_str(&format!(
            "{indent}{}/ — {} files, ~{} tokens\n",
            name, count, tokens,
        ));
    }
    out
}

/// Check if a file path looks like an entry point (main.rs, lib.rs, index.*, etc.)
fn _is_entry_point(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    matches!(
        name,
        "main.rs"
            | "lib.rs"
            | "mod.rs"
            | "index.ts"
            | "index.js"
            | "index.tsx"
            | "index.jsx"
            | "__init__.py"
            | "main.py"
            | "main.go"
            | "main.c"
            | "main.cpp"
    )
}

/// Find imports for a file and dependents. Simplified in-process version.
pub fn find_deps(root: &Path, target: &str, limit: usize, _offset: usize) -> String {
    let target_path = if std::path::Path::new(target).is_absolute() {
        std::path::PathBuf::from(target)
    } else {
        root.join(target)
    };

    let mut out = String::new();
    let Ok(content) = ffs::bom::read_file(&target_path) else {
        out.push_str(&format!("[cannot read {}]\n", target_path.display()));
        return out;
    };
    let ft = ffs_symbol::lang::detect_file_type(&target_path);
    let lang = match ft {
        ffs_symbol::types::FileType::Code(l) => l,
        _ => {
            out.push_str(&format!("[{} is not a code file]\n", target_path.display()));
            return out;
        }
    };

    // Extract imports (simple pattern)
    out.push_str(&format!("File: {}\n", target_path.display()));
    out.push_str(&format!("Language: {lang:?}\n\n"));

    out.push_str("Imports:\n");
    let imports = extract_simple_imports(&content, lang);
    if imports.is_empty() {
        out.push_str("  [none found]\n");
    } else {
        for imp in &imports {
            out.push_str(&format!("  {imp}\n"));
        }
    }

    // Find dependents (files that reference the target)
    out.push_str(&format!("\nDependents (top {limit}):\n"));
    let target_name = target_path
        .file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    if target_name.is_empty() {
        out.push_str("  [could not determine target name]\n");
    } else {
        let deps = find_dependents(root, &target_name, limit);
        if deps.is_empty() {
            out.push_str("  [none found]\n");
        } else {
            for d in &deps {
                out.push_str(&format!("  {}\n", d.display()));
            }
        }
    }
    out
}

fn extract_simple_imports(content: &str, lang: ffs_symbol::types::Lang) -> Vec<String> {
    match lang {
        ffs_symbol::types::Lang::Rust
        | ffs_symbol::types::Lang::TypeScript
        | ffs_symbol::types::Lang::JavaScript
        | ffs_symbol::types::Lang::Tsx => content
            .lines()
            .filter(|l| l.trim().starts_with("use ") || l.trim().starts_with("import "))
            .map(|l| l.trim().to_string())
            .collect(),
        ffs_symbol::types::Lang::Python => content
            .lines()
            .filter(|l| l.trim().starts_with("import ") || l.trim().starts_with("from "))
            .map(|l| l.trim().to_string())
            .collect(),
        ffs_symbol::types::Lang::Go => content
            .lines()
            .filter(|l| l.trim().starts_with("import "))
            .map(|l| l.trim().to_string())
            .collect(),
        ffs_symbol::types::Lang::Verse => content
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with("using {") || t.starts_with("import ")
            })
            .map(|l| l.trim().to_string())
            .collect(),
        ffs_symbol::types::Lang::C | ffs_symbol::types::Lang::Cpp => content
            .lines()
            .filter(|l| l.trim().starts_with("#include"))
            .map(|l| l.trim().to_string())
            .collect(),
        _ => content
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with("import ") || t.starts_with("use ") || t.starts_with("#include")
            })
            .map(|l| l.trim().to_string())
            .take(50)
            .collect(),
    }
}

fn find_dependents(root: &Path, target_name: &str, limit: usize) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in ignore::WalkBuilder::new(root)
        .standard_filters(true)
        .follow_links(false)
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.into_path();
        if !matches!(
            ffs_symbol::lang::detect_file_type(&path),
            ffs_symbol::types::FileType::Code(_)
        ) {
            continue;
        }
        let Ok(content) = ffs::bom::read_file(&path) else {
            continue;
        };
        if content.contains(target_name) {
            out.push(path);
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

/// Options for [`find_flow`]. Grouped into a struct because the function
/// takes more parameters than clippy's `too_many_arguments` lint allows.
pub struct FlowOptions<'a> {
    pub engine: &'a Engine,
    pub root: &'a Path,
    pub name: &'a str,
    pub limit: usize,
    pub offset: usize,
    pub callees_top: usize,
    pub callers_top: usize,
    /// Caps the total bytes of body excerpts (0 = unlimited). It is a real
    /// cap, not advisory: without it a symbol with many long definitions
    /// dumps every raw line of each one, and the default `maxResults` of 10
    /// made that unbounded.
    pub budget: usize,
}

/// Simplified flow: definitions + body + callees + callers.
pub fn find_flow(opts: FlowOptions<'_>) -> String {
    let FlowOptions {
        engine,
        root,
        name,
        limit,
        offset,
        callees_top,
        callers_top,
        budget,
    } = opts;
    let mut definitions = resolve_symbol_name(name, &engine.handles.symbols);
    definitions.sort_by_key(|b| std::cmp::Reverse(b.weight));
    let mut out = String::new();
    if definitions.is_empty() {
        out.push_str(&format!("[no definitions for `{name}`]\n"));
        return out;
    }

    let total = definitions.len();
    let page: Vec<_> = definitions.into_iter().skip(offset).take(limit).collect();

    // Callers and callees are keyed on the symbol name, not the individual
    // definition, so every card would print byte-identical lists. Emit them
    // once up front instead of N times.
    let callees = find_callee_sites(engine, root, name, callees_top.max(1));
    out.push_str(&format!(
        "callees ({} shown of {}):\n",
        callees.len().min(callees_top),
        callees.len(),
    ));
    if callees.is_empty() {
        out.push_str("  [none]\n");
    } else {
        for c in callees.iter().take(callees_top) {
            out.push_str(&format!("  {} @ {}:{}\n", c.text, c.path, c.line));
        }
    }

    let callers = find_call_sites(engine, root, name, callers_top.max(50));
    out.push_str(&format!(
        "callers ({} shown of {}):\n",
        callers.len().min(callers_top),
        callers.len(),
    ));
    if callers.is_empty() {
        out.push_str("  [none]\n");
    } else {
        for c in callers.iter().take(callers_top) {
            out.push_str(&format!("  {}:{}: {}\n", c.path, c.line, c.text));
        }
    }
    out.push('\n');

    let mut body_bytes_left = if budget == 0 { usize::MAX } else { budget };
    for (idx, def) in page.iter().enumerate() {
        let card_idx = offset + idx + 1;
        out.push_str(&format!(
            "── card {card_idx}/{total}: {name} @ {}:{} ({}) ──\n",
            rel_path(root, &def.path),
            def.line,
            def.kind,
        ));

        if body_bytes_left == 0 {
            out.push_str("body: [budget exhausted]\n\n");
            continue;
        }

        // Body excerpt, clipped to whatever is left of the budget.
        if let Ok(content) = ffs::bom::read_file(&def.path) {
            let start = def.line.saturating_sub(1) as usize;
            let end = (def.end_line as usize).min(content.lines().count());
            out.push_str(&format!("body [{}..{}]:\n", def.line, end));
            let mut wrote = 0usize;
            for (i, line) in content.lines().enumerate().skip(start).take(end - start) {
                let row = format!("  {:>4}: {line}\n", i + 1);
                if row.len() > body_bytes_left {
                    out.push_str("  [body truncated by budget]\n");
                    body_bytes_left = 0;
                    break;
                }
                body_bytes_left -= row.len();
                wrote += 1;
                out.push_str(&row);
            }
            if wrote < end.saturating_sub(start) && body_bytes_left > 0 {
                out.push_str("  [body truncated]\n");
            }
        }
        out.push('\n');
    }

    if offset + page.len() < total {
        out.push_str(&format!("... and {} more\n", total - offset - page.len()));
    }
    out
}

/// Simplified impact: score files by direct caller count + reverse imports.
pub fn find_impact(
    engine: &Engine,
    root: &Path,
    name: &str,
    limit: usize,
    offset: usize,
    _hops: u32,
) -> String {
    let mut definitions = resolve_symbol_name(name, &engine.handles.symbols);
    definitions.sort_by_key(|b| std::cmp::Reverse(b.weight));
    if definitions.is_empty() {
        return format!("[no impact found for {name}]\n");
    }

    // Direct callers
    let callers = find_call_sites(engine, root, name, limit.max(100));
    let mut scores: std::collections::BTreeMap<String, (u32, u32)> =
        std::collections::BTreeMap::new(); // path -> (direct_score, import_score)

    for c in &callers {
        let entry = scores.entry(c.path.clone()).or_default();
        entry.0 += 3; // direct callers weighted 3x
    }

    // Reverse imports: files that import files containing the symbol.
    //
    // Everything here must be keyed the same way `find_call_sites` keys its
    // hits — by relative path. `scores` is a BTreeMap keyed by that string, so
    // mixing in absolute paths would split one file across two rows (and one
    // row would print an absolute path while the other printed the relative
    // one). `def_paths` likewise has to hold relative paths for the
    // definition-file skip below to actually match.
    let def_paths: std::collections::HashSet<String> = definitions
        .iter()
        .map(|d| rel_path(root, &d.path))
        .collect();
    for entry in ignore::WalkBuilder::new(root)
        .standard_filters(true)
        .follow_links(false)
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.into_path();
        let path_str = rel_path(root, &path);
        if def_paths.contains(&path_str) {
            continue;
        }
        if !matches!(
            ffs_symbol::lang::detect_file_type(&path),
            ffs_symbol::types::FileType::Code(_)
        ) {
            continue;
        }
        let Ok(content) = ffs::bom::read_file(&path) else {
            continue;
        };
        for def_path in &def_paths {
            let stem = std::path::Path::new(def_path)
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if !stem.is_empty() && content.contains(stem) {
                let entry = scores.entry(path_str.clone()).or_default();
                entry.1 += 2; // import edge weighted 2x
                break;
            }
        }
    }

    // Sort by total score desc
    let mut rows: Vec<_> = scores.into_iter().collect();
    rows.sort_by(|a, b| {
        let a_score = a.1.0 + a.1.1;
        let b_score = b.1.0 + b.1.1;
        b_score.cmp(&a_score).then_with(|| a.0.cmp(&b.0))
    });

    let total = rows.len();
    let page: Vec<_> = rows.into_iter().skip(offset).take(limit).collect();
    if page.is_empty() {
        return format!("[no impact found for {name}]\n");
    }

    let mut out = String::new();
    for (path, (direct, import)) in &page {
        let score = direct + import;
        out.push_str(&format!("{score:>5}  {path}\n"));
    }
    if offset + page.len() < total {
        out.push_str(&format!("... and {} more\n", total - offset - page.len()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn engine_refs_params_parse_minimal() {
        let p: EngineRefsParams = serde_json::from_value(json!({ "name": "foo" })).unwrap();
        assert_eq!(p.name, "foo");
        assert!(p.max_results.is_none());
        assert!(p.offset.is_none());
    }

    #[test]
    fn engine_refs_params_parse_full() {
        let p: EngineRefsParams =
            serde_json::from_value(json!({ "name": "foo", "maxResults": 25, "offset": 50 }))
                .unwrap();
        assert_eq!(p.max_results, Some(25.0));
        assert_eq!(p.offset, Some(50.0));
    }

    #[test]
    fn engine_flow_params_parse_full() {
        let p: EngineFlowParams = serde_json::from_value(json!({
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
    fn engine_impact_params_parse_full() {
        let p: EngineImpactParams = serde_json::from_value(json!({
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
    fn engine_refs_params_rejects_missing_name() {
        let r: Result<EngineRefsParams, _> = serde_json::from_value(json!({ "maxResults": 1 }));
        assert!(r.is_err());
    }

    // --- Fix #87: resolve_symbol_name qualified name support ---

    fn make_index(entries: Vec<(&str, SymbolLocation)>) -> ffs_symbol::symbol_index::SymbolIndex {
        use ffs_symbol::symbol_index::{SymbolIndex, SymbolIndexSnapshot};
        let mut map = std::collections::HashMap::new();
        for (name, loc) in entries {
            map.entry(name.to_string())
                .or_insert_with(Vec::new)
                .push(loc);
        }
        SymbolIndex::from_snapshot(SymbolIndexSnapshot {
            map,
            files: std::collections::HashMap::new(),
        })
    }

    #[test]
    fn resolve_symbol_name_exact_match() {
        let idx = make_index(vec![(
            "foo",
            SymbolLocation {
                path: "a.rs".into(),
                line: 1,
                end_line: 5,
                kind: "function_item".into(),
                weight: 100,
            },
        )]);
        let result = resolve_symbol_name("foo", &idx);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].kind, "function_item");
    }

    #[test]
    fn resolve_symbol_name_qualified_fallback() {
        let idx = make_index(vec![(
            "patch",
            SymbolLocation {
                path: "editor.rs".into(),
                line: 10,
                end_line: 20,
                kind: "function_item".into(),
                weight: 100,
            },
        )]);
        let result = resolve_symbol_name("Editor::patch", &idx);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].path.to_str().unwrap(), "editor.rs");
    }

    #[test]
    fn resolve_symbol_name_qualified_not_found() {
        let idx = make_index(vec![]);
        let result = resolve_symbol_name("Type::missing", &idx);
        assert!(result.is_empty());
    }

    // --- Fix #85: catch_unwind_result ---

    #[test]
    fn catch_unwind_result_normal_return() {
        let r = crate::server::catch_unwind_result(|| {
            Ok(rmcp::model::CallToolResult::success(vec![
                rmcp::model::Content::text("ok"),
            ]))
        });
        assert!(r.is_ok());
    }

    #[test]
    fn catch_unwind_result_catches_panic() {
        let r: Result<rmcp::model::CallToolResult, rmcp::model::ErrorData> =
            crate::server::catch_unwind_result(|| {
                panic!("test panic");
            });
        assert!(r.is_err());
        let err = r.unwrap_err();
        assert!(err.message.contains("test panic"));
    }
}

#[cfg(test)]
mod rel_path_tests {
    use super::rel_path;
    use std::path::Path;

    #[test]
    fn strips_root_prefix() {
        let root = Path::new("/repo");
        let out = rel_path(root, Path::new("/repo/src/main.rs"));
        assert_eq!(out.replace('\\', "/"), "src/main.rs");
    }

    #[test]
    fn falls_back_to_absolute_outside_root() {
        let root = Path::new("/repo");
        let outside = Path::new("/elsewhere/lib.rs");
        let out = rel_path(root, outside);
        assert!(out.contains("elsewhere"), "got {out:?}");
    }
}

#[cfg(test)]
mod impact_path_tests {
    use super::*;
    use ffs_engine::{Engine, EngineConfig};

    /// Regression: `find_impact`'s score map is keyed by whatever string it
    /// puts in — callers come from `find_call_sites` (relative paths) while
    /// the reverse-import walk used to push absolute ones. One file then got
    /// two rows, one of them absolute, and the definition-file skip compared
    /// relative against absolute so it never fired.
    #[test]
    fn impact_rows_are_all_relative_and_unique() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("target.rs"), "pub fn target_fn() -> i32 { 1 }\n").unwrap();
        std::fs::write(
            root.join("caller.rs"),
            "use target;\npub fn go() { target_fn(); }\n",
        )
        .unwrap();

        let engine = Engine::new(EngineConfig::default());
        engine.index(root);

        let out = find_impact(&engine, root, "target_fn", 20, 0, 1);

        // Every emitted row path must be relative to `root` — an absolute
        // path here means the two halves of the score map disagreed.
        let abs_prefix = root.to_string_lossy().to_string();
        for line in out.lines() {
            assert!(
                !line.contains(&abs_prefix),
                "impact output leaked an absolute path: {line:?}\nfull output:\n{out}"
            );
        }

        // No file may appear twice: one file = one row.
        let mut paths: Vec<&str> = Vec::new();
        for line in out.lines() {
            if let Some(idx) = line.find("  ") {
                let p = line[idx..].trim();
                if !p.is_empty() && !p.starts_with("...") {
                    paths.push(p);
                }
            }
        }
        let mut sorted = paths.clone();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(
            before,
            sorted.len(),
            "impact output has duplicate rows: {paths:?}"
        );
    }
}

#[cfg(test)]
mod flow_budget_tests {
    use super::*;
    use ffs_engine::{Engine, EngineConfig};

    fn engine_for(files: &[(&str, &str)]) -> (tempfile::TempDir, Engine) {
        let tmp = tempfile::tempdir().unwrap();
        for (rel, body) in files {
            let p = tmp.path().join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(p, body).unwrap();
        }
        let engine = Engine::new(EngineConfig::default());
        engine.index(tmp.path());
        (tmp, engine)
    }

    #[test]
    fn flow_lists_callers_and_callees_once_not_per_card() {
        // Two definitions of the same name: the caller/callee lists are keyed
        // on the name, so emitting them per card duplicates them verbatim.
        let (tmp, engine) = engine_for(&[
            ("a.rs", "pub fn dup() {\n    helper();\n}\n"),
            ("b.rs", "pub fn dup() {\n    helper();\n}\n"),
            ("c.rs", "pub fn helper() {}\npub fn caller() { dup(); }\n"),
        ]);
        let out = find_flow(FlowOptions {
            engine: &engine,
            root: tmp.path(),
            name: "dup",
            limit: 10,
            offset: 0,
            callees_top: 5,
            callers_top: 5,
            budget: 10_000,
        });
        assert_eq!(
            out.matches("callers (").count(),
            1,
            "callers header must appear once, got:\n{out}"
        );
        assert_eq!(
            out.matches("callees (").count(),
            1,
            "callees header must appear once, got:\n{out}"
        );
        assert_eq!(
            out.matches("── card ").count(),
            2,
            "both definition cards still present, got:\n{out}"
        );
    }

    #[test]
    fn flow_respects_body_budget() {
        // A definition far larger than the budget must be clipped, with a
        // truncation marker, instead of dumping every line.
        let big_body: String = (0..400).map(|i| format!("    let v{i} = {i};\n")).collect();
        let src = format!("pub fn huge() {{\n{big_body}}}\n");
        let (tmp, engine) = engine_for(&[("big.rs", &src)]);

        let out = find_flow(FlowOptions {
            engine: &engine,
            root: tmp.path(),
            name: "huge",
            limit: 10,
            offset: 0,
            callees_top: 5,
            callers_top: 5,
            budget: 1000,
        });
        assert!(
            out.contains("truncated"),
            "oversized body must be clipped, got {} bytes:\n{out}",
            out.len()
        );
        assert!(
            out.len() < 4000,
            "budget of 1000 should keep output small, got {} bytes",
            out.len()
        );
    }

    #[test]
    fn flow_zero_budget_means_unlimited() {
        let (tmp, engine) = engine_for(&[("a.rs", "pub fn small() {\n    let x = 1;\n}\n")]);
        let out = find_flow(FlowOptions {
            engine: &engine,
            root: tmp.path(),
            name: "small",
            limit: 10,
            offset: 0,
            callees_top: 5,
            callers_top: 5,
            budget: 10_000,
        });
        assert!(out.contains("let x = 1;"), "body should be present:\n{out}");
        assert!(!out.contains("truncated"));
    }
}

#[cfg(test)]
mod token_shape_tests {
    use super::*;
    use ffs_engine::{Engine, EngineConfig};

    fn engine_for(files: &[(&str, &str)]) -> (tempfile::TempDir, Engine) {
        let tmp = tempfile::tempdir().unwrap();
        for (rel, body) in files {
            let p = tmp.path().join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(p, body).unwrap();
        }
        let engine = Engine::new(EngineConfig::default());
        engine.index(tmp.path());
        (tmp, engine)
    }

    #[test]
    fn symbol_hits_omit_internal_weight() {
        let (tmp, engine) = engine_for(&[("a.rs", "pub fn thing() {}\n")]);
        let hits = engine.handles.symbols.lookup_exact("thing");
        let out = format_symbol_hits(&hits, "thing", tmp.path());
        assert!(
            !out.contains("weight"),
            "internal weight must not be emitted, got:\n{out}"
        );
        assert!(out.contains("thing") || out.contains("a.rs"), "got:\n{out}");
    }

    #[test]
    fn refs_omits_weight_and_unresolved_enclosing_placeholder() {
        let (tmp, engine) = engine_for(&[(
            "a.rs",
            "pub fn target() {}\npub fn caller() { target(); }\n",
        )]);
        let r = find_refs(&engine, tmp.path(), "target", 50, 0);
        let out = format_refs_result(&r, tmp.path());
        assert!(
            !out.contains("w="),
            "weight must not be emitted, got:\n{out}"
        );
        assert!(
            !out.contains("(in ?)"),
            "unresolved enclosing must not print a placeholder, got:\n{out}"
        );
    }

    #[test]
    fn flow_output_is_bounded_for_repeated_definitions() {
        // The regression this guards: caller/callee lists are keyed on the
        // symbol name, so N definition cards used to re-print them verbatim.
        let mut files: Vec<(String, String)> = Vec::new();
        for i in 0..6 {
            files.push((
                format!("d{i}.rs"),
                format!("pub fn repeated() {{\n    helper();\n}}\n"),
            ));
        }
        files.push((
            "uses.rs".to_string(),
            "pub fn helper() {}\npub fn c0() { repeated(); }\npub fn c1() { repeated(); }\n"
                .to_string(),
        ));
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        let (tmp, engine) = engine_for(&refs);

        let out = find_flow(FlowOptions {
            engine: &engine,
            root: tmp.path(),
            name: "repeated",
            limit: 10,
            offset: 0,
            callees_top: 5,
            callers_top: 5,
            budget: 10_000,
        });
        assert_eq!(
            out.matches("callers (").count(),
            1,
            "callers must be listed once, not once per card:\n{out}"
        );
        assert_eq!(out.matches("callees (").count(), 1, "callees once:\n{out}");
        assert_eq!(
            out.matches("── card ").count(),
            6,
            "6 cards expected:\n{out}"
        );
    }
}
