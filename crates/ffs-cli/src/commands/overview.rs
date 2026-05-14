//! `ffs overview` — high-signal summary of the workspace for agents.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::SystemTime;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use ffs_engine::Engine;
use ffs_symbol::lang::detect_file_type;
use ffs_symbol::types::{FileType, Lang};

use crate::cli::OutputFormat;

#[derive(Debug, Parser)]
pub struct Args {
    /// How many language buckets to include in the breakdown.
    #[arg(long, default_value_t = 10)]
    pub top_languages: usize,

    /// How many of the most-defined symbol names to include.
    #[arg(long, default_value_t = 15)]
    pub top_symbols: usize,

    /// How many entry-point candidates to surface.
    #[arg(long, default_value_t = 10)]
    pub top_entrypoints: usize,
}

#[derive(Debug, Serialize)]
struct LanguageStat {
    lang: String,
    files: usize,
    code_lines: usize,
    bytes: u64,
}

#[derive(Debug, Serialize)]
struct SymbolStat {
    name: String,
    definitions: usize,
}

#[derive(Debug, Serialize)]
struct OverviewOutput {
    root: String,
    files: usize,
    code_files: usize,
    code_lines: usize,
    bytes: u64,
    fingerprint: String,
    languages: Vec<LanguageStat>,
    build_files: Vec<String>,
    entrypoints: Vec<String>,
    top_symbols: Vec<SymbolStat>,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    let engine = Engine::default();
    engine.index(&root);

    let files = super::walk_files(&root);

    let mut total_files = 0usize;
    let mut code_files = 0usize;
    let mut total_lines = 0usize;
    let mut total_bytes = 0u64;
    let mut lang_buckets: HashMap<Lang, LangBucket> = HashMap::new();
    let mut build_files: Vec<String> = Vec::new();
    let mut entrypoints: Vec<String> = Vec::new();
    let mut fingerprint_input: Vec<(String, u64, SystemTime)> = Vec::new();

    for path in &files {
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        let size = meta.len();
        total_bytes += size;
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        total_files += 1;

        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if is_build_file(name) {
                build_files.push(rel.clone());
            }
            if is_entrypoint(name, &rel) {
                entrypoints.push(rel.clone());
            }
        }

        if let FileType::Code(lang) = detect_file_type(path) {
            code_files += 1;
            let lines = std::fs::read_to_string(path)
                .map(|c| c.lines().count())
                .unwrap_or(0);
            total_lines += lines;
            let bucket = lang_buckets.entry(lang).or_default();
            bucket.files += 1;
            bucket.code_lines += lines;
            bucket.bytes += size;
        }

        fingerprint_input.push((rel, size, mtime));
    }

    let mut languages: Vec<LanguageStat> = lang_buckets
        .into_iter()
        .map(|(lang, b)| LanguageStat {
            lang: format!("{lang:?}"),
            files: b.files,
            code_lines: b.code_lines,
            bytes: b.bytes,
        })
        .collect();
    languages.sort_by(|a, b| b.code_lines.cmp(&a.code_lines).then(a.lang.cmp(&b.lang)));
    languages.truncate(args.top_languages);

    build_files.sort();
    entrypoints.sort();
    entrypoints.truncate(args.top_entrypoints);

    let top_symbols = collect_top_symbols(&engine, args.top_symbols);

    let fingerprint = fingerprint(&fingerprint_input);

    let payload = OverviewOutput {
        root: root.to_string_lossy().to_string(),
        files: total_files,
        code_files,
        code_lines: total_lines,
        bytes: total_bytes,
        fingerprint,
        languages,
        build_files,
        entrypoints,
        top_symbols,
    };
    super::emit(format, &payload, render_text)
}

#[derive(Default)]
struct LangBucket {
    files: usize,
    code_lines: usize,
    bytes: u64,
}

fn is_build_file(name: &str) -> bool {
    matches!(
        name,
        "Cargo.toml"
            | "Cargo.lock"
            | "pyproject.toml"
            | "setup.py"
            | "setup.cfg"
            | "package.json"
            | "package-lock.json"
            | "yarn.lock"
            | "bun.lockb"
            | "pnpm-lock.yaml"
            | "go.mod"
            | "go.sum"
            | "pom.xml"
            | "build.gradle"
            | "build.gradle.kts"
            | "settings.gradle"
            | "settings.gradle.kts"
            | "Gemfile"
            | "Gemfile.lock"
            | "composer.json"
            | "composer.lock"
            | "Makefile"
            | "CMakeLists.txt"
            | "Dockerfile"
            | "flake.nix"
            | "shell.nix"
            | "default.nix"
            | "mix.exs"
    )
}

fn is_entrypoint(name: &str, rel: &str) -> bool {
    if matches!(
        name,
        "main.rs"
            | "lib.rs"
            | "main.py"
            | "__main__.py"
            | "app.py"
            | "manage.py"
            | "main.go"
            | "Main.java"
            | "Application.java"
            | "Program.cs"
            | "main.kt"
            | "main.swift"
    ) {
        return true;
    }
    let lower = name.to_lowercase();
    if lower == "index.ts" || lower == "index.tsx" || lower == "index.js" || lower == "index.mjs" {
        return rel.starts_with("src/") || rel.contains("/src/") || rel.split('/').count() <= 2;
    }
    false
}

