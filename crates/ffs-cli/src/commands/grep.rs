use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use anyhow::Result;
use clap::Parser;
use memchr::memmem;
use serde::Serialize;

use crate::cli::OutputFormat;

/// A byte buffer that either owns its data (small files) or holds a live
/// memory map (larger files). Derefs to `&[u8]` so search code treats both
/// uniformly. Using mmap avoids the read+copy syscall for larger files —
/// ripgrep's `MmapChoice::Auto` does the same.
enum SearchBuffer {
    Owned(Vec<u8>),
    Mapped(memmap2::Mmap),
}

impl std::ops::Deref for SearchBuffer {
    type Target = [u8];
    #[inline]
    fn deref(&self) -> &[u8] {
        match self {
            SearchBuffer::Owned(v) => v,
            SearchBuffer::Mapped(m) => m,
        }
    }
}

/// Read a file's bytes for searching. Uses mmap for files above a small
/// threshold (avoids the read+copy syscall, like ripgrep's MmapChoice::Auto);
/// falls back to reading through the already-open handle for tiny files
/// (single open, single read — no second `File::open`).
fn read_for_search(path: &Path) -> std::io::Result<SearchBuffer> {
    use std::io::Read;
    const MMAP_THRESHOLD: u64 = 4 * 1024;
    let file = std::fs::File::open(path)?;
    let meta = file.metadata()?;
    if meta.len() < MMAP_THRESHOLD {
        let mut buf = Vec::with_capacity(meta.len() as usize);
        let mut file = file;
        file.read_to_end(&mut buf)?;
        return Ok(SearchBuffer::Owned(buf));
    }
    let map = unsafe { memmap2::Mmap::map(&file) }?;
    Ok(SearchBuffer::Mapped(map))
}

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
    /// Byte ranges `[start, end)` of each match within `text`, for terminal
    /// highlighting. Omitted from JSON when empty (e.g. `-l` mode).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    match_ranges: Vec<(u32, u32)>,
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

    /// Returns a lazy iterator of `(start, end)` byte offsets for each match.
    /// No intermediate allocation: matches are produced on demand, so `-l`
    /// mode can stop after the first match without scanning the whole file.
    fn find_iter<'a>(
        &'a self,
        haystack: &'a [u8],
    ) -> Box<dyn Iterator<Item = (usize, usize)> + 'a> {
        match self {
            Matcher::Literal {
                needle,
                case_insensitive,
            } => {
                let nlen = needle.len();
                if *case_insensitive {
                    // Case-insensitive ASCII: scan for (first, last) needle
                    // bytes via memchr2, then verify the interior with a fast
                    // case-folded compare. No lowercased copy, fully lazy.
                    let needle = needle.clone();
                    Box::new(CaseInsensitiveLiteralIter {
                        haystack,
                        needle,
                        pos: 0,
                    })
                } else {
                    // Case-sensitive literal: stream via memmem lazily (no
                    // intermediate position Vec).
                    Box::new(memmem::find_iter(haystack, needle).map(move |p| (p, p + nlen)))
                }
            }
            Matcher::Regex(re) => Box::new(re.find_iter(haystack).map(|m| (m.start(), m.end()))),
        }
    }

    /// Returns true if `haystack` contains at least one match. Used by
    /// files-with-matches mode: short-circuits at the first hit.
    fn is_match(&self, haystack: &[u8]) -> bool {
        match self {
            Matcher::Literal {
                needle,
                case_insensitive,
            } => {
                if *case_insensitive {
                    CaseInsensitiveLiteralIter {
                        haystack,
                        needle: needle.clone(),
                        pos: 0,
                    }
                    .next()
                    .is_some()
                } else {
                    memmem::find(haystack, needle).is_some()
                }
            }
            Matcher::Regex(re) => re.is_match(haystack),
        }
    }
}

/// Lazy case-insensitive ASCII literal iterator.
///
/// Finds candidate positions with `memchr2` on the needle's first byte
/// (both cases), then verifies the full needle with a case-folded compare
/// that relies on ASCII differing only in bit 0x20. Zero allocation.
struct CaseInsensitiveLiteralIter<'a> {
    haystack: &'a [u8],
    needle: Vec<u8>,
    pos: usize,
}

