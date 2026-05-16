use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use ffs_engine::dispatch::DispatchResult;
use ffs_engine::Engine;

use crate::cli::OutputFormat;

#[derive(Debug, Parser)]
pub struct Args {
    /// Free-form query — engine classifies and routes it.
    pub query: String,
}

#[derive(Debug, Serialize)]
struct DispatchOutput {
    raw: String,
    classification: String,
    summary: Vec<String>,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    // Bug 10: empty query has no useful semantics — reject up-front.
    let q = args.query.trim();
    if q.is_empty() {
        return Err(anyhow::anyhow!(
            "ffs dispatch: query is empty; pass a non-empty term"
        ));
    }

    let engine = Engine::default();
    engine.index(root);

    let result = engine.dispatch(&args.query, root);
    let (classification, summary) = match result {
        DispatchResult::Symbol { hits, .. } => (
            "symbol".to_string(),
            hits.into_iter()
                .map(|h| format!("{}:{}", h.path.to_string_lossy(), h.line))
                .collect(),
        ),
        DispatchResult::SymbolGlob { hits, classified } => {
            // Bug 9: when the symbol-prefix lookup is empty, hand off to the
            // filename glob backend so patterns like `*.py` actually return
            // matching files instead of an empty summary.
            if hits.is_empty() {
                let pattern = match &classified.query {
                    ffs_symbol::types::QueryType::SymbolGlob(p) => p.clone(),
                    _ => args.query.clone(),
                };
                let mut paths = run_glob(root, &pattern);
                paths.sort();
                ("symbol_glob".to_string(), paths)
            } else {
                (
                    "symbol_glob".to_string(),
                    hits.into_iter()
                        .map(|(n, h)| {
                            format!("{} @ {}:{}", n, h.path.to_string_lossy(), h.line)
                        })
                        .collect(),
                )
            }
        }
        DispatchResult::FilePath { path, .. } => (
            "file_path".to_string(),
            vec![path.to_string_lossy().to_string()],
        ),
        DispatchResult::Glob { pattern, .. } => {
            // Bug 9: classifying as glob and then doing nothing is useless.
            // Hand off to the glob backend and surface real matches.
            let mut paths = run_glob(root, &pattern);
            paths.sort();
            ("glob".to_string(), paths)
        }
        DispatchResult::ContentFallback { .. } => (
            "content_fallback".to_string(),
            vec!["[fallback to grep]".to_string()],
        ),
    };

    let payload = DispatchOutput {
        raw: args.query,
        classification,
        summary,
    };
    super::emit(format, &payload, |p| {
        let mut out = format!("classified as: {}\n", p.classification);
        for line in &p.summary {
            out.push_str(line);
            out.push('\n');
        }
        out
    })
}

fn run_glob(root: &Path, pattern: &str) -> Vec<String> {
    // Bare extension globs like `*.py` are usually meant as "anywhere in the
    // tree"; expand them to `**/*.py` so the dispatch result matches user
    // expectations. Patterns that already contain `/` or `**` pass through.
    let expanded = if !pattern.contains('/') && pattern.starts_with('*') {
        format!("**/{}", pattern)
    } else {
        pattern.to_string()
    };

    let mut builder = ignore::overrides::OverrideBuilder::new(root);
    if builder.add(&expanded).is_err() {
        return Vec::new();
    }
    let Ok(overrides) = builder.build() else {
        return Vec::new();
    };

    ignore::WalkBuilder::new(root)
        .overrides(overrides)
        .standard_filters(true)
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter_map(|e| e.path().to_str().map(|s| s.to_string()))
        .take(200)
        .collect()
}
