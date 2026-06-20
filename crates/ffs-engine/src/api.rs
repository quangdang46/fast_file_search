//! High-level API for programmatic use (jcode, external tools).
//!
//! Provides structured, serializable search and retrieval functions
//! that can be called directly as a Rust library dependency.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ignore::WalkBuilder;
use rayon::prelude::*;
use serde::Serialize;

use ffs_search::role::{Role, detect_role};

// ─── Re-exports ───
pub use crate::dispatch::ReadResult;
pub use ffs_symbol::types::OutlineEntry;

// ─── Grep API ───

/// A single match in a file.
#[derive(Debug, Clone, Serialize)]
pub struct GrepMatch {
    pub line: u32,
    pub text: String,
}

/// A symbol group containing matches.
#[derive(Debug, Clone, Serialize)]
pub struct MatchGroup {
    pub kind: String,
    pub name: String,
    pub start_line: u32,
    pub end_line: u32,
    pub matches: Vec<GrepMatch>,
}

/// Matches in a single file, with symbol groups.
#[derive(Debug, Clone, Serialize)]
pub struct GrepFile {
    pub path: String,
    pub language: String,
    pub total_matches: usize,
    pub total_symbols: usize,
    pub groups: Vec<MatchGroup>,
}

/// Result of a grep search.
#[derive(Debug, Clone, Serialize)]
pub struct GrepResult {
    pub needle: String,
    pub total_files: usize,
    pub total_matches: usize,
    pub files: Vec<GrepFile>,
}

/// Options for grep search.
#[derive(Debug, Clone)]
pub struct GrepOptions {
    pub regex: bool,
    pub case_sensitive: bool,
    pub max_matches: usize,
    pub max_files: usize,
}

impl Default for GrepOptions {
    fn default() -> Self {
        Self {
            regex: false,
            case_sensitive: false,
            max_matches: 200,
            max_files: 50,
        }
    }
}

/// Search file contents with optional symbol grouping.
pub fn grep(root: &Path, needle: &str, options: &GrepOptions) -> GrepResult {
    let needle_lower = needle.to_lowercase();
    let is_regex = options.regex;

    // Build the regex if needed
    let re = if is_regex {
        regex::bytes::RegexBuilder::new(needle)
            .case_insensitive(!options.case_sensitive)
            .build()
            .ok()
    } else {
        None
    };

    // Walk files
    let files: Vec<PathBuf> = WalkBuilder::new(root)
        .standard_filters(true)
        .follow_links(false)
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.into_path())
        .collect();

    let hits_mutex: Mutex<Vec<(PathBuf, u32, String)>> = Mutex::new(Vec::new());

    files.par_iter().for_each(|path| {
        let Ok(content) = std::fs::read(path) else { return };
        if content[..content.len().min(4096)].contains(&0u8) { return }

        let mut local_hits: Vec<(u32, String)> = Vec::new();
        let text = String::from_utf8_lossy(&content);

        if let Some(ref regex) = re {
            for m in regex.find_iter(&content) {
                let line = byte_to_line(&content, m.start());
                let snippet = extract_line(&text, line);
                local_hits.push((line, snippet));
            }
        } else {
            for (i, line) in text.lines().enumerate() {
                let line_num = (i + 1) as u32;
                let check = if options.case_sensitive { line } else { &line.to_lowercase() };
                if check.contains(&needle_lower) {
                    local_hits.push((line_num, line.to_string()));
                }
            }
        }

        if local_hits.is_empty() { return }

        if let Ok(mut guard) = hits_mutex.lock() {
            for (ln, txt) in local_hits {
                guard.push((path.clone(), ln, txt));
            }
        }
    });

    let all_hits = hits_mutex.into_inner().unwrap_or_default();

    // Group by file then by enclosing symbol
    let mut by_file: std::collections::BTreeMap<String, Vec<(u32, String)>> = std::collections::BTreeMap::new();
    for (path, line, text) in &all_hits {
        by_file.entry(path.to_string_lossy().to_string()).or_default().push((*line, text.clone()));
    }

    let mut result_files: Vec<GrepFile> = Vec::new();
    for (path_str, file_hits) in &by_file {
        let content = std::fs::read_to_string(path_str).ok();
        let language = path_str.rsplit('.').next().unwrap_or("").to_string();
        let entries = content.as_deref().map(simple_outline).unwrap_or_default();

        let mut groups: Vec<MatchGroup> = Vec::new();
        let mut unmatched: Vec<GrepMatch> = Vec::new();

        for (line, text) in file_hits {
            let line_usize = *line as usize;
            let enclosing = entries.iter().find(|e| e.start_line <= line_usize && line_usize <= e.end_line);
            if let Some(sym) = enclosing {
                if let Some(g) = groups.iter_mut().find(|g: &&mut MatchGroup| g.name == sym.name && g.kind == sym.kind) {
                    g.matches.push(GrepMatch { line: *line, text: text.clone() });
                } else {
                    groups.push(MatchGroup {
                        kind: sym.kind.clone(),
                        name: sym.name.clone(),
                        start_line: sym.start_line as u32,
                        end_line: sym.end_line as u32,
                        matches: vec![GrepMatch { line: *line, text: text.clone() }],
                    });
                }
            } else {
                unmatched.push(GrepMatch { line: *line, text: text.clone() });
            }
        }

        if !unmatched.is_empty() {
            groups.push(MatchGroup {
                kind: "file".into(),
                name: "<file scope>".into(),
                start_line: 0,
                end_line: 0,
                matches: unmatched,
            });
        }

        result_files.push(GrepFile {
            path: path_str.clone(),
            language,
            total_matches: file_hits.len(),
            total_symbols: entries.len(),
            groups,
        });

        if result_files.len() >= options.max_files { break }
    }

    let total_matches: usize = result_files.iter().map(|f| f.total_matches).sum();

    GrepResult {
        needle: needle.to_string(),
        total_files: result_files.len(),
        total_matches,
        files: result_files,
    }
}

