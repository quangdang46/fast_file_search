//! Phase B of the @-mention system: payload resolution.
//!
//! Given a list of paths selected by the host (CLI / MCP / C ABI / D provider),
//! `resolve_mentions` reads each one, classifies it (text / binary / image /
//! directory), applies a `FilterLevel`, applies a `line_range` slice, then
//! `smart_truncate`s to fit the caller's token budget. Results include an
//! `audit` field for debugging expensive resolutions.
//!
//! `MentionResolverCache` is a tiny dedup-by-turn cache so the same path
//! resolved twice in a single host turn returns the cached `ResolvedMention`
//! without re-reading the file or re-applying the filter pipeline.
//!
//! This module is intentionally self-contained: it does not depend on the
//! Phase A `MentionCandidate` type (which lives in `ffs-core::mention`),
//! because Phase A is not part of this worktree base. Hosts pass in a slice
//! of `PathBuf` and get back a `Vec<ResolvedMention>` of the same length.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use ffs_budget::{
    estimate_tokens, smart_truncate, tokens_to_bytes, AggressiveFilter, FilterLevel,
    FilterStrategy, MinimalFilter, NoFilter, TruncationOutcome,
};
use serde::{Deserialize, Serialize};

/// Maximum bytes read for binary detection. The probe never reads more than
/// this so a 10GB file does not block the resolver.
const BINARY_PROBE_BYTES: usize = 8 * 1024;

/// Maximum entries in a directory listing. Keeps the resolved payload bounded.
const DIRECTORY_LISTING_CAP: usize = 100;

/// Image extensions recognized by the resolver. Anything matching is marked
/// `MentionKind::Image` and never opened as text.
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// MIME type for each recognized image extension.
fn image_mime_for(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// The kind of resource a @-mention resolves to in Phase B.
///
/// `External(&'static str)` from Phase A is intentionally absent: Phase B
/// only resolves filesystem resources. Phase D will reintroduce the
/// provider trait; for now anything not in {File, Directory, Image} falls
/// through to a plain `File` with `content = None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MentionKind {
    File,
    Directory,
    Image,
}

/// Audit information attached to every `ResolvedMention`. Used by the host
/// to surface "why did this read cost so much" without re-walking the
/// resolver pipeline.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MentionAudit {
    /// Pipeline phase that produced this audit (always "B" for now;
    /// Phase D will tag external provider passes differently).
    pub phase: String,
    /// Token cost of the content *before* truncation. Lets the caller
    /// see how much `smart_truncate` actually saved.
    pub tokens_before_truncate: u32,
    /// Set when the resolver hit an I/O or classification error.
    /// `content` will be `None` and `kind` will reflect what the resolver
    /// was *able* to determine (often `File` with no body).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Caller-tunable knobs for a single resolve pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveOptions {
    /// Maximum tokens of *body* allowed for each resolved mention. The
    /// resolver converts to a byte budget internally. Default 50_000.
    pub max_tokens: u32,
    /// Comment / whitespace stripping intensity. Default `Minimal`.
    pub filter_level: FilterLevel,
    /// Optional `(start_line, end_line)` 1-based inclusive slice. Applied
    /// *before* `smart_truncate` so the line range is honored even when
    /// the underlying file is large.
    pub line_range: Option<(u32, u32)>,
}

impl Default for ResolveOptions {
    fn default() -> Self {
        Self {
            max_tokens: 50_000,
            filter_level: FilterLevel::Minimal,
            line_range: None,
        }
    }
}

