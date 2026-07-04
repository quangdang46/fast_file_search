use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use anyhow::Result;
use clap::Parser;
use memchr::memmem;
use rayon::prelude::*;
use serde::Serialize;

use crate::cli::OutputFormat;

#[derive(Debug, Parser)]
#[command(after_help = "\
EXAMPLES:
  ffs grep TODO                            # smart-case literal search
  ffs grep '\\bTODO\\b' --regex            # forced regex (auto-detect would also pick this up)
  ffs grep -F '.is_file()'                 # force literal — '.' won't be a regex wildcard
  ffs grep --regex 'fn\\s+\\w+\\(' --root crates/  # signature-style regex over a sub-tree
  ffs grep -w error                        # whole-word match only
  ffs grep -l fixme                        # files-with-matches mode (one path per line)")]
pub struct Args {
    /// Pattern. Auto-detected as a regular expression when it contains any
    /// regex metacharacter (`.`, `*`, `+`, `?`, `^`, `$`, `[`, `(`, `|`, `\`).
    /// Force literal interpretation with `--fixed-strings`, or force regex
    /// with `--regex`.
    pub needle: String,

    /// Maximum lines emitted total across all files.
    #[arg(long, default_value_t = 200)]
    pub limit: usize,

    /// Match case sensitively (default: false / smart-case when unset).
    #[arg(short = 's', long)]
    pub case_sensitive: bool,

    /// Force regex interpretation (overrides auto-detection).
    #[arg(short = 'r', long)]
    pub regex: bool,

    /// Force literal / fixed-string interpretation (overrides auto-detection).
    #[arg(short = 'F', long = "fixed-strings", conflicts_with = "regex")]
    pub fixed_strings: bool,

    /// Require whole-word matches (wraps the pattern with `\b…\b`).
    #[arg(short = 'w', long = "word-regexp")]
    pub word_regexp: bool,

    /// Stop after N matches per file. 0 = unlimited (default).
    #[arg(long = "max-count", default_value_t = 0)]
    pub max_count: usize,

    /// Output only the file paths (one per line) — like `rg -l`.
    #[arg(short = 'l', long = "files-with-matches")]
    pub files_with_matches: bool,

    /// Group matches by file and enclosing symbol (like agentgrep).
    #[arg(long)]
    pub group: bool,
}

#[derive(Debug, Serialize)]
struct GrepHit {
    path: String,
    line: u32,
    text: String,
}

#[derive(Debug, Serialize)]
struct GrepResult {
    needle: String,
    hits: Vec<GrepHit>,
    total_files_searched: usize,
    /// "literal" or "regex" — whichever matcher actually ran.
    mode: &'static str,
    schema: &'static str,
}

/// Auto-detect: looks like a regex if it contains any of `.+*?^$[(|\` characters.
/// Mirrors the heuristic in `ffs::grep::has_regex_metacharacters` so CLI and MCP
/// agree on what a "literal" query looks like.
fn looks_like_regex(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(
            c,
            '.' | '+' | '*' | '?' | '^' | '$' | '[' | '(' | '|' | '\\'
        )
    })
}

#[derive(Clone)]
enum Matcher {
    Literal {
        needle: Vec<u8>,
        case_insensitive: bool,
    },
    Regex(regex::bytes::Regex),
}

impl Matcher {
    fn build(args: &Args) -> Result<(Self, &'static str)> {
        // Smart case: case_sensitive flag forces sensitive; otherwise we look
        // at the pattern to decide. Pattern with any uppercase => sensitive.
        let smart_case_sensitive =
            args.case_sensitive || args.needle.chars().any(|c| c.is_uppercase());

        let use_regex = args.regex || (!args.fixed_strings && looks_like_regex(&args.needle));

        if use_regex {
            let mut pattern = args.needle.clone();
            if args.word_regexp {
                pattern = format!(r"\b(?:{})\b", pattern);
            }
            let re = regex::bytes::RegexBuilder::new(&pattern)
                .case_insensitive(!smart_case_sensitive)
                .multi_line(true)
                .build()
                .map_err(|e| anyhow::anyhow!("invalid regex {:?}: {e}", args.needle))?;
            Ok((Matcher::Regex(re), "regex"))
        } else {
            let needle_bytes = if smart_case_sensitive {
                args.needle.as_bytes().to_vec()
            } else {
                args.needle.to_lowercase().into_bytes()
            };
            Ok((
                Matcher::Literal {
                    needle: needle_bytes,
                    case_insensitive: !smart_case_sensitive,
                },
                "literal",
            ))
        }
    }

