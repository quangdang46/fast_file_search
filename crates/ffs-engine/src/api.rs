//! High-level API for programmatic use (jcode, external tools).
//!
//! Provides structured, serializable search and retrieval functions
//! that can be called directly as a Rust library dependency.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ignore::WalkBuilder;
use rayon::prelude::*;
use serde::Serialize;

use ffs_search::role::detect_role;

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
        let Ok(content) = std::fs::read(path) else {
            return;
        };
        if content[..content.len().min(4096)].contains(&0u8) {
            return;
        }

        let mut local_hits: Vec<(u32, String)> = Vec::new();
        let text = String::from_utf8_lossy(&content);

        if let Some(ref regex) = re {
            // find_iter yields matches in order: count newlines incrementally
            // (O(file) total) instead of byte_to_line's O(file) per match, and
            // slice the snippet directly around the match offset.
            let mut prev = 0usize;
            let mut line = 1u32;
            for m in regex.find_iter(&content) {
                line += content[prev..m.start()]
                    .iter()
                    .filter(|&&b| b == b'\n')
                    .count() as u32;
                prev = m.start();
                local_hits.push((line, extract_line_at(&content, m.start())));
            }
        } else {
            // Reuse one lowercase buffer across lines instead of allocating a
            // fresh String per line.
            let mut lower_buf = String::with_capacity(256);
            for (i, line) in text.lines().enumerate() {
                let line_num = (i + 1) as u32;
                let matched = if options.case_sensitive {
                    line.contains(&needle_lower)
                } else {
                    lower_buf.clear();
                    lower_buf.extend(line.chars().flat_map(char::to_lowercase));
                    lower_buf.contains(&needle_lower)
                };
                if matched {
                    local_hits.push((line_num, line.to_string()));
                }
            }
        }

        if local_hits.is_empty() {
            return;
        }

        if let Ok(mut guard) = hits_mutex.lock() {
            for (ln, txt) in local_hits {
                guard.push((path.clone(), ln, txt));
            }
        }
    });

    let all_hits = hits_mutex.into_inner().unwrap_or_default();

    // Group by file then by enclosing symbol
    let mut by_file: std::collections::BTreeMap<String, Vec<(u32, String)>> =
        std::collections::BTreeMap::new();
    for (path, line, text) in &all_hits {
        by_file
            .entry(path.to_string_lossy().to_string())
            .or_default()
            .push((*line, text.clone()));
    }

    let mut result_files: Vec<GrepFile> = Vec::new();
    for (path_str, file_hits) in &by_file {
        let content = ffs_search::bom::read_file(path_str).ok();
        let language = path_str.rsplit('.').next().unwrap_or("").to_string();
        let entries = content.as_deref().map(simple_outline).unwrap_or_default();

        let mut groups: Vec<MatchGroup> = Vec::new();
        let mut unmatched: Vec<GrepMatch> = Vec::new();

        for (line, text) in file_hits {
            let line_usize = *line as usize;
            let enclosing = entries
                .iter()
                .find(|e| e.start_line <= line_usize && line_usize <= e.end_line);
            if let Some(sym) = enclosing {
                if let Some(g) = groups.iter_mut().find(|g: &&mut MatchGroup| {
                    g.name == sym.name
                        && g.kind == sym.kind
                        && g.start_line == (sym.start_line as u32)
                        && g.end_line == (sym.end_line as u32)
                }) {
                    g.matches.push(GrepMatch {
                        line: *line,
                        text: text.clone(),
                    });
                } else {
                    groups.push(MatchGroup {
                        kind: sym.kind.clone(),
                        name: sym.name.clone(),
                        start_line: sym.start_line as u32,
                        end_line: sym.end_line as u32,
                        matches: vec![GrepMatch {
                            line: *line,
                            text: text.clone(),
                        }],
                    });
                }
            } else {
                unmatched.push(GrepMatch {
                    line: *line,
                    text: text.clone(),
                });
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

        if result_files.len() >= options.max_files {
            break;
        }
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
        Self {
            max_files: 30,
            score_threshold: 1,
        }
    }
}

/// Find files by name with role-based scoring.
pub fn find(root: &Path, query: &str, options: &FindOptions) -> FindResult {
    let query_lower = query.to_lowercase();
    let tokens: Vec<&str> = query_lower.split_whitespace().collect();

    // Parallel walk + scoring. Paths are sorted first so results are
    // deterministic (equal-score ties resolve alphabetically by path).
    let paths: Vec<PathBuf> = {
        use ignore::WalkState;
        use std::sync::Mutex;
        let collected: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
        let mut wb = WalkBuilder::new(root);
        wb.standard_filters(true).follow_links(false);
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .min(8);
        wb.threads(threads);
        wb.build_parallel().run(|| {
            let collected = &collected;
            Box::new(move |result| match result {
                Ok(entry) => {
                    if entry.file_type().is_some_and(|t| t.is_file()) {
                        collected.lock().unwrap().push(entry.into_path());
                    }
                    WalkState::Continue
                }
                Err(_) => WalkState::Continue,
            })
        });
        let mut v = collected.into_inner().unwrap_or_default();
        v.sort();
        v
    };

    let mut scored: Vec<ScoredFile> = {
        use rayon::prelude::*;
        paths
            .par_iter()
            .filter_map(|path| {
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

                if score <= 0 {
                    return None;
                }

                // Role bonus
                let role = detect_role(path);
                let role_bonus = role.score_bonus();
                if role_bonus != 0 {
                    score += role_bonus;
                    reasons.push(format!(
                        "role bonus: {} ({})",
                        role.as_str(),
                        if role_bonus > 0 {
                            format!("+{role_bonus}")
                        } else {
                            role_bonus.to_string()
                        }
                    ));
                }

                if score < options.score_threshold {
                    return None;
                }

                Some(ScoredFile {
                    path: path_str,
                    role: role.as_str().to_string(),
                    score,
                    score_breakdown: reasons,
                })
            })
            .collect()
    };

    // Sort by score descending (stable: path order preserved for ties)
    scored.sort_by_key(|b| std::cmp::Reverse(b.score));
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

    let content = ffs_search::bom::read_file(path).ok()?;
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
    let lines: Vec<&str> = text.lines().collect();
    let n = lines.len();

    // Single forward pass: each line is byte-scanned once and brace events
    // update every open definition frame, instead of re-scanning the suffix
    // of the file for every definition (was O(defs x file)).
    // NOTE: string/char literal state is tracked once globally rather than
    // per-definition; identical on well-formed code, may differ only for
    // paths with unbalanced quotes before a definition.
    #[derive(Clone, Copy)]
    struct Frame {
        sym_idx: usize,
        depth: i32,
        first_brace: bool,
    }

    let mut entries: Vec<SymEntry> = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut in_string = false;

    for (i, line) in lines.iter().enumerate() {
        let line_num = i + 1;
        let trimmed = line.trim_start();

        // Definition detection — same keyword priority chain as before.
        let def: Option<(&'static str, String)> = if let Some(name) = kw_match(trimmed, "fn ") {
            Some(("function", name))
        } else if let Some(name) = kw_match(trimmed, "struct ") {
            Some(("struct", name))
        } else if let Some(name) = kw_match(trimmed, "enum ") {
            Some(("enum", name))
        } else if let Some(name) = kw_match(trimmed, "trait ") {
            Some(("trait", name))
        } else if let Some(name) = match_impl(trimmed) {
            // Extract impl name: prefer implementing type (for impl Trait for Type)
            let mut rest = name.as_str();
            if let Some(pos) = rest.find(" for ") {
                rest = &rest[pos + 5..];
            }
            // Strip generics from impl side if present in the captured name
            if rest.starts_with('<') {
                let mut depth = 0usize;
                for (ci, c) in rest.char_indices() {
                    match c {
                        '<' => depth += 1,
                        '>' => {
                            depth -= 1;
                            if depth == 0 {
                                rest = rest[ci + c.len_utf8()..].trim_start();
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            let name = rest.split('{').next().unwrap_or(rest).trim().to_string();
            Some(("impl", name))
        } else if let Some(name) = kw_match(trimmed, "function ") {
            Some(("function", name))
        } else if let Some(name) = kw_match(trimmed, "class ") {
            Some(("class", name))
        } else {
            None
        };

        if let Some((kind, name)) = def {
            entries.push(SymEntry {
                kind: kind.into(),
                name,
                start_line: line_num,
                end_line: line_num,
            });
            stack.push(Frame {
                sym_idx: entries.len() - 1,
                depth: 0,
                first_brace: false,
            });
        }

        // Scan the line once; each brace event updates all open frames.
        let bytes = line.as_bytes();
        let mut j = 0usize;
        while j < bytes.len() {
            let b = bytes[j];
            if in_string {
                if b == b'\\' {
                    j += 2;
                    continue;
                }
                if b == b'"' {
                    in_string = false;
                }
                j += 1;
                continue;
            }
            match b {
                b'/' if j + 1 < bytes.len() && bytes[j + 1] == b'/' => break,
                b'"' => {
                    in_string = true;
                    j += 1;
                }
                b'\'' => {
                    // Char literal '{'/'\n' vs lifetime 'a — skip a short
                    // literal, otherwise treat as an ordinary byte.
                    if j + 2 < bytes.len() && bytes[j + 2] == b'\'' {
                        j += 3;
                    } else if j + 3 < bytes.len() && bytes[j + 1] == b'\\' && bytes[j + 3] == b'\''
                    {
                        j += 4;
                    } else {
                        j += 1;
                    }
                }
                b'{' => {
                    for f in stack.iter_mut() {
                        f.depth += 1;
                        f.first_brace = true;
                    }
                    j += 1;
                }
                b'}' => {
                    for f in stack.iter_mut() {
                        f.depth -= 1;
                    }
                    j += 1;
                }
                _ => j += 1,
            }
        }

        // Close frames at end of line: balanced block, or bodyless
        // declaration (`struct X;`, `fn f();`) before any brace.
        let semi = line.trim_end().ends_with(';');
        if !stack.is_empty() && (semi || stack.iter().any(|f| f.first_brace && f.depth <= 0)) {
            let mut w = 0usize;
            for r in 0..stack.len() {
                let f = stack[r];
                if (f.first_brace && f.depth <= 0) || (!f.first_brace && semi) {
                    entries[f.sym_idx].end_line = line_num;
                } else {
                    stack[w] = f;
                    w += 1;
                }
            }
            stack.truncate(w);
        }
    }

    // Unclosed blocks extend past EOF, matching the old find_block_end fallback.
    for f in stack {
        entries[f.sym_idx].end_line = n + 1;
    }

    entries.sort_by_key(|a| a.start_line);
    entries
}

fn kw_match(line: &str, kw: &str) -> Option<String> {
    if line.starts_with(kw) {
        let rest = line.strip_prefix(kw)?;
        let name = rest
            .split(['(', '<', ':', '{', ' '])
            .next()?
            .trim()
            .to_string();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
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

/// Match `impl` blocks including generic forms `impl<T>`, `impl<T> Trait for
/// Type`, and `impl Trait for Type`. `kw_match` can't be used: it splits on
/// `<`/space, which truncates both the generic parameter list and the
/// `Trait for Type` target.
fn match_impl(line: &str) -> Option<String> {
    let rest = line.strip_prefix("impl")?;
    // Require a boundary so `implementation`-like identifiers don't match.
    if !(rest.starts_with('<') || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    let mut rest = rest.trim_start();
    if rest.starts_with('<') {
        // Skip the balanced generic parameter list.
        let mut depth = 0usize;
        let mut end = None;
        for (i, c) in rest.char_indices() {
            match c {
                '<' => depth += 1,
                '>' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        end = Some(i + c.len_utf8());
                        break;
                    }
                }
                _ => {}
            }
        }
        let generics_end = end?;
        rest = rest.get(generics_end..)?.trim_start();
    }
    // Impl targets may contain spaces (`Trait for Type`) and generics
    // (`Foo<T>`), so take everything up to the opening brace.
    let name = rest.split('{').next()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn extract_line_at(content: &[u8], offset: usize) -> String {
    // Slice the line containing `offset` directly — O(line length) instead of
    // text.lines().nth()'s O(file). Operates on raw bytes so multi-byte UTF-8
    // before the match can't shift the offset (lossy conversion may change
    // byte length).
    let start = content[..offset]
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let end = content[offset..]
        .iter()
        .position(|&b| b == b'\n')
        .map(|i| offset + i)
        .unwrap_or(content.len());
    String::from_utf8_lossy(&content[start..end]).into_owned()
}
