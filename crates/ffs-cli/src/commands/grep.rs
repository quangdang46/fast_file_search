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
    fn find_iter<'a>(&'a self, haystack: &'a [u8]) -> Box<dyn Iterator<Item = (usize, usize)> + 'a> {
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
                    let positions: Vec<(usize, usize)> = finder
                        .find_iter(&lower)
                        .map(|p| (p, p + nlen))
                        .collect();
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
            let text = if end > off && end <= content.len()
                && content[off..end].contains(&b'\n')
            {
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