impl Iterator for CaseInsensitiveLiteralIter<'_> {
    type Item = (usize, usize);

    fn next(&mut self) -> Option<(usize, usize)> {
        if self.needle.is_empty() || self.haystack.len() < self.needle.len() {
            return None;
        }
        let first_lo = self.needle[0];
        let first_hi = first_lo.to_ascii_uppercase();
        let tail = &self.needle[1..];
        let max_pos = self.haystack.len() - self.needle.len();
        for pos in memchr::memchr2_iter(first_lo, first_hi, &self.haystack[self.pos..]) {
            let abs = self.pos + pos;
            if abs > max_pos {
                return None;
            }
            let candidate = &self.haystack[abs + 1..abs + self.needle.len()];
            if ascii_case_eq(candidate, tail) {
                self.pos = abs + 1;
                return Some((abs, abs + self.needle.len()));
            }
        }
        self.pos = self.haystack.len();
        None
    }
}

/// Fast ASCII case-insensitive byte-slice comparison (differ only in bit
/// 0x20). Both slices must be equal length.
fn ascii_case_eq(a: &[u8], b: &[u8]) -> bool {
    let len = a.len();
    let mut i = 0;
    while i + 8 <= len {
        let va = u64::from_ne_bytes(a[i..i + 8].try_into().unwrap());
        let vb = u64::from_ne_bytes(b[i..i + 8].try_into().unwrap());
        if va != vb {
            const MASK: u64 = 0x2020_2020_2020_2020;
            if (va | MASK) != (vb | MASK) {
                return false;
            }
        }
        i += 8;
    }
    while i < len {
        let ha = a[i];
        let hb = b[i];
        if ha != hb && (ha | 0x20) != (hb | 0x20) {
            return false;
        }
        i += 1;
    }
    true
}