fn collect_top_symbols(engine: &Engine, n: usize) -> Vec<SymbolStat> {
    let all = engine.handles.symbols.lookup_prefix("");
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (name, _loc) in all {
        *counts.entry(name).or_insert(0) += 1;
    }
    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked
        .into_iter()
        .take(n)
        .map(|(name, definitions)| SymbolStat { name, definitions })
        .collect()
}

fn fingerprint(items: &[(String, u64, SystemTime)]) -> String {
    let mut sorted: Vec<&(String, u64, SystemTime)> = items.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut state = FnvHasher::new();
    for (path, size, mtime) in sorted {
        for b in path.as_bytes() {
            state.update(*b);
        }
        state.update(0);
        for b in size.to_le_bytes() {
            state.update(b);
        }
        let secs = mtime
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        for b in secs.to_le_bytes() {
            state.update(b);
        }
    }
    format!("{:016x}", state.finish())
}

struct FnvHasher {
    h: u64,
}

impl FnvHasher {
    fn new() -> Self {
        Self {
            h: 0xcbf29ce484222325,
        }
    }
    fn update(&mut self, b: u8) {
        self.h ^= b as u64;
        self.h = self.h.wrapping_mul(0x100000001b3);
    }
    fn finish(self) -> u64 {
        self.h
    }
}

fn render_text(p: &OverviewOutput) -> String {
    let mut out = String::new();
    out.push_str(&format!("# overview: {}\n", p.root));
    out.push_str(&format!("fingerprint   {}\n", p.fingerprint));
    out.push_str(&format!(
        "files         {}  (code: {}, total LOC ~{})\n",
        p.files, p.code_files, p.code_lines
    ));
    out.push_str(&format!("bytes         {}\n", humanize_bytes(p.bytes)));

    if !p.languages.is_empty() {
        out.push_str("\n## languages\n");
        for l in &p.languages {
            out.push_str(&format!(
                "  {:<14} {:>4} files  ~{} LOC\n",
                l.lang, l.files, l.code_lines
            ));
        }
    }

    if !p.build_files.is_empty() {
        out.push_str("\n## build files\n");
        for f in &p.build_files {
            out.push_str(&format!("  {f}\n"));
        }
    }

    if !p.entrypoints.is_empty() {
        out.push_str("\n## entrypoints\n");
        for f in &p.entrypoints {
            out.push_str(&format!("  {f}\n"));
        }
    }

    if !p.top_symbols.is_empty() {
        out.push_str("\n## top symbols\n");
        for s in &p.top_symbols {
            out.push_str(&format!("  {:<24} {} defs\n", s.name, s.definitions));
        }
    }
    out
}

fn humanize_bytes(b: u64) -> String {
    const K: u64 = 1024;
    if b < K {
        format!("{b}B")
    } else if b < K * K {
        format!("{:.1}K", b as f64 / K as f64)
    } else if b < K * K * K {
        format!("{:.1}M", b as f64 / (K * K) as f64)
    } else {
        format!("{:.1}G", b as f64 / (K * K * K) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_files_match_known_names() {
        assert!(is_build_file("Cargo.toml"));
        assert!(is_build_file("package.json"));
        assert!(is_build_file("pyproject.toml"));
        assert!(is_build_file("Dockerfile"));
        assert!(!is_build_file("README.md"));
        assert!(!is_build_file("Cargo.txt"));
    }

    #[test]
    fn entrypoints_pick_known_names() {
        assert!(is_entrypoint("main.rs", "src/main.rs"));
        assert!(is_entrypoint("lib.rs", "src/lib.rs"));
        assert!(is_entrypoint("main.py", "app/main.py"));
        assert!(!is_entrypoint("util.rs", "src/util.rs"));
    }

    #[test]
    fn entrypoints_index_only_at_src_or_top_level() {
        assert!(is_entrypoint("index.ts", "src/index.ts"));
        assert!(is_entrypoint("index.ts", "index.ts"));
        assert!(is_entrypoint("index.ts", "packages/foo/src/index.ts"));
        assert!(!is_entrypoint("index.ts", "node_modules/x/y/index.ts"));
    }

    #[test]
    fn fingerprint_is_deterministic_and_size_sensitive() {
        let mtime = SystemTime::UNIX_EPOCH;
        let a = vec![("a.rs".to_string(), 10u64, mtime)];
        let b = vec![("a.rs".to_string(), 10u64, mtime)];
        assert_eq!(fingerprint(&a), fingerprint(&b));

        let c = vec![("a.rs".to_string(), 11u64, mtime)];
        assert_ne!(fingerprint(&a), fingerprint(&c));
    }

    #[test]
    fn fingerprint_is_order_independent() {
        let mtime = SystemTime::UNIX_EPOCH;
        let a = vec![
            ("a.rs".to_string(), 10u64, mtime),
            ("b.rs".to_string(), 20u64, mtime),
        ];
        let b = vec![
            ("b.rs".to_string(), 20u64, mtime),
            ("a.rs".to_string(), 10u64, mtime),
        ];
        assert_eq!(fingerprint(&a), fingerprint(&b));
    }
}
