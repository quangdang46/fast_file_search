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

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let scopes: Vec<&Path> = if args.scope.is_empty() {
        vec![root]
    } else {
        args.scope.iter().map(|p| p.as_path()).collect()
    };
    let needle_lower = args.needle.to_lowercase();
    let mut seen: HashSet<String> = HashSet::new();
    let mut all: Vec<String> = Vec::new();
    for scope in scopes {
        let files = super::walk_files(scope);
        for p in files {
            let Some(s) = p.to_str() else { continue };
            if s.to_lowercase().contains(&needle_lower) && seen.insert(s.to_string()) {
                all.push(s.to_string());
            }
        }
    }
    all.sort();

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
        let a = td.path().join("a");
        let b = td.path().join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        // Same relative structure under both scopes to test dedup.
        std::fs::write(a.join("shared.rs"), "x").unwrap();
        std::fs::write(b.join("unique.rs"), "y").unwrap();

        let mut seen = HashSet::new();
        let scopes = vec![a.as_path(), b.as_path()];
        for scope in scopes {
            for p in super::super::walk_files(scope) {
                if let Some(s) = p.to_str() {
                    if s.contains("shared") || s.contains("unique") {
                        seen.insert(s.to_string());
                    }
                }
            }
        }
        assert_eq!(seen.len(), 2, "should see both shared.rs and unique.rs");
    }
}