/// Map a byte offset to `(1-based line number, byte offset of line start, line slice)`.
fn byte_to_line(haystack: &[u8], offset: usize) -> (u32, usize, &[u8]) {
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
    (line, line_start, &haystack[line_start..line_end])
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
    // `needle_bytes` is the case-folded literal we search for.
    let needle_bytes: &[u8] = match &matcher {
        Matcher::Literal { needle, .. } => needle.as_slice(),
        _ => &[],
    };
    let bigram = match &matcher {
        Matcher::Literal { needle, .. } if needle.len() >= 2 => {
            crate::cache::CacheDir::at(root).load_bigram_index(root)
        }
        _ => None,
    };

    // Bigram prefilter → a set of candidate paths; the search still walks the
    // whole tree ONCE (rg-style fused walk+search) and skips non-candidates via
    // O(1) membership. No prefilter → search every file in the walk. This keeps
    // a single parallel walk regardless of how scattered the candidates are.
    // `total_files` (the bug-18 denominator) is the whole workspace: from the
    // bigram cache when present, else derived from the walked tree.
    let candidate_paths: Option<std::collections::HashSet<PathBuf>> = match &bigram {
        Some(idx) => idx.filter(needle_bytes).map(|paths| {
            paths
                .into_iter()
                .map(PathBuf::from)
                .collect::<std::collections::HashSet<_>>()
        }),
        None => None,
    };
    // Bigram says the pattern cannot appear anywhere — short-circuit with no
    // matches instead of walking the whole tree.
    if candidate_paths.as_ref().is_some_and(|s| s.is_empty()) {
        let total_files = bigram.as_ref().map_or(0, |idx| idx.file_count());
        let payload = GrepResult {
            needle: args.needle,
            hits: Vec::new(),
            total_files_searched: total_files,
            mode,
            schema: "v1",
        };
        return super::emit(format, &payload, |p| {
            if p.hits.is_empty() {
                format!("[no matches across {} files]\n", p.total_files_searched)
            } else {
                String::new()
            }
        });
    }
    let total_files = bigram.as_ref().map_or_else(
        || {
            // No cache: count files with a full walk (bug 18 denominator).
            super::walk_files(root).len()
        },
        |idx| idx.file_count(),
    );
    let limit = args.limit;
    let max_count = if args.max_count == 0 {
        usize::MAX
    } else {
        args.max_count
    };

    let hits_mutex: Mutex<Vec<GrepHit>> = Mutex::new(Vec::new());
    let hit_counter = AtomicUsize::new(0);
    let stop = std::sync::atomic::AtomicBool::new(false);

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .min(8);

    let walker = ignore::WalkBuilder::new(root)
        .standard_filters(true)
        .follow_links(false)
        .threads(threads)
        .build_parallel();
    walker.run(|| {
        let matcher = &matcher;
        let hits_mutex = &hits_mutex;
        let hit_counter = &hit_counter;
        let stop = &stop;
        let candidate_paths = &candidate_paths;
        let files_with_matches = args.files_with_matches;
        let (limit, max_count) = (limit, max_count);
        Box::new(move |entry| {
            if stop.load(Ordering::Relaxed) {
                return ignore::WalkState::Quit;
            }
            let Ok(e) = entry else {
                return ignore::WalkState::Continue;
            };
            if !e.file_type().is_some_and(|t| t.is_file()) {
                return ignore::WalkState::Continue;
            }
            let path = e.into_path();
            // Bigram prefilter: skip files that cannot contain the needle.
            if let Some(cands) = candidate_paths {
                if !cands.contains(&path) {
                    return ignore::WalkState::Continue;
                }
            }

            let Ok(content) = read_for_search(&path) else {
                return ignore::WalkState::Continue;
            };

            // files-with-matches: only need to know IF the file matches.
            if files_with_matches {
                if matcher.is_match(&content) {
                    let prior = hit_counter.fetch_add(1, Ordering::Relaxed);
                    if prior >= limit {
                        stop.store(true, Ordering::Relaxed);
                        return ignore::WalkState::Quit;
                    }
                    if let Ok(mut guard) = hits_mutex.lock() {
                        if guard.len() >= limit {
                            stop.store(true, Ordering::Relaxed);
                            return ignore::WalkState::Quit;
                        }
                        guard.push(GrepHit {
                            path: path.to_string_lossy().into_owned(),
                            line: 0,
                            text: String::new(),
                            match_ranges: Vec::new(),
                        });
                    }
                }
                return ignore::WalkState::Continue;
            }
            // Quick binary heuristic: skip files containing NUL in the first 8KB.
            let probe = &content[..content.len().min(8 * 1024)];
            if probe.contains(&0u8) {
                return ignore::WalkState::Continue;
            }

            // Collect matches keyed by line so multiple matches on the same
            // line become a single hit carrying ALL match ranges.
            let mut by_line: std::collections::BTreeMap<u32, (String, Vec<(u32, u32)>)> =
                std::collections::BTreeMap::new();
            for (per_file, (off, end)) in matcher.find_iter(&content).enumerate() {
                if per_file >= max_count {
                    break;
                }
                let (line, line_start, slice) = byte_to_line(&content, off);
                // Bug 16: multiline match → render the whole span.
                let (text, range) =
                    if end > off && end <= content.len() && content[off..end].contains(&b'\n') {
                        let snippet = &content[off..end];
                        let text = String::from_utf8_lossy(snippet).replace('\n', "\\n");
                        let range = if text.is_empty() {
                            None
                        } else {
                            Some((0, text.len() as u32))
                        };
                        (text, range)
                    } else {
                        let text = String::from_utf8_lossy(slice).into_owned();
                        let s = off.saturating_sub(line_start);
                        let e = end.saturating_sub(line_start);
                        let len = text.len() as u32;
                        let range = if e > s {
                            Some((s.min(len as usize) as u32, e.min(len as usize) as u32))
                        } else {
                            None
                        };
                        (text, range)
                    };
                let entry = by_line.entry(line).or_insert_with(|| (text, Vec::new()));
                if let Some(r) = range {
                    entry.1.push(r);
                }
                // files-with-matches: first match per file is enough.
                if files_with_matches {
                    break;
                }
            }

            if by_line.is_empty() {
                return ignore::WalkState::Continue;
            }

            let local_hits: Vec<GrepHit> = by_line
                .into_iter()
                .map(|(line, (text, match_ranges))| GrepHit {
                    path: path.to_string_lossy().into_owned(),
                    line,
                    text,
                    match_ranges,
                })
                .collect();

            let prior = hit_counter.fetch_add(local_hits.len(), Ordering::Relaxed);
            if prior >= limit {
                stop.store(true, Ordering::Relaxed);
                return ignore::WalkState::Quit;
            }

            if let Ok(mut guard) = hits_mutex.lock() {
                for h in local_hits {
                    if guard.len() >= limit {
                        stop.store(true, Ordering::Relaxed);
                        return ignore::WalkState::Quit;
                    }
                    guard.push(h);
                }
            }
            ignore::WalkState::Continue
        })
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
                match_ranges: Vec::new(),
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
        let path_spec = super::render::path_spec();
        let line_spec = super::render::line_spec();
        for h in &p.hits {
            if h.line == 0 {
                out.push_str(&super::render::colorize(&h.path, &path_spec));
                out.push('\n');
            } else {
                out.push_str(&super::render::colorize(&h.path, &path_spec));
                out.push(':');
                out.push_str(&super::render::colorize(&h.line.to_string(), &line_spec));
                out.push_str(": ");
                out.push_str(&super::render::colorize_matches(&h.text, &h.match_ranges));
                out.push('\n');
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
        // line_start follows the last newline before the offset
        assert_eq!(byte_to_line(h, 6).1, 6);
        assert_eq!(byte_to_line(h, 13).1, 13);
    }
}
