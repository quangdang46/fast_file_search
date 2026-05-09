use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use fff_engine::dispatch::DispatchResult;
use fff_engine::Engine;

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
        DispatchResult::SymbolGlob { hits, .. } => (
            "symbol_glob".to_string(),
            hits.into_iter()
                .map(|(n, h)| format!("{} @ {}:{}", n, h.path.to_string_lossy(), h.line))
                .collect(),
        ),
        DispatchResult::FilePath { path, .. } => (
            "file_path".to_string(),
            vec![path.to_string_lossy().to_string()],
        ),
        DispatchResult::Glob { pattern, .. } => ("glob".to_string(), vec![pattern]),
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