// ─── Find API ───

/// A scored file result.
#[derive(Debug, Clone, Serialize)]
pub struct ScoredFile {
    pub path: String,
    pub role: String,
    pub score: i32,
    pub score_breakdown: Vec<String>,
}

/// Result of a find search.
#[derive(Debug, Clone, Serialize)]
pub struct FindResult {
    pub query: String,
    pub total_files: usize,
    pub files: Vec<ScoredFile>,
}

/// Options for find search.
#[derive(Debug, Clone)]
pub struct FindOptions {
    pub max_files: usize,
    pub score_threshold: i32,
}

impl Default for FindOptions {
    fn default() -> Self {
        Self { max_files: 30, score_threshold: 1 }
    }
}

/// Find files by name with role-based scoring.
pub fn find(root: &Path, query: &str, options: &FindOptions) -> FindResult {
    let query_lower = query.to_lowercase();
    let tokens: Vec<&str> = query_lower.split_whitespace().collect();

    let mut scored: Vec<ScoredFile> = Vec::new();

    for entry in WalkBuilder::new(root)
        .standard_filters(true)
        .follow_links(false)
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
    {
        let path = entry.path();
        let rel = path.to_string_lossy().to_lowercase();
        let path_str = path.to_string_lossy().to_string();

        // Score: path contains query
        let mut score: i32 = 0;
        let mut reasons: Vec<String> = Vec::new();

        // Full query match in path
        if rel.contains(&query_lower) {
            score += 100;
            reasons.push("path contains full query".into());
        }

        // Individual token matches
        let token_matches: usize = tokens.iter().filter(|t| rel.contains(*t)).count();
        if token_matches > 0 {
            score += (token_matches * 25) as i32;
            reasons.push(format!("{token_matches} tokens matched in path"));
        }

        if score <= 0 { continue }

        // Role bonus
        let role = detect_role(path);
        let role_bonus = role.score_bonus();
        if role_bonus != 0 {
            score += role_bonus;
            reasons.push(format!("role bonus: {} ({})", role.as_str(), if role_bonus > 0 { format!("+{role_bonus}") } else { role_bonus.to_string() }));
        }

        if score < options.score_threshold { continue }

        scored.push(ScoredFile {
            path: path_str,
            role: role.as_str().to_string(),
            score,
            score_breakdown: reasons,
        });
    }

    // Sort by score descending
    scored.sort_by(|a, b| b.score.cmp(&a.score));
    scored.truncate(options.max_files);

    FindResult {
        query: query.to_string(),
        total_files: scored.len(),
        files: scored,
    }
}

// ─── Outline API ───

