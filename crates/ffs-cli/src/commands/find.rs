use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::commands::pagination::{footer, Page};

#[derive(Debug, Parser)]
pub struct Args {
    /// Path-substring or fuzzy needle.
    pub needle: String,

    /// Maximum results returned in this page.
    #[arg(long, default_value_t = 50)]
    pub limit: usize,

    /// Skip this many results before starting the page.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,

    /// Search in an additional directory. May be given multiple times.
    #[arg(long)]
    pub scope: Vec<PathBuf>,

    /// Force fuzzy / typo-resistant matching from the start (skip exact pass).
    #[arg(long)]
    pub fuzzy: bool,

    /// Disable the zero-match fuzzy fallback. Pure substring only.
    #[arg(long)]
    pub no_fuzzy: bool,
}

#[derive(Debug, Serialize)]
struct FindResult {
    needle: String,
    matches: Vec<String>,
    total: usize,
    offset: usize,
    has_more: bool,
    /// Set when the result came from the fuzzy fallback rather than an exact
    /// substring match. Lets agents reason about confidence.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    fuzzy_fallback: bool,
    schema: &'static str,
}

fn resolve_scopes(scopes: &[PathBuf], root: &Path) -> Vec<PathBuf> {
    if scopes.is_empty() {
        return vec![root.to_path_buf()];
    }
    // `--scope` is documented as "Search in an *additional* directory" — so
    // unions the listed dirs with the working root. Relative paths resolve
    // against `root`; absolute paths pass through unchanged. Duplicates are
    // collapsed in arrival order.
    let mut out: Vec<PathBuf> = Vec::with_capacity(scopes.len() + 1);
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let root_buf = root.to_path_buf();
    if seen.insert(root_buf.clone()) {
        out.push(root_buf);
    }
    for p in scopes {
        let resolved = if p.is_absolute() {
            p.clone()
        } else {
            root.join(p)
        };
        // Skip non-existent scopes silently — the walker would just no-op.
        if !resolved.exists() {
            continue;
        }
        if seen.insert(resolved.clone()) {
            out.push(resolved);
        }
    }
    out
}

pub(crate) fn search_matches(scopes: &[PathBuf], needle: &str) -> Vec<String> {
    let smart_case = needle.chars().any(|c| c.is_uppercase());
    let needle_norm = if smart_case {
        needle.to_string()
    } else {
        needle.to_lowercase()
    };
    let mut seen: HashSet<String> = HashSet::new();
    let mut all: Vec<String> = Vec::new();
    for scope in scopes {
        for p in super::walk_files(scope) {
            let Some(s) = p.to_str() else { continue };
            let hit = if smart_case {
                s.contains(needle_norm.as_str())
            } else {
                s.to_lowercase().contains(needle_norm.as_str())
            };
            if hit && seen.insert(s.to_string()) {
                all.push(s.to_string());
            }
        }
    }
    all.sort();
    all
}

/// SIMD-backed typo-resistant fuzzy search across paths. Uses `neo_frizbee`
/// Smith-Waterman scoring (same algorithm the Neovim picker uses) with
/// `max_typos=Some(2)` so single-character typos like `isntall.sh -> install.sh`
/// still match. Returns results sorted by score (highest first).
///
/// Only basename matches above a per-character minimum are surfaced. Without
/// this guard, long fuzzy queries like `UnifiedScanner` against a repo that
/// has no such filename produce dozens of low-confidence "matches" with score
/// near zero.
pub(crate) fn fuzzy_search_matches(scopes: &[PathBuf], needle: &str) -> Vec<String> {
    let mut all_paths: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for scope in scopes {
        for p in super::walk_files(scope) {
            if let Some(s) = p.to_str() {
                if seen.insert(s.to_string()) {
                    all_paths.push(s.to_string());
                }
            }
        }
    }

    if all_paths.is_empty() {
        return Vec::new();
    }

    let haystacks: Vec<&str> = all_paths.iter().map(String::as_str).collect();
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .min(8);
    let config = neo_frizbee::Config {
        max_typos: Some(2),
        sort: true,
        ..Default::default()
    };
    let matches = neo_frizbee::match_list_parallel(needle, &haystacks, &config, threads);

    // Tighten fuzzy fallback (bug 5). We only keep matches that either
    //   (a) contain a substantial run of the needle's characters in order
    //       in the basename, or
    //   (b) score within ~50% of the top match.
    // Otherwise long needles like `UnifiedScanner` against a repo with no
    // such filename produce dozens of low-confidence dumps.
    let needle_lower = needle.to_lowercase();
    let needle_len = needle_lower.chars().count();
    let min_run = ((needle_len as f64) * 0.5).ceil() as usize;
    let min_run = min_run.max(3);

    let top_score = matches.iter().map(|m| m.score).max().unwrap_or(0);
    let score_floor = (top_score as f64 * 0.5) as u16;

    matches
        .into_iter()
        .filter_map(|m| {
            let path = all_paths.get(m.index as usize)?;
            let basename = std::path::Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(path)
                .to_lowercase();
            let run = longest_in_order_run(&basename, &needle_lower);
            if run >= min_run || (top_score > 0 && m.score >= score_floor) {
                Some(path.clone())
            } else {
                None
            }
        })
        .collect()
}