/// The resolved payload for a single @-mention.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedMention {
    /// Absolute path the resolver actually read. Always set; if the host
    /// passed a relative path, it was canonicalized against the process
    /// CWD before resolution.
    pub path: PathBuf,
    pub kind: MentionKind,
    pub line_range: Option<(u32, u32)>,
    pub size: u64,
    pub is_binary: bool,
    /// Text body. `None` for binary / image / directory, or for an
    /// unreadable file (in which case `audit.error` is set).
    pub content: Option<String>,
    /// Base64-encoded image body. Always `None` in Phase B — the resolver
    /// does not load image bytes; it only sets `image_mime` so the host
    /// can decide whether to base64-encode later. The field exists so the
    /// shape is stable for Phase C (MCP) and Phase D (external providers).
    pub image_base64: Option<String>,
    /// Image MIME for the recognized extension. `None` for non-images.
    pub image_mime: Option<String>,
    /// Set when `smart_truncate` actually dropped bytes. Lets the host
    /// surface a "truncated, [N more lines]" hint to the user.
    pub truncation: Option<TruncationOutcome>,
    /// Token cost of the *final* (post-truncation, post-filter) content.
    /// Zero when `content` is `None`.
    pub token_cost: u32,
    pub audit: MentionAudit,
}

/// Resolve a batch of paths in order. Returns one `ResolvedMention` per
/// input path, in the same order. Never panics: every error becomes a
/// `ResolvedMention { content: None, audit.error: Some(..) }`.
pub fn resolve_mentions(paths: &[PathBuf], opts: &ResolveOptions) -> Vec<ResolvedMention> {
    paths.iter().map(|p| resolve_one(p, opts)).collect()
}

fn resolve_one(path: &Path, opts: &ResolveOptions) -> ResolvedMention {
    // Canonicalize so the caller always sees an absolute path. On error
    // (path doesn't exist, permission denied) we still return a mention
    // with kind=File and content=None + audit.error.
    let abs = match fs::canonicalize(path) {
        Ok(p) => p,
        Err(e) => {
            return ResolvedMention {
                path: path.to_path_buf(),
                kind: MentionKind::File,
                line_range: opts.line_range,
                size: 0,
                is_binary: false,
                content: None,
                image_base64: None,
                image_mime: None,
                truncation: None,
                token_cost: 0,
                audit: MentionAudit {
                    phase: "B".to_string(),
                    tokens_before_truncate: 0,
                    error: Some(e.to_string()),
                },
            };
        }
    };

    let metadata = match fs::metadata(&abs) {
        Ok(m) => m,
        Err(e) => {
            return ResolvedMention {
                path: abs,
                kind: MentionKind::File,
                line_range: opts.line_range,
                size: 0,
                is_binary: false,
                content: None,
                image_base64: None,
                image_mime: None,
                truncation: None,
                token_cost: 0,
                audit: MentionAudit {
                    phase: "B".to_string(),
                    tokens_before_truncate: 0,
                    error: Some(e.to_string()),
                },
            };
        }
    };

    // Directory short-circuit: list immediate children, do not recurse.
    if metadata.is_dir() {
        return resolve_directory(&abs, &metadata);
    }

    let size = metadata.len();
    let ext = abs
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // Image short-circuit: mark Image, do not open as text.
    if IMAGE_EXTS.contains(&ext.as_str()) {
        return ResolvedMention {
            path: abs,
            kind: MentionKind::Image,
            line_range: opts.line_range,
            size,
            is_binary: true,
            content: None,
            image_base64: None,
            image_mime: image_mime_for(&ext).map(|s| s.to_string()),
            truncation: None,
            token_cost: 0,
            audit: MentionAudit {
                phase: "B".to_string(),
                tokens_before_truncate: 0,
                error: None,
            },
        };
    }

    // Regular file. Binary-detection probe first, then text path.
    match read_and_classify(&abs) {
        ClassifiedFile::Text(content) => resolve_text(&abs, size, content, opts),
        ClassifiedFile::Binary => ResolvedMention {
            path: abs,
            kind: MentionKind::File,
            line_range: opts.line_range,
            size,
            is_binary: true,
            content: None,
            image_base64: None,
            image_mime: None,
            truncation: None,
            token_cost: 0,
            audit: MentionAudit {
                phase: "B".to_string(),
                tokens_before_truncate: 0,
                error: None,
            },
        },
        ClassifiedFile::IoError(e) => ResolvedMention {
            path: abs,
            kind: MentionKind::File,
            line_range: opts.line_range,
            size,
            is_binary: false,
            content: None,
            image_base64: None,
            image_mime: None,
            truncation: None,
            token_cost: 0,
            audit: MentionAudit {
                phase: "B".to_string(),
                tokens_before_truncate: 0,
                error: Some(e),
            },
        },
    }
}

