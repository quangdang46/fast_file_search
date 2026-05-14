//! `ffs impact <symbol>` — rank workspace files by how much each one would
//! be affected if `<symbol>` changed. Combines three signals per file:
//!
//! * direct callers (single-hop call sites)        — weight 3
//! * reverse-import edges to the symbol's defn(s)  — weight 2
//! * transitive callers (BFS depth 2 + 3 hits)     — weight 1
//!
//! Lock zones: only `crates/ffs-cli` is touched. The command is purely
//! additive (new sub-command, no changes to existing ones).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use clap::Parser;
use ignore::WalkBuilder;
use serde::Serialize;

use ffs_engine::Engine;
use ffs_symbol::lang::detect_file_type;
use ffs_symbol::types::{FileType, Lang};

use crate::cli::OutputFormat;
use crate::commands::callers_bfs::{run_bfs, BfsConfig, CandidateFile};
use crate::commands::deps_resolve::{extract_imports, resolve_import};
use crate::commands::pagination::{footer, Page};

#[derive(Debug, Parser)]
pub struct Args {
    /// Symbol name (function / method / class / …) to score impact for.
    pub name: String,

    /// Maximum rows returned in this page.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,

    /// Skip this many rows before starting the page.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,

    /// How far the callers BFS walks. 1 disables the transitive signal;
    /// 3 (default) captures fan-in two hops away. Capped at 3.
    #[arg(long, default_value_t = 3)]
    pub hops: u32,

    /// Stop propagating from any single name that produces more than this
    /// many hits in one hop. Mirrors `ffs callers --hub-guard`.
    #[arg(long, default_value_t = 50)]
    pub hub_guard: usize,
}

#[derive(Debug, Serialize)]
struct ImpactResult {
    path: String,
    score: u32,
    reasons: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ImpactOutput {
    name: String,
    hops: u32,
    hub_guard: usize,
    results: Vec<ImpactResult>,
    total: usize,
    offset: usize,
    has_more: bool,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let engine = Engine::default();
    engine.index(&root_canon);

    let files = super::walk_files(&root_canon);
    let candidates: Vec<CandidateFile> = files
        .iter()
        .filter_map(|path| {
            let meta = std::fs::metadata(path).ok()?;
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let content = std::fs::read_to_string(path).ok()?;
            let lang = match detect_file_type(path) {
                FileType::Code(l) => Some(l),
                _ => None,
            };
            Some(CandidateFile {
                path: path.clone(),
                mtime,
                content,
                lang,
            })
        })
        .collect();

    let definitions = engine.handles.symbols.lookup_exact(&args.name);
    let defn_paths: BTreeSet<PathBuf> = definitions
        .iter()
        .map(|d| d.path.canonicalize().unwrap_or_else(|_| d.path.clone()))
        .collect();

    let hops = args.hops.clamp(1, 3);
    let bfs = run_bfs(
        &engine,
        &args.name,
        &candidates,
        BfsConfig {
            max_hops: hops,
            hub_guard: args.hub_guard,
            skip_hubs: String::new(),
        },
        &root_canon,
    );

    let mut direct: HashMap<String, u32> = HashMap::new();
    let mut transitive: HashMap<String, u32> = HashMap::new();
    for hit in &bfs.hits {
        let path = display_relative(Path::new(&hit.path), &root_canon);
        if hit.depth <= 1 {
            *direct.entry(path).or_default() += 1;
        } else {
            *transitive.entry(path).or_default() += 1;
        }
    }

    let imports = reverse_imports(&root_canon, &defn_paths);

    // Union of all paths with any signal.
    let mut paths: BTreeSet<String> = BTreeSet::new();
    paths.extend(direct.keys().cloned());
    paths.extend(transitive.keys().cloned());
    paths.extend(imports.keys().cloned());
    // The symbol's own defn file is not "impacted by" the symbol — drop it.
    for d in &defn_paths {
        paths.remove(&display_relative(d, &root_canon));
    }

    let mut rows: Vec<ImpactResult> = paths
        .into_iter()
        .map(|path| {
            let d = *direct.get(&path).unwrap_or(&0);
            let i = *imports.get(&path).unwrap_or(&0);
            let t = *transitive.get(&path).unwrap_or(&0);
            let score = d * 3 + i * 2 + t;
            let mut reasons = Vec::new();
            if d > 0 {
                reasons.push(format!("direct: {d}"));
            }
            if i > 0 {
                reasons.push(format!("imports: {i}"));
            }
            if t > 0 {
                reasons.push(format!("transitive: {t}"));
            }
            ImpactResult {
                path,
                score,
                reasons,
            }
        })
        .filter(|r| r.score > 0)
        .collect();
    rows.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));

    let page = Page::paginate(rows, args.offset, args.limit);
    let payload = ImpactOutput {
        name: args.name.clone(),
        hops,
        hub_guard: args.hub_guard,
        total: page.total,
        offset: page.offset,
        has_more: page.has_more,
        results: page.items,
    };
    super::emit(format, &payload, render_text)
}

// Count, per file in the workspace, how many of its imports resolve to one of
// the symbol's definition files. Skip the defn files themselves so they don't
// self-impact.
fn reverse_imports(root: &Path, defn_paths: &BTreeSet<PathBuf>) -> BTreeMap<String, u32> {
    let mut out: BTreeMap<String, u32> = BTreeMap::new();
    if defn_paths.is_empty() {
        return out;
    }
    for entry in WalkBuilder::new(root)
        .standard_filters(true)
        .hidden(false)
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.into_path();
        let path_canon = path.canonicalize().unwrap_or_else(|_| path.clone());
        if defn_paths.contains(&path_canon) {
            continue;
        }
        let Some(lang) = code_lang(&path_canon) else {
            continue;
        };
        let Ok(content) = std::fs::read_to_string(&path_canon) else {
            continue;
        };
        let mut count = 0u32;
        for spec in extract_imports(&content, lang) {
            if let Some(resolved) = resolve_import(&spec, &path_canon, root, lang) {
                let resolved = resolved.canonicalize().unwrap_or(resolved);
                if defn_paths.contains(&resolved) {
                    count += 1;
                }
            }
        }
        if count > 0 {
            out.insert(display_relative(&path_canon, root), count);
        }
    }
    out
}

fn code_lang(path: &Path) -> Option<Lang> {
    match detect_file_type(path) {
        FileType::Code(l) => Some(l),
        _ => None,
    }
}

fn display_relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn render_text(p: &ImpactOutput) -> String {
    let mut out = String::new();
    if p.total == 0 {
        out.push_str(&format!("[no impact found for {}]\n", p.name));
        return out;
    }
    for r in &p.results {
        let reasons = if r.reasons.is_empty() {
            String::new()
        } else {
            format!("  ({})", r.reasons.join(", "))
        };
        out.push_str(&format!("{:>5}  {}{}\n", r.score, r.path, reasons));
    }
    out.push_str(&footer(p.total, p.offset, p.results.len(), p.has_more));
    out
}