    /// Returns iterator of (start_byte_offset, end_byte_offset) for each
    /// match. Caller maps the start back to a line number; the end is used
    /// to render multi-line matches faithfully (bug 16).
    fn find_iter<'a>(
        &'a self,
        haystack: &'a [u8],
    ) -> Box<dyn Iterator<Item = (usize, usize)> + 'a> {
        match self {
            Matcher::Literal {
                needle,
                case_insensitive,
            } => {
                if *case_insensitive {
                    let needle = needle.clone();
                    let lower: Vec<u8> = haystack.iter().map(|b| b.to_ascii_lowercase()).collect();
                    let nlen = needle.len();
                    let finder = memmem::Finder::new(&needle).into_owned();
                    let positions: Vec<(usize, usize)> =
                        finder.find_iter(&lower).map(|p| (p, p + nlen)).collect();
                    Box::new(positions.into_iter())
                } else {
                    let nlen = needle.len();
                    let finder = memmem::Finder::new(needle.as_slice());
                    Box::new(
                        finder
                            .find_iter(haystack)
                            .map(|p| (p, p + nlen))
                            .collect::<Vec<_>>()
                            .into_iter(),
                    )
                }
            }
            Matcher::Regex(re) => Box::new(
                re.find_iter(haystack)
                    .map(|m| (m.start(), m.end()))
                    .collect::<Vec<_>>()
                    .into_iter(),
            ),
        }
    }
}