enum ClassifiedFile {
    Text(String),
    Binary,
    IoError(String),
}

fn read_and_classify(path: &Path) -> ClassifiedFile {
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return ClassifiedFile::IoError(e.to_string()),
    };
    let mut probe = [0u8; BINARY_PROBE_BYTES];
    let probe_len = match file.read(&mut probe) {
        Ok(n) => n,
        Err(e) => return ClassifiedFile::IoError(e.to_string()),
    };
    if probe[..probe_len].contains(&0u8) {
        return ClassifiedFile::Binary;
    }
    // Re-open and read the whole file as text. We use lossy UTF-8 so
    // a stray multi-byte sequence in an otherwise-text file still
    // produces *some* string the host can show.
    let full = match fs::read(path) {
        Ok(b) => b,
        Err(e) => return ClassifiedFile::IoError(e.to_string()),
    };
    ClassifiedFile::Text(String::from_utf8_lossy(&full).into_owned())
}

fn resolve_directory(path: &Path, metadata: &fs::Metadata) -> ResolvedMention {
    let entries = match fs::read_dir(path) {
        Ok(rd) => rd,
        Err(e) => {
            return ResolvedMention {
                path: path.to_path_buf(),
                kind: MentionKind::Directory,
                line_range: None,
                size: 0,
                is_binary: false,
                content: None,
                image_base64: None,
                image_mime: None,
                truncation: None,
                token_cost: 0,
                audit: MentionAudit {
                    phase: "B".to_string(),
                    tokens_before_truncate: 0,
                    error: Some(e.to_string()),
                },
            };
        }
    };

    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();

    let truncated = names.len() > DIRECTORY_LISTING_CAP;
    if truncated {
        names.truncate(DIRECTORY_LISTING_CAP);
    }
    let body = names.join("\n");
    let body_with_marker = if truncated {
        format!(
            "{}\n[{} more entries truncated]\n",
            body,
            DIRECTORY_LISTING_CAP // upper bound on hidden count
        )
    } else {
        body
    };
    let final_len = body_with_marker.len();

    let token_cost = estimate_tokens(final_len as u64) as u32;
    ResolvedMention {
        path: path.to_path_buf(),
        kind: MentionKind::Directory,
        line_range: None,
        size: metadata.len(),
        is_binary: false,
        content: Some(body_with_marker),
        image_base64: None,
        image_mime: None,
        truncation: if truncated {
            Some(TruncationOutcome {
                kept_lines: DIRECTORY_LISTING_CAP,
                dropped_lines: 0, // unknown; cap was reached
                kept_bytes: final_len,
                footer_bytes: 0,
            })
        } else {
            None
        },
        token_cost,
        audit: MentionAudit {
            phase: "B".to_string(),
            tokens_before_truncate: token_cost,
            error: None,
        },
    }
}

