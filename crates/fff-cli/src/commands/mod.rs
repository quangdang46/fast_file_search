pub mod callees;
pub mod callers;
pub(crate) mod dedup;
pub mod dispatch;
pub(crate) mod facets;
pub mod find;
pub mod glob;
pub mod grep;
pub mod index;
pub mod mcp;
pub mod outline;
pub(crate) mod outline_format;
pub mod overview;
pub(crate) mod pagination;
pub mod read;
pub mod siblings;
pub mod symbol;

use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::cli::OutputFormat;

/// Helper: emit either a human text rendering or JSON.
pub(crate) fn emit<T, R>(format: OutputFormat, payload: &T, render_text: R) -> Result<()>
where
    T: Serialize,
    R: FnOnce(&T) -> String,
{
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(payload)?);
        }
        OutputFormat::Text => {
            print!("{}", render_text(payload));
        }
    }
    Ok(())
}

/// Walk all files under `root` honoring `.gitignore` and return their paths.
pub(crate) fn walk_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .standard_filters(true)
        .follow_links(false)
        .build();
    for entry in walker.flatten() {
        if let Some(ft) = entry.file_type() {
            if ft.is_file() {
                out.push(entry.into_path());
            }
        }
    }
    out
}