/// Map a byte offset to (1-based line number, line slice).
fn byte_to_line(haystack: &[u8], offset: usize) -> (u32, &[u8]) {
    // Walk forwards counting newlines is O(N). For large files this is the
    // bottleneck for hit-dense patterns; switching to a sorted newline index
    // would let us binary-search per hit. For typical workloads (few hits
    // per file) the linear scan is fine.
    let mut line = 1u32;
    let mut line_start = 0usize;
    let mut i = 0;
    while i < offset {
        if haystack[i] == b'\n' {
            line += 1;
            line_start = i + 1;
        }
        i += 1;
    }
    let line_end = haystack[line_start..]
        .iter()
        .position(|&b| b == b'\n')
        .map(|p| line_start + p)
        .unwrap_or(haystack.len());
    (line, &haystack[line_start..line_end])
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    if args.needle.is_empty() {
        return Err(anyhow::anyhow!(
            "ffs grep: needle is empty; pass a non-empty pattern"
        ));
    }
    let (matcher, mode) = Matcher::build(&args)?;

    // Bigram prefilter: only safe (and helpful) for literal patterns.
    // We try to load the persisted index; on miss we just scan everything.
    let prefilter_paths: Option<Vec<PathBuf>> = match &matcher {
        Matcher::Literal { needle, .. } if needle.len() >= 2 => {
            let cache = crate::cache::CacheDir::at(root);
            cache.load_bigram_index(root).and_then(|idx| {
                idx.filter(needle)
                    .map(|paths| paths.into_iter().map(PathBuf::from).collect())
            })
        }
        _ => None,
    };

    let files: Vec<PathBuf> = match prefilter_paths {
        Some(paths) => paths,
        None => super::walk_files(root),
    };
    // Bug 18: report a consistent denominator. Always use the total workspace
    // file count so the literal and regex paths both display "matches across
    // N files" with the same N — even when the bigram prefilter narrows the
    // candidate set down to zero.
    let total_files = if matches!(matcher, Matcher::Literal { .. }) {
        super::walk_files(root).len()
    } else {
        files.len()
    };
    let limit = args.limit;
    let max_count = if args.max_count == 0 {
        usize::MAX
    } else {
        args.max_count
    };

    let hits_mutex: Mutex<Vec<GrepHit>> = Mutex::new(Vec::new());
    let hit_counter = AtomicUsize::new(0);
    let stop = std::sync::atomic::AtomicBool::new(false);

    // Parallel scan across files; cooperative early-exit once the global
    // limit is reached so we don't waste IO on the tail of the workload.
    files.par_iter().for_each(|path: &PathBuf| {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let Ok(content) = std::fs::read(path) else {
            return;
        };
        // Quick binary heuristic: skip files containing NUL in the first 8KB.
        let probe = &content[..content.len().min(8 * 1024)];
        if probe.contains(&0u8) {
            return;
        }

        let mut local_hits: Vec<GrepHit> = Vec::new();
        for (per_file, (off, end)) in matcher.find_iter(&content).enumerate() {
            if per_file >= max_count {
                break;
            }
            let (line, slice) = byte_to_line(&content, off);
            // Bug 16: when a regex with `(?s)` (or any multi-line construct)
            // matches across newlines, render the whole matched span instead
            // of just the first line — otherwise the displayed text is
            // misleading (looks like only `foo` matched when the regex
            // really required both `foo` and `bar`).
            let text = if end > off && end <= content.len() && content[off..end].contains(&b'\n') {
                let snippet = &content[off..end];
                String::from_utf8_lossy(snippet).replace('\n', "\\n")
            } else {
                String::from_utf8_lossy(slice).into_owned()
            };
            local_hits.push(GrepHit {
                path: path.to_string_lossy().into_owned(),
                line,
                text,
            });
            // If this is the only hit we care about (files_with_matches mode),
            // we can short-circuit per file.
            if args.files_with_matches {
                break;
            }
        }

        if local_hits.is_empty() {
            return;
        }
        let prior = hit_counter.fetch_add(local_hits.len(), Ordering::Relaxed);
        if prior >= limit {
            stop.store(true, Ordering::Relaxed);
            return;
        }

        // De-dupe lines per file (regex can match same line multiple times).
        // Keep first occurrence.
        let mut seen_lines = std::collections::HashSet::new();
        local_hits.retain(|h| seen_lines.insert(h.line));

        if let Ok(mut guard) = hits_mutex.lock() {
            for h in local_hits {
                if guard.len() >= limit {
                    stop.store(true, Ordering::Relaxed);
                    break;
                }
                guard.push(h);
            }
        }
    });

    let mut hits = hits_mutex.into_inner().unwrap_or_default();
    hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    hits.truncate(limit);

    if args.files_with_matches {
        let mut paths: Vec<String> = hits.iter().map(|h| h.path.clone()).collect();
        paths.sort();
        paths.dedup();
        hits = paths
            .into_iter()
            .map(|p| GrepHit {
                path: p,
                line: 0,
                text: String::new(),
            })
            .collect();
    }

    // When --group is set, emit symbol-grouped output instead
    if args.group {
        let grouped = build_grouped_result(&args.needle, &hits, mode);
        return super::emit(format, &grouped, |p| {
            let mut out = String::new();
            if p.files.is_empty() {
                out.push_str(&format!("[no matches across {total_files} files]\n"));
                return out;
            }
            for f in &p.files {
                out.push_str(&format!(
                    "{} ({} matches, {} symbols)\n",
                    f.path, f.total_matches, f.total_symbols
                ));
                for g in &f.groups {
                    out.push_str(&format!(
                        "  {} {} @ L{}-L{}\n",
                        g.kind, g.name, g.start_line, g.end_line
                    ));
                    for m in &g.matches {
                        out.push_str(&format!("    - L{} {}\n", m.line, m.text));
                    }
                }
                out.push('\n');
            }
            out
        });
    }

    let payload = GrepResult {
        needle: args.needle,
        hits,
        total_files_searched: total_files,
        mode,
        schema: "v1",
    };
    super::emit(format, &payload, |p| {
        let mut out = String::new();
        for h in &p.hits {
            if h.line == 0 {
                out.push_str(&h.path);
                out.push('\n');
            } else {
                out.push_str(&format!("{}:{}: {}\n", h.path, h.line, h.text));
            }
        }
        if p.hits.is_empty() {
            out.push_str(&format!(
                "[no matches across {} files]\n",
                p.total_files_searched
            ));
        }
        out
    })
}

/* ─── Grouped output (--group flag) ─── */

/// A match grouped by its enclosing symbol.
#[derive(Debug, Serialize)]
struct GroupedMatch {
    line: u32,
    text: String,
}

/// A symbol group containing matches.
#[derive(Debug, Serialize)]
struct MatchGroup {
    kind: String,
    name: String,
    start_line: u32,
    end_line: u32,
    matches: Vec<GroupedMatch>,
}

/// Matches in a single file, with symbol groups.
#[derive(Debug, Serialize)]
struct FileGroup {
    path: String,
    total_matches: usize,
    total_symbols: usize,
    groups: Vec<MatchGroup>,
}