fn resolve_text(path: &Path, size: u64, raw: String, opts: &ResolveOptions) -> ResolvedMention {
    // 1. Apply line range slice (1-based, inclusive on both ends).
    let sliced = match opts.line_range {
        Some((start, end)) if start > 0 => slice_lines(&raw, start, end),
        _ => raw.clone(),
    };

    // 2. Apply filter.
    let filtered = apply_filter(&sliced, opts.filter_level);

    // 3. Truncate to token budget. Convert tokens -> bytes using the
    //    same 4-bytes-per-token estimate the budget module uses
    //    everywhere else, so the resolver and `read` agree on what
    //    "fits in 50k tokens" means.
    let max_bytes = tokens_to_bytes(u64::from(opts.max_tokens)) as usize;
    let (body, truncation) = smart_truncate(&filtered, max_bytes);

    let tokens_before = estimate_tokens(filtered.len() as u64) as u32;
    let token_cost = estimate_tokens(body.len() as u64) as u32;

    ResolvedMention {
        path: path.to_path_buf(),
        kind: MentionKind::File,
        line_range: opts.line_range,
        size,
        is_binary: false,
        content: Some(body),
        image_base64: None,
        image_mime: None,
        truncation: Some(truncation),
        token_cost,
        audit: MentionAudit {
            phase: "B".to_string(),
            tokens_before_truncate: tokens_before,
            error: None,
        },
    }
}

fn apply_filter(input: &str, level: FilterLevel) -> String {
    match level {
        FilterLevel::None => NoFilter.apply(input),
        FilterLevel::Minimal => MinimalFilter.apply(input),
        FilterLevel::Aggressive => AggressiveFilter.apply(input),
    }
}

/// Slice `input` to the inclusive 1-based line range `[start, end]`. Out-
/// of-range `start` past EOF yields an empty string; `end` is clamped to
/// the last line. We work in `split('\n')` and re-join so the trailing
/// newline behaviour matches the rest of the budget pipeline.
fn slice_lines(input: &str, start: u32, end: u32) -> String {
    let lines: Vec<&str> = input.split('\n').collect();
    let s = (start as usize).saturating_sub(1).min(lines.len());
    let e = (end as usize).min(lines.len());
    if s >= e {
        return String::new();
    }
    lines[s..e].join("\n")
}

// ---------------------------------------------------------------------------
// Dedup-by-turn cache
// ---------------------------------------------------------------------------

/// Lightweight per-turn memoization of `resolve_mentions`.
///
/// Hosts create one `MentionResolverCache` at the start of a turn, call
/// `resolve_cached` for each path, and drop the cache at the end of the
/// turn. `turn_id == 0` disables caching (every call hits the resolver).
///
/// The cache is **not** thread-safe — wrap in `Mutex` if multiple agents
/// share it. Phase D will add a `DashMap` if the trait surface demands it.
#[derive(Debug, Default)]
pub struct MentionResolverCache {
    /// Per-turn entries. Keyed by absolute path so two relative paths
    /// pointing at the same file dedup correctly.
    by_turn: HashMap<u64, HashMap<PathBuf, ResolvedMention>>,
    /// Most recent `turn_id` passed to `resolve_cached`. Used as a
    /// micro-optimization so the common case ("still in turn N") skips
    /// a `HashMap::get` on `by_turn`.
    last_turn: u64,
}