/// Length of the longest contiguous prefix of `needle` whose characters
/// appear (in order, case-insensitive) inside `haystack`. Used as a coarse
/// confidence floor for fuzzy matches.
fn longest_in_order_run(haystack: &str, needle: &str) -> usize {
    let mut best = 0usize;
    let h: Vec<char> = haystack.chars().collect();
    let n: Vec<char> = needle.chars().collect();
    if n.is_empty() {
        return 0;
    }
    for start in 0..h.len() {
        let mut hi = start;
        let mut ni = 0usize;
        while hi < h.len() && ni < n.len() {
            if h[hi] == n[ni] {
                ni += 1;
            }
            hi += 1;
        }
        if ni > best {
            best = ni;
        }
    }
    best
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    // Bug 6: empty needle has no useful semantics for find — reject rather
    // than silently dumping the whole repo.
    if args.needle.is_empty() {
        return Err(anyhow::anyhow!(
            "ffs find: needle is empty; pass a non-empty path substring"
        ));
    }

    let resolved_scopes = resolve_scopes(&args.scope, root);

    let (all, fuzzy_fallback) = if args.fuzzy {
        (fuzzy_search_matches(&resolved_scopes, &args.needle), true)
    } else {
        let exact = search_matches(&resolved_scopes, &args.needle);
        if exact.is_empty() && !args.no_fuzzy {
            (fuzzy_search_matches(&resolved_scopes, &args.needle), true)
        } else {
            (exact, false)
        }
    };

    let page = Page::paginate(all, args.offset, args.limit);
    let payload = FindResult {
        needle: args.needle,
        total: page.total,
        offset: page.offset,
        has_more: page.has_more,
        matches: page.items,
        fuzzy_fallback,
        schema: "v1",
    };
    super::emit(format, &payload, |p| {
        let mut out = String::new();
        if p.fuzzy_fallback && !p.matches.is_empty() {
            out.push_str("# fuzzy fallback (no exact match)\n");
        }
        for m in &p.matches {
            out.push_str(m);
            out.push('\n');
        }
        if p.total > 0 {
            out.push_str(&footer(p.total, p.offset, p.matches.len(), p.has_more));
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_scope_matches_paths() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        std::fs::write(root.join("foo.rs"), "x").unwrap();
        std::fs::write(root.join("bar.rs"), "y").unwrap();

        let args = Args {
            needle: "foo".into(),
            limit: 50,
            offset: 0,
            scope: vec![],
            fuzzy: false,
            no_fuzzy: false,
        };
        run(args, root, OutputFormat::Json).unwrap();
        // Json emit writes to stdout; just assert it doesn't panic.
    }

    #[test]
    fn multi_scope_dedups() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        let a = root.join("a");
        let b = root.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("shared.rs"), "x").unwrap();
        std::fs::write(b.join("unique.rs"), "y").unwrap();

        let scopes = resolve_scopes(&[a, b], root);
        let hits_shared = search_matches(&scopes, "shared");
        let hits_unique = search_matches(&scopes, "unique");
        assert_eq!(hits_shared.len(), 1);
        assert_eq!(hits_unique.len(), 1);
    }

    #[test]
    fn relative_scope_bound_to_root() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/inside.rs"), "x").unwrap();

        // Relative scope: "src" resolves under root and is added on top of
        // the root scope, so the union is `[root, root/src]`.
        let scopes = resolve_scopes(&[PathBuf::from("src")], root);
        assert_eq!(scopes, vec![root.to_path_buf(), root.join("src")]);
        let hits = search_matches(&scopes, "inside");
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn absolute_scope_outside_root_unions_with_root() {
        let td = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = td.path();
        // Absolute scope outside root → unioned with root, not silently dropped.
        let scopes = resolve_scopes(&[outside.path().to_path_buf()], root);
        assert_eq!(
            scopes,
            vec![root.to_path_buf(), outside.path().to_path_buf()]
        );
    }

    #[test]
    fn nonexistent_scope_silently_dropped() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        let scopes = resolve_scopes(&[PathBuf::from("/no/such/path/xyz")], root);
        assert_eq!(scopes, vec![root.to_path_buf()]);
    }
}