/// Enriched grep result with symbol-grouped output.
#[derive(Debug, Serialize)]
struct GroupedGrepResult {
    needle: String,
    total_files: usize,
    total_matches: usize,
    mode: &'static str,
    files: Vec<FileGroup>,
    schema: &'static str,
}

fn build_grouped_result(needle: &str, hits: &[GrepHit], mode: &'static str) -> GroupedGrepResult {
    // Group hits by file
    let mut by_file: std::collections::BTreeMap<String, Vec<&GrepHit>> =
        std::collections::BTreeMap::new();
    for h in hits {
        by_file.entry(h.path.clone()).or_default().push(h);
    }

    let mut files: Vec<FileGroup> = Vec::new();
    for (path, file_hits) in &by_file {
        // Try to parse the file outline for symbol grouping
        let content = ffs_search::bom::read_file(path).ok();
        let entries = content
            .as_deref()
            .map(get_simple_outline)
            .unwrap_or_default();

        let mut groups: Vec<MatchGroup> = Vec::new();
        let mut unmatched: Vec<GroupedMatch> = Vec::new();

        for hit in file_hits {
            let line = hit.line as usize;
            // Find enclosing symbol
            let enclosing = entries
                .iter()
                .find(|e| e.start_line <= line && line <= e.end_line);
            if let Some(sym) = enclosing {
                // Check if we already have a group for this symbol
                if let Some(g) = groups
                    .iter_mut()
                    .find(|g: &&mut MatchGroup| g.name == sym.name && g.kind == sym.kind)
                {
                    g.matches.push(GroupedMatch {
                        line: hit.line,
                        text: hit.text.clone(),
                    });
                } else {
                    groups.push(MatchGroup {
                        kind: sym.kind.clone(),
                        name: sym.name.clone(),
                        start_line: sym.start_line as u32,
                        end_line: sym.end_line as u32,
                        matches: vec![GroupedMatch {
                            line: hit.line,
                            text: hit.text.clone(),
                        }],
                    });
                }
            } else {
                unmatched.push(GroupedMatch {
                    line: hit.line,
                    text: hit.text.clone(),
                });
            }
        }

        // Put unmatched hits in a file-scope group
        if !unmatched.is_empty() {
            groups.push(MatchGroup {
                kind: "file".to_string(),
                name: "<file scope>".to_string(),
                start_line: 0,
                end_line: 0,
                matches: unmatched,
            });
        }

        files.push(FileGroup {
            path: path.clone(),
            total_matches: file_hits.len(),
            total_symbols: entries.len(),
            groups,
        });
    }

    GroupedGrepResult {
        needle: needle.to_string(),
        total_files: files.len(),
        total_matches: hits.len(),
        mode,
        files,
        schema: "v2_grouped",
    }
}

/// A simple structure item for grouping.
struct SymEntry {
    kind: String,
    name: String,
    start_line: usize,
    end_line: usize,
}