impl MentionResolverCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve `path` (or return the cached `ResolvedMention` if `path`
    /// was already resolved in this `turn_id`).
    ///
    /// `opts` is *ignored* on a cache hit — the cached result was built
    /// with whatever options were used the first time. Hosts that need
    /// option-aware dedup should pass a synthetic cache key (e.g. hash
    /// of `opts`) instead of using this convenience method.
    pub fn resolve_cached(
        &mut self,
        path: &Path,
        opts: &ResolveOptions,
        turn_id: u64,
    ) -> ResolvedMention {
        if turn_id == 0 {
            return resolve_one(path, opts);
        }

        if self.last_turn != turn_id {
            // Stale turn: clear and re-key.
            self.by_turn.clear();
            self.last_turn = turn_id;
        }

        let bucket = self.by_turn.entry(turn_id).or_default();

        // Canonicalize so a path string like "./foo.rs" and "/abs/foo.rs"
        // dedup correctly. If canonicalize fails, use the path verbatim
        // and proceed; the resolver itself will also try to canonicalize.
        let key = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

        if let Some(hit) = bucket.get(&key) {
            return hit.clone();
        }

        let resolved = resolve_one(&key, opts);
        bucket.insert(key, resolved.clone());
        resolved
    }

    /// Number of paths currently memoized across all known turns.
    /// Test-only.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.by_turn.values().map(|b| b.len()).sum()
    }

    /// Drop all cached entries. Call between turns if you'd rather not
    /// rely on the implicit clear in `resolve_cached`.
    pub fn clear(&mut self) {
        self.by_turn.clear();
        self.last_turn = 0;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    /// Build a temp dir containing the requested files. Returns the
    /// directory handle and a vec of absolute paths matching `files`.
    fn write_files(spec: &[(&str, &[u8])]) -> (tempfile::TempDir, Vec<PathBuf>) {
        let dir = tempdir().expect("tempdir");
        let mut paths = Vec::new();
        for (name, body) in spec {
            let p = dir.path().join(name);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).expect("mkdir -p");
            }
            let mut f = fs::File::create(&p).expect("create");
            f.write_all(body).expect("write");
            paths.push(p);
        }
        (dir, paths)
    }

    #[test]
    fn text_file_resolved_with_content_and_token_cost() {
        let (dir, paths) = write_files(&[("hello.rs", b"fn main() {}\n")]);
        let opts = ResolveOptions::default();
        let out = resolve_mentions(&paths, &opts);
        assert_eq!(out.len(), 1);
        let m = &out[0];
        assert_eq!(m.kind, MentionKind::File);
        assert!(!m.is_binary);
        assert!(m.content.as_deref().unwrap().contains("fn main"));
        assert!(m.token_cost > 0);
        assert_eq!(m.audit.phase, "B");
        // Token cost should equal estimate_tokens of the post-filter
        // post-truncate body length.
        let body = m.content.as_deref().unwrap();
        assert_eq!(m.token_cost as u64, estimate_tokens(body.len() as u64));
        // The path was canonicalized to an absolute path.
        assert!(m.path.starts_with(dir.path()));
    }

    #[test]
    fn binary_file_detected_content_none() {
        // 0x00 byte in the first 8KB => binary.
        let (dir, paths) = write_files(&[("blob.bin", &[0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9])]);
        let opts = ResolveOptions::default();
        let out = resolve_mentions(&paths, &opts);
        let m = &out[0];
        assert_eq!(m.kind, MentionKind::File);
        assert!(m.is_binary);
        assert!(m.content.is_none());
        assert_eq!(m.token_cost, 0);
        assert!(m.audit.error.is_none());
        assert!(m.path.starts_with(dir.path()));
    }

    #[test]
    fn image_marked_image_with_mime() {
        let (dir, paths) = write_files(&[("logo.png", b"\x89PNG\r\n\x1a\n")]);
        let opts = ResolveOptions::default();
        let out = resolve_mentions(&paths, &opts);
        let m = &out[0];
        assert_eq!(m.kind, MentionKind::Image);
        assert_eq!(m.image_mime.as_deref(), Some("image/png"));
        assert!(m.is_binary);
        assert!(m.content.is_none());
        assert_eq!(m.image_base64, None);
        assert!(m.path.starts_with(dir.path()));
    }

    #[test]
    fn jpeg_mime_jpeg() {
        let (_dir, paths) = write_files(&[("a.jpg", b"not really a jpg")]);
        let out = resolve_mentions(&paths, &ResolveOptions::default());
        assert_eq!(out[0].image_mime.as_deref(), Some("image/jpeg"));
    }

    #[test]
    fn jpeg_extension_uppercase_mime_jpeg() {
        let (_dir, paths) = write_files(&[("a.JPEG", b"x")]);
        let out = resolve_mentions(&paths, &ResolveOptions::default());
        assert_eq!(out[0].image_mime.as_deref(), Some("image/jpeg"));
    }

    #[test]
    fn webp_mime() {
        let (_dir, paths) = write_files(&[("a.webp", b"x")]);
        let out = resolve_mentions(&paths, &ResolveOptions::default());
        assert_eq!(out[0].image_mime.as_deref(), Some("image/webp"));
    }

    #[test]
    fn directory_resolved_with_listing() {
        let dir = tempdir().expect("tempdir");
        for n in ["a.rs", "b.rs", "c.txt"] {
            fs::write(dir.path().join(n), b"x").unwrap();
        }
        let p = dir.path().to_path_buf();
        let out = resolve_mentions(&[p.clone()], &ResolveOptions::default());
        let m = &out[0];
        assert_eq!(m.kind, MentionKind::Directory);
        let body = m.content.as_deref().expect("dir body");
        assert!(body.contains("a.rs"));
        assert!(body.contains("b.rs"));
        assert!(body.contains("c.txt"));
        assert!(m.token_cost > 0);
    }

    #[test]
    fn line_range_applied_before_truncation() {
        let body: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        let (dir, paths) = write_files(&[("x.txt", body.as_bytes())]);
        let opts = ResolveOptions {
            line_range: Some((5, 7)),
            ..ResolveOptions::default()
        };
        let out = resolve_mentions(&paths, &opts);
        let m = &out[0];
        let c = m.content.as_deref().unwrap();
        assert!(c.contains("line 5"));
        assert!(c.contains("line 6"));
        assert!(c.contains("line 7"));
        assert!(!c.contains("line 4"));
        assert!(!c.contains("line 8"));
        assert_eq!(m.line_range, Some((5, 7)));
        assert!(m.path.starts_with(dir.path()));
    }

    #[test]
    fn smart_truncate_applied_when_content_exceeds_budget() {
        // Build a 200-line file; ask for a budget that fits ~20 lines.
        let body: String = (1..=200).map(|i| format!("line {i}\n")).collect();
        let (_dir, paths) = write_files(&[("big.txt", body.as_bytes())]);
        // 200 bytes total budget fits a handful of lines, well under
        // 200 lines. The footer should be present.
        let opts = ResolveOptions {
            max_tokens: 10, // 40 bytes via tokens_to_bytes
            ..ResolveOptions::default()
        };
        let out = resolve_mentions(&paths, &opts);
        let m = &out[0];
        let c = m.content.as_deref().unwrap();
        assert!(
            c.contains("more lines]"),
            "expected truncation footer in: {c}"
        );
        let trunc = m.truncation.as_ref().expect("truncation outcome");
        assert!(trunc.dropped_lines > 0);
    }

    #[test]
    fn no_truncation_when_content_fits() {
        let (_dir, paths) = write_files(&[("small.txt", b"hello\n")]);
        let opts = ResolveOptions {
            max_tokens: 50_000,
            ..ResolveOptions::default()
        };
        let out = resolve_mentions(&paths, &opts);
        let m = &out[0];
        let trunc = m.truncation.as_ref().unwrap();
        assert_eq!(trunc.dropped_lines, 0);
    }

    #[test]
    fn nonexistent_file_returns_error_audit() {
        let bogus = PathBuf::from("/this/path/does/not/exist/abcxyz.rs");
        let out = resolve_mentions(&[bogus.clone()], &ResolveOptions::default());
        let m = &out[0];
        assert_eq!(m.kind, MentionKind::File);
        assert!(m.content.is_none());
        assert!(m.audit.error.is_some());
        // We record the input path verbatim when canonicalize fails.
        assert_eq!(m.path, bogus);
    }

    #[test]
    fn dedup_same_turn_returns_cached_result() {
        let (_dir, paths) = write_files(&[("a.rs", b"fn a() {}\n")]);
        let p = paths[0].clone();
        let opts = ResolveOptions::default();
        let mut cache = MentionResolverCache::new();

        let r1 = cache.resolve_cached(&p, &opts, 42);
        let r2 = cache.resolve_cached(&p, &opts, 42);
        // The two results should be byte-identical (cache hit clones).
        assert_eq!(r1.path, r2.path);
        assert_eq!(r1.content, r2.content);
        assert_eq!(r1.token_cost, r2.token_cost);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn dedup_turn_zero_disables_cache() {
        let (_dir, paths) = write_files(&[("a.rs", b"fn a() {}\n")]);
        let p = paths[0].clone();
        let opts = ResolveOptions::default();
        let mut cache = MentionResolverCache::new();
        let _ = cache.resolve_cached(&p, &opts, 0);
        let _ = cache.resolve_cached(&p, &opts, 0);
        // turn_id == 0 must NOT populate the cache.
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn different_turns_do_not_share_cache() {
        let (_dir, paths) = write_files(&[("a.rs", b"fn a() {}\n")]);
        let p = paths[0].clone();
        let opts = ResolveOptions::default();
        let mut cache = MentionResolverCache::new();
        let _ = cache.resolve_cached(&p, &opts, 1);
        assert_eq!(cache.len(), 1);
        // Switching turn_id drops the previous turn's bucket.
        let _ = cache.resolve_cached(&p, &opts, 2);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn filter_levels_change_output() {
        // A Rust source file with a triple-slash doc comment, a regular
        // line comment, and a block comment. Minimal keeps ///, strips //.
        // Aggressive strips everything.
        let body = b"/// keep me\n// strip me\n/* block */\nfn x() {}\n";
        let (_dir, paths) = write_files(&[("x.rs", body)]);

        let minimal = resolve_mentions(
            &paths,
            &ResolveOptions {
                filter_level: FilterLevel::Minimal,
                ..ResolveOptions::default()
            },
        );
        let aggressive = resolve_mentions(
            &paths,
            &ResolveOptions {
                filter_level: FilterLevel::Aggressive,
                ..ResolveOptions::default()
            },
        );
        let none = resolve_mentions(
            &paths,
            &ResolveOptions {
                filter_level: FilterLevel::None,
                ..ResolveOptions::default()
            },
        );

        let m = minimal[0].content.as_deref().unwrap();
        assert!(m.contains("/// keep me"), "minimal must keep doc: {m}");
        assert!(!m.contains("strip me"), "minimal must strip // line: {m}");

        let a = aggressive[0].content.as_deref().unwrap();
        assert!(!a.contains("keep me"), "aggressive must strip /// doc: {a}");
        assert!(!a.contains("block"), "aggressive must strip /* */: {a}");

        let n = none[0].content.as_deref().unwrap();
        assert!(n.contains("keep me"));
        assert!(n.contains("strip me"));
        assert!(n.contains("block"));
    }

    #[test]
    fn nonexistent_directory_returns_error() {
        let bogus = PathBuf::from("/no/such/dir/zzzqqq");
        let out = resolve_mentions(&[bogus], &ResolveOptions::default());
        // The canonicalize step fails before we ever get to the dir
        // branch, so kind=File + audit.error is the contract.
        let m = &out[0];
        assert!(m.audit.error.is_some());
    }

    #[test]
    fn binary_detection_probe_uses_first_8kb() {
        // Place NUL only after the probe window: first 10KB are pure
        // ASCII, then 4KB of zeros. The probe sees no NUL => treated as
        // text, so the resolver opens it as text. This is the documented
        // trade-off: cheap probe, possible false negative on huge files.
        let mut body = vec![b'a'; 10 * 1024];
        body.extend(std::iter::repeat(0u8).take(4 * 1024));
        let (_dir, paths) = write_files(&[("tricky.bin", &body)]);
        let out = resolve_mentions(&paths, &ResolveOptions::default());
        let m = &out[0];
        assert!(!m.is_binary, "binary detection must only look at first 8KB");
        assert!(m.content.is_some());
    }

    #[test]
    fn resolve_options_default_is_minimal_50k_tokens() {
        let o = ResolveOptions::default();
        assert_eq!(o.max_tokens, 50_000);
        assert_eq!(o.filter_level, FilterLevel::Minimal);
        assert_eq!(o.line_range, None);
    }
}