/// Result of a file outline.
#[derive(Debug, Clone, Serialize)]
pub struct OutlineResult {
    pub path: String,
    pub language: String,
    pub entries: Vec<OutlineEntry>,
}

/// Get the structural outline of a file.
pub fn outline(path: &Path) -> Option<OutlineResult> {
    use ffs_symbol::lang::detect_file_type;
    use ffs_symbol::outline::get_outline_entries;

    let content = std::fs::read_to_string(path).ok()?;
    let ft = detect_file_type(path);
    let lang = match ft {
        ffs_symbol::types::FileType::Code(l) => l,
        _ => return None,
    };
    let entries = get_outline_entries(&content, lang);

    Some(OutlineResult {
        path: path.to_string_lossy().to_string(),
        language: format!("{lang:?}"),
        entries,
    })
}

// ─── Internal helpers ───

struct SymEntry {
    kind: String,
    name: String,
    start_line: usize,
    end_line: usize,
}

fn simple_outline(text: &str) -> Vec<SymEntry> {
    let mut entries = Vec::new();
    let lines: Vec<&str> = text.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        let line_num = i + 1;
        let trimmed = line.trim_start();

        // Rust
        if let Some(name) = kw_match(trimmed, "fn ") {
            let end = find_block_end(&lines[i..], line_num);
            entries.push(SymEntry { kind: "function".into(), name, start_line: line_num, end_line: end });
        } else if let Some(name) = kw_match(trimmed, "struct ") {
            let end = find_block_end(&lines[i..], line_num);
            entries.push(SymEntry { kind: "struct".into(), name, start_line: line_num, end_line: end });
        } else if let Some(name) = kw_match(trimmed, "enum ") {
            let end = find_block_end(&lines[i..], line_num);
            entries.push(SymEntry { kind: "enum".into(), name, start_line: line_num, end_line: end });
        } else if let Some(name) = kw_match(trimmed, "trait ") {
            let end = find_block_end(&lines[i..], line_num);
            entries.push(SymEntry { kind: "trait".into(), name, start_line: line_num, end_line: end });
        } else if let Some(name) = kw_match(trimmed, "impl ") {
            let name = name.split(|c: char| c == '{' || c == 'w').next().unwrap_or("").trim().to_string();
            let end = find_block_end(&lines[i..], line_num);
            entries.push(SymEntry { kind: "impl".into(), name, start_line: line_num, end_line: end });
        }
        // TypeScript/JS
        else if let Some(name) = kw_match(trimmed, "function ") {
            let end = find_block_end(&lines[i..], line_num);
            entries.push(SymEntry { kind: "function".into(), name, start_line: line_num, end_line: end });
        } else if let Some(name) = kw_match(trimmed, "class ") {
            let end = find_block_end(&lines[i..], line_num);
            entries.push(SymEntry { kind: "class".into(), name, start_line: line_num, end_line: end });
        }
    }

    entries.sort_by(|a, b| a.start_line.cmp(&b.start_line));
    entries
}

fn kw_match<'a>(line: &'a str, kw: &str) -> Option<String> {
    if line.starts_with(kw) {
        let rest = line.strip_prefix(kw)?;
        let name = rest.split(|c: char| c == '(' || c == '<' || c == ':' || c == '{' || c == ' ')
            .next()?.trim().to_string();
        if name.is_empty() { None } else { Some(name) }
    } else {
        for prefix in &["pub ", "pub(crate) ", "pub(super) ", "export "] {
            if let Some(after) = line.strip_prefix(prefix) {
                if after.starts_with(kw) {
                    return kw_match(after, kw);
                }
            }
        }
        None
    }
}

fn find_block_end(lines: &[&str], start: usize) -> usize {
    let mut depth: i32 = 0;
    let mut first_brace = false;
    for (i, line) in lines.iter().enumerate() {
        for &b in line.as_bytes() {
            if b == b'{' { depth += 1; first_brace = true; }
            else if b == b'}' { depth -= 1; }
        }
        if first_brace && depth <= 0 && i > 0 { return start + i; }
    }
    start + lines.len()
}

fn byte_to_line(content: &[u8], offset: usize) -> u32 {
    content[..offset].iter().filter(|&&b| b == b'\n').count() as u32 + 1
}

fn extract_line(text: &str, line_num: u32) -> String {
    text.lines().nth((line_num - 1) as usize).unwrap_or("").to_string()
}
