//! `scry deps <path>` — list a file's imports (raw + resolved) and the files
//! that depend on it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use clap::Parser;
use ignore::WalkBuilder;
use serde::Serialize;

use fff_symbol::lang::detect_file_type;
use fff_symbol::types::FileType;

use crate::cli::OutputFormat;
use crate::commands::deps_resolve::{extract_imports, resolve_import};
use crate::commands::pagination::{footer, Page};

#[derive(Debug, Parser)]
pub struct Args {
    /// File to analyse, relative to --root.
    pub target: String,

    /// Maximum dependents returned in this page.
    #[arg(long, default_value_t = 100)]
    pub limit: usize,

    /// Skip this many dependents before starting the page.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,

    /// Skip the dependents walk entirely. Imports still resolve.
    #[arg(long, default_value_t = false)]
    pub no_dependents: bool,
}

#[derive(Debug, Serialize)]
struct ImportEntry {
    spec: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved: Option<String>,
}

#[derive(Debug, Serialize)]
struct DependentEntry {
    path: String,
    spec: String,
}

#[derive(Debug, Serialize)]
struct DepsOutput {
    target: String,
    imports: Vec<ImportEntry>,
    dependents: Vec<DependentEntry>,
    total_dependents: usize,
    offset: usize,
    has_more: bool,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let target = if Path::new(&args.target).is_absolute() {
        PathBuf::from(&args.target)
    } else {
        root.join(&args.target)
    };
    if !target.is_file() {
        return Err(anyhow!("target is not a file: {}", target.display()));
    }
    let target_canon = target.canonicalize().unwrap_or_else(|_| target.clone());

    let imports = imports_for_file(&target_canon, &root);

    let mut dependents: Vec<DependentEntry> = Vec::new();
    if !args.no_dependents {
        let target_for_match = target_canon.clone();
        for path in walk_workspace(&root) {
            if path == target_for_match {
                continue;
            }
            for spec in raw_imports_for(&path) {
                if let Some(file_lang) = code_lang(&path) {
                    if let Some(resolved) = resolve_import(&spec, &path, &root, file_lang) {
                        let resolved = resolved.canonicalize().unwrap_or(resolved);
                        if resolved == target_for_match {
                            dependents.push(DependentEntry {
                                path: display_relative(&path, &root),
                                spec,
                            });
                            break;
                        }
                    }
                }
            }
        }
    }

    let page = Page::paginate(dependents, args.offset, args.limit);
    let payload = DepsOutput {
        target: display_relative(&target_canon, &root),
        imports,
        dependents: page.items,
        total_dependents: page.total,
        offset: page.offset,
        has_more: page.has_more,
    };
    super::emit(format, &payload, render_text)
}

fn imports_for_file(path: &Path, root: &Path) -> Vec<ImportEntry> {
    let Some(lang) = code_lang(path) else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let raw = extract_imports(&content, lang);
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for spec in raw {
        if !seen.insert(spec.clone()) {
            continue;
        }
        let resolved = resolve_import(&spec, path, root, lang).map(|p| {
            let canon = p.canonicalize().unwrap_or(p);
            display_relative(&canon, root)
        });
        out.push(ImportEntry { spec, resolved });
    }
    out
}

fn raw_imports_for(path: &Path) -> Vec<String> {
    let Some(lang) = code_lang(path) else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    extract_imports(&content, lang)
}

fn code_lang(path: &Path) -> Option<fff_symbol::types::Lang> {
    match detect_file_type(path) {
        FileType::Code(l) => Some(l),
        _ => None,
    }
}

fn walk_workspace(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in WalkBuilder::new(root)
        .standard_filters(true)
        .hidden(false)
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let p = entry.into_path();
        if matches!(detect_file_type(&p), FileType::Code(_)) {
            out.push(p.canonicalize().unwrap_or(p));
        }
    }
    out
}

fn display_relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn render_text(p: &DepsOutput) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n", p.target));

    if p.imports.is_empty() {
        out.push_str("[no imports]\n");
    } else {
        out.push_str("## imports\n");
        for imp in &p.imports {
            match &imp.resolved {
                Some(r) => out.push_str(&format!("  {} -> {}\n", imp.spec, r)),
                None => out.push_str(&format!("  {} (unresolved)\n", imp.spec)),
            }
        }
    }

    if p.dependents.is_empty() && p.total_dependents == 0 {
        out.push_str("## dependents\n[none]\n");
    } else {
        out.push_str("## dependents\n");
        for d in &p.dependents {
            out.push_str(&format!("  {} (via {})\n", d.path, d.spec));
        }
        out.push_str(&footer(
            p.total_dependents,
            p.offset,
            p.dependents.len(),
            p.has_more,
        ));
    }
    out
}
