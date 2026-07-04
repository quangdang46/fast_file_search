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
use ffs_symbol::symbol_index::SymbolLocation;
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
    /// Token budget for body excerpts (default 10000).
    #[allow(dead_code)]
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
pub struct EngineOutlineParams {
    /// Path to the file whose structural outline should be rendered.
    pub path: String,
    /// Rendering style: "agent" (default), "markdown", "structured", or "tabular".
    #[allow(dead_code)]
    pub style: Option<String>,
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

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EngineOverviewParams {
    /// How many language buckets to include (default 10).
    #[serde(rename = "topLanguages")]
    pub top_languages: Option<f64>,
    /// How many of the most-defined symbol names to include (default 15).
    #[serde(rename = "topSymbols")]
    pub top_symbols: Option<f64>,
    /// How many entry-point candidates to surface (default 10).
    #[serde(rename = "topEntrypoints")]
    pub top_entrypoints: Option<f64>,
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
        // Only scan code files — matches CLI's walk_files which filters by
        // detect_file_type() == Code(_). Skipping non-code files avoids binary
        // bloat and makes callers/refs consistent with CLI output.
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
        let Ok(content) = ffs::bom::read_file(&def.path) else {
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

/// Walk all code files under `root` with standard .gitignore filters.
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
    let definitions: Vec<SymbolLocation> = engine.handles.symbols.lookup_exact(name);
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
        let path_str = path.to_string_lossy().to_string();
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
            if definition_line_set.contains(&(path_str.clone(), lineno)) {
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
        let path_str = path.to_string_lossy().to_string();
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
pub fn format_refs_result(r: &RefsResult) -> String {
    let mut out = String::new();
    out.push_str(&format!("Symbol: {}\n", r.name));
    out.push_str(&format!("Definitions ({}):\n", r.definitions.len()));
    if r.definitions.is_empty() {
        out.push_str("  [none]\n");
    } else {
        for d in &r.definitions {
            out.push_str(&format!(
                "  {}:{} ({}, w={})\n",
                d.path.display(),
                d.line,
                d.kind,
                d.weight,
            ));
        }
    }

    out.push_str(&format!("\nUsages ({}):\n", r.total_usages));
    if r.total_usages == 0 {
        out.push_str("  [none]\n");
    } else {
        for u in &r.usages {
            let encl = u.enclosing.as_deref().unwrap_or("?");
            out.push_str(&format!(
                "  {}:{} (in {}): {}\n",
                u.path, u.line, encl, u.text,
            ));
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

/// Format the outline for a file. Returns agent-friendly text (header + tree).
/// Used in-process instead of subprocess for `ffs outline`.
pub fn format_outline(path: &std::path::Path) -> Result<String, String> {
    let ft = ffs_symbol::lang::detect_file_type(path);
    let lang = match ft {
        ffs_symbol::types::FileType::Code(l) => l,
        _ => return Err(format!("not a code file: {}", path.display())),
    };
    let content =
        ffs::bom::read_file(path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let entries = ffs_symbol::outline::get_outline_entries(&content, lang);
    let total_lines = content.lines().count();
    let total_tokens = ffs_symbol::types::estimate_tokens(content.len() as u64);

    let mut out = String::new();
    out.push_str(&format!(
        "# {} ({} lines, ~{} tokens) [outline]\n\n",
        path.display(),
        total_lines,
        total_tokens,
    ));
    if entries.is_empty() {
        out.push_str("(no symbols)\n");
    } else {
        for e in &entries {
            render_outline_entry(&mut out, e, 0);
        }
    }
    Ok(out)
}

fn render_outline_entry(out: &mut String, entry: &ffs_symbol::types::OutlineEntry, depth: usize) {
    let indent = "  ".repeat(depth);
    out.push_str(&format!(
        "{indent}[{}-{}] {} {}\n",
        entry.start_line,
        entry.end_line,
        format_outline_kind(entry.kind),
        entry.name,
    ));
    for child in &entry.children {
        render_outline_entry(out, child, depth + 1);
    }
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
    }
}
pub fn find_siblings(
    engine: &Engine,
    _root: &Path,
    name: &str,
    _include_imports: bool,
    limit: usize,
    offset: usize,
) -> String {
    let definitions = engine.handles.symbols.lookup_exact(name);
    let mut out = String::new();
    if definitions.is_empty() {
        out.push_str(&format!("[no definitions found for `{name}`]\n"));
        return out;
    }
    for (idx, def) in definitions.iter().enumerate() {
        out.push_str(&format!(
            "definition {}/{}: {}:{} ({} , w={})\n",
            idx + 1,
            definitions.len(),
            def.path.display(),
            def.line,
            def.kind,
            def.weight,
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

/// High-signal workspace summary. Mirrors `ffs overview`.
pub fn format_overview(
    engine: &Engine,
    root: &Path,
    top_languages: usize,
    top_symbols: usize,
    _top_entrypoints: usize,
) -> String {
    use ignore::WalkBuilder;
    use std::collections::BTreeMap;

    let mut lang_counts: BTreeMap<String, u32> = BTreeMap::new();
    let mut total_files = 0u32;
    let mut total_bytes = 0u64;

    for entry in WalkBuilder::new(root)
        .standard_filters(true)
        .follow_links(false)
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        total_files += 1;
        let path = entry.into_path();
        let ft = ffs_symbol::lang::detect_file_type(&path);
        let kind = match ft {
            ffs_symbol::types::FileType::Code(l) => format!("{l:?}"),
            _ => format!("{ft:?}"),
        };
        *lang_counts.entry(kind).or_insert(0) += 1;
        if let Ok(meta) = std::fs::metadata(&path) {
            total_bytes += meta.len();
        }
    }

    let mut out = String::new();
    let total_tokens = ffs_symbol::types::estimate_tokens(total_bytes);
    out.push_str(&format!(
        "# {} · {} files · ~{} tokens\n\n",
        root.display(),
        total_files,
        total_tokens,
    ));

    // Languages
    out.push_str(&format!("## Languages (top {top_languages})\n"));
    let mut lang_sorted: Vec<_> = lang_counts.into_iter().collect();
    lang_sorted.sort_by_key(|k| std::cmp::Reverse(k.1));
    for (lang, count) in lang_sorted.iter().take(top_languages) {
        out.push_str(&format!("  {lang}: {count} files\n"));
    }

    // Top symbols from engine
    if top_symbols > 0 {
        out.push_str(&format!("\n## Top-defined symbols (top {top_symbols})\n"));
        let all = engine.handles.symbols.lookup_prefix("");
        // Group by name, sum weights
        use std::collections::HashMap;
        let mut by_name: HashMap<&str, u16> = HashMap::new();
        for (name, loc) in &all {
            *by_name.entry(name).or_insert(0) += loc.weight;
        }
        let mut sorted: Vec<_> = by_name.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        for (name, weight) in sorted.iter().take(top_symbols) {
            out.push_str(&format!("  {} (w={})\n", name, weight));
        }
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

/// Simplified flow: definitions + body + callees + callers per definition.
pub fn find_flow(
    engine: &Engine,
    root: &Path,
    name: &str,
    limit: usize,
    offset: usize,
    callees_top: usize,
    callers_top: usize,
) -> String {
    let definitions = engine.handles.symbols.lookup_exact(name);
    let mut out = String::new();
    if definitions.is_empty() {
        out.push_str(&format!("[no definitions for `{name}`]\n"));
        return out;
    }

    let total = definitions.len();
    let page: Vec<_> = definitions.into_iter().skip(offset).take(limit).collect();
    let mut callers_cache: Option<Vec<CallHit>> = None;
    let mut callees_cache: Option<Vec<CallHit>> = None;

    for (idx, def) in page.iter().enumerate() {
        let card_idx = offset + idx + 1;
        out.push_str(&format!(
            "── card {card_idx}/{total}: {name} @ {}:{} ({}, w={}) ──\n",
            def.path.display(),
            def.line,
            def.kind,
            def.weight,
        ));

        // Body excerpt
        if let Ok(content) = ffs::bom::read_file(&def.path) {
            let start = def.line.saturating_sub(1) as usize;
            let end = (def.end_line as usize).min(content.lines().count());
            out.push_str(&format!("body [{}..{}]:\n", def.line, end));
            for (i, line) in content.lines().enumerate().skip(start).take(end - start) {
                out.push_str(&format!("  {:>4}: {line}\n", i + 1));
            }
        }

        // Callees
        let callees =
            callees_cache.get_or_insert_with(|| find_callee_sites(engine, root, name, callees_top));
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

        // Callers
        let callers = callers_cache.get_or_insert_with(|| {
            // Use all callers (no limit for caching), then truncate
            find_call_sites(engine, root, name, callers_top.max(50))
        });
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
    let definitions = engine.handles.symbols.lookup_exact(name);
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

    // Reverse imports: files that import files containing the symbol
    let def_paths: std::collections::HashSet<String> = definitions
        .iter()
        .map(|d| d.path.to_string_lossy().to_string())
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
        let path_str = path.to_string_lossy().to_string();
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
}
