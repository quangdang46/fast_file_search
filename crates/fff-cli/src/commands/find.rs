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
}

#[derive(Debug, Serialize)]
struct FindResult {
    needle: String,
    matches: Vec<String>,
    total: usize,
    offset: usize,
    has_more: bool,
}

fn resolve_scopes(scopes: &[PathBuf], root: &Path) -> Vec<PathBuf> {
    if scopes.is_empty() {
        return vec![root.to_path_buf()];
    }
    // Relative scopes resolve against `root`; absolutes pass through; any
    // scope that escapes `root` after resolution falls back to `root`.
    scopes
        .iter()
        .map(|p| {
            let resolved = if p.is_absolute() {
                p.clone()
            } else {
                root.join(p)
            };
            if resolved.starts_with(root) {
                resolved
            } else {
                root.to_path_buf()
            }
        })
        .collect()
}

pub(crate) fn search_matches(scopes: &[PathBuf], needle: &str) -> Vec<String> {
    let needle_lower = needle.to_lowercase();
    let mut seen: HashSet<String> = HashSet::new();
    let mut all: Vec<String> = Vec::new();
    for scope in scopes {
        for p in super::walk_files(scope) {
            let Some(s) = p.to_str() else { continue };
            if s.to_lowercase().contains(&needle_lower) && seen.insert(s.to_string()) {
                all.push(s.to_string());
            }
        }
    }
    all.sort();
    all
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let resolved_scopes = resolve_scopes(&args.scope, root);
    let all = search_matches(&resolved_scopes, &args.needle);

    let page = Page::paginate(all, args.offset, args.limit);
    let payload = FindResult {
        needle: args.needle,
        total: page.total,
        offset: page.offset,
        has_more: page.has_more,
        matches: page.items,
    };
    super::emit(format, &payload, |p| {
        let mut out = String::new();
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

        // Relative scope: "src" resolves under root.
        let scopes = resolve_scopes(&[PathBuf::from("src")], root);
        assert_eq!(scopes, vec![root.join("src")]);
        let hits = search_matches(&scopes, "inside");
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn absolute_scope_outside_root_falls_back() {
        let td = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = td.path();
        // Absolute scope outside root → falls back to root.
        let scopes = resolve_scopes(&[outside.path().to_path_buf()], root);
        assert_eq!(scopes, vec![root.to_path_buf()]);
    }
}