/// Get a simple outline from file content using regex-based parsing
/// (lightweight alternative to full tree-sitter outline).
fn get_simple_outline(text: &str) -> Vec<SymEntry> {
    let mut entries = Vec::new();
    let lines: Vec<&str> = text.lines().collect();

    // Detect language from shebang or common patterns
    let lang = if text.contains("fn ") && text.contains("struct ") && text.contains("impl ") {
        "rust"
    } else if text.contains("function ") || text.contains("const ") || text.contains("import ") {
        "typescript"
    } else {
        "generic"
    };

    for (i, line) in lines.iter().enumerate() {
        let line_num = i + 1;
        let trimmed = line.trim_start();

        match lang {
            "rust" => {
                // Functions: pub fn name(...
                if let Some(name) = parse_after_keyword(trimmed, "fn ") {
                    let end = find_block_end(&lines[i..], line_num);
                    entries.push(SymEntry {
                        kind: "function".into(),
                        name,
                        start_line: line_num,
                        end_line: end,
                    });
                }
                // Structs: struct Name { ...
                else if let Some(name) = parse_after_keyword(trimmed, "struct ") {
                    let end = find_block_end(&lines[i..], line_num);
                    entries.push(SymEntry {
                        kind: "struct".into(),
                        name,
                        start_line: line_num,
                        end_line: end,
                    });
                }
                // Enums: enum Name { ...
                else if let Some(name) = parse_after_keyword(trimmed, "enum ") {
                    let end = find_block_end(&lines[i..], line_num);
                    entries.push(SymEntry {
                        kind: "enum".into(),
                        name,
                        start_line: line_num,
                        end_line: end,
                    });
                }
                // Traits: trait Name { ...
                else if let Some(name) = parse_after_keyword(trimmed, "trait ") {
                    let end = find_block_end(&lines[i..], line_num);
                    entries.push(SymEntry {
                        kind: "trait".into(),
                        name,
                        start_line: line_num,
                        end_line: end,
                    });
                }
                // impl blocks
                else if let Some(name) = parse_after_keyword(trimmed, "impl ") {
                    // Extract just the type name (before the { or where)
                    let name = name.split(['{', 'w']).next().unwrap_or(&name).trim();
                    let end = find_block_end(&lines[i..], line_num);
                    entries.push(SymEntry {
                        kind: "impl".into(),
                        name: name.to_string(),
                        start_line: line_num,
                        end_line: end,
                    });
                }
            }
            "typescript" => {
                if let Some(name) = parse_after_keyword(trimmed, "function ") {
                    let end = find_ts_block_end(&lines[i..], line_num);
                    entries.push(SymEntry {
                        kind: "function".into(),
                        name,
                        start_line: line_num,
                        end_line: end,
                    });
                } else if let Some(name) = parse_after_keyword(trimmed, "class ") {
                    let end = find_ts_block_end(&lines[i..], line_num);
                    entries.push(SymEntry {
                        kind: "class".into(),
                        name,
                        start_line: line_num,
                        end_line: end,
                    });
                } else if let Some(name) = parse_after_keyword(trimmed, "interface ") {
                    let end = find_ts_block_end(&lines[i..], line_num);
                    entries.push(SymEntry {
                        kind: "interface".into(),
                        name,
                        start_line: line_num,
                        end_line: end,
                    });
                }
            }
            "generic" => {
                // Generic function detection for any language
                for kw in &["fn ", "def ", "func ", "function "] {
                    if let Some(name) = parse_after_keyword(trimmed, kw) {
                        entries.push(SymEntry {
                            kind: "definition".into(),
                            name,
                            start_line: line_num,
                            end_line: line_num + 5,
                        });
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    // Merge overlapping entries
    entries.sort_by_key(|a| a.start_line);
    entries
}

fn parse_after_keyword(line: &str, kw: &str) -> Option<String> {
    if !line.starts_with(kw) {
        // Also check with pub/export prefix
        let pub_prefixes = ["pub ", "pub(crate) ", "pub(super) ", "export "];
        for prefix in &pub_prefixes {
            if line.starts_with(prefix) {
                let after_prefix = line.strip_prefix(prefix)?;
                if after_prefix.starts_with(kw) {
                    return parse_after_keyword(after_prefix, kw);
                }
            }
        }
        return None;
    }
    let rest = line.strip_prefix(kw)?;
    // Extract name (up to (, <, :, {, whitespace)
    let name = rest.split(['(', '<', ':', '{', ' ']).next()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn find_block_end(lines: &[&str], start: usize) -> usize {
    let mut depth: i32 = 0;
    let mut first_brace = false;
    for (i, line) in lines.iter().enumerate() {
        let abs_line = start + i;
        for &b in line.as_bytes() {
            if b == b'{' {
                depth += 1;
                first_brace = true;
            } else if b == b'}' {
                depth -= 1;
            }
        }
        if first_brace && depth <= 0 {
            return abs_line;
        }
    }
    start + lines.len()
}

fn find_ts_block_end(lines: &[&str], start: usize) -> usize {
    find_block_end(lines, start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_regex_metachars() {
        assert!(looks_like_regex("foo.*bar"));
        assert!(looks_like_regex("^EXPORT"));
        assert!(looks_like_regex("a|b"));
        assert!(!looks_like_regex("EXPORT_SYMBOL_GPL"));
        assert!(!looks_like_regex("simple_word"));
    }

    #[test]
    fn byte_to_line_basic() {
        let h = b"first\nsecond\nthird\n";
        assert_eq!(byte_to_line(h, 0).0, 1);
        assert_eq!(byte_to_line(h, 6).0, 2);
        assert_eq!(byte_to_line(h, 13).0, 3);
    }
}
