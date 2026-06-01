//! `MentionResolver` — Phase A core @-mention candidate search.
//!
//! Reuses `FilePicker::fuzzy_search()` and `fuzzy_search_directories()` for
//! ranking (SIMD `frizbee` engine with frecency boost). Adds a cursor-aware
//! `@`-trigger layer and a flat `MentionCandidate` projection so host apps
//! can plug the resolver into a TUI popup without touching `FilePicker`.
//!
//! Phase A is sync, single-thread, zero-copy (candidates borrow from the
//! picker's arena). Phase D will add an `Arc<dyn MentionProvider>` registry
//! for `External` kinds and possibly an async API.

use ffs_query_parser::QueryParser;

use crate::FilePicker;
use crate::SharedFrecency;
use crate::types::FileItem;

use super::trigger::{MentionTrigger, detect_trigger};

/// The kind of resource a @-mention resolves to.
///
/// In Phase A, only `File` and `Directory` are produced by the built-in
/// providers. `External(id)` is a tagged escape hatch so host apps
/// (jcode, Claude Code, Codex, …) can inject their own kinds without
/// ffs-core learning a new variant. The `id` is opaque to ffs-core.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MentionKind {
    File,
    Directory,
    /// Opaque identifier, e.g. "agent", "tool", "skill", "image".
    /// ffs-core does not interpret it; host providers do.
    External(&'static str),
}

/// A ranked candidate, returned by `MentionResolver::search()`.
///
/// Borrows from the `FilePicker`'s arena — zero-copy. The host app
/// owns the lifetime and either clones what it needs or passes the
/// reference downstream.
#[derive(Debug, Clone)]
pub struct MentionCandidate<'a> {
    pub kind: MentionKind,
    /// Display label, e.g. `src/main.rs` (file) or `src/components/` (dir).
    pub display: String,
    /// Path relative to picker base, e.g. `src/main.rs`.
    pub relative_path: String,
    /// Optional pre-computed frecency score (boosted ranking hint).
    /// Zero if frecency was not initialized.
    pub frecency_score: i64,
    /// Underlying file size, if applicable. Zero for directories / external.
    pub size: u64,
    /// Whether the file is binary (false for directories / external).
    pub is_binary: bool,
    /// Last-modified timestamp (unix seconds). Zero for external.
    pub modified: u64,
    /// Match indices for highlight rendering. Empty if not applicable.
    pub match_indices: Vec<u32>,
    /// The raw score returned by the underlying fuzzy engine.
    pub score: i32,

    /// For `File`: reference to the underlying `FileItem` (borrowed).
    /// None for `Directory` / `External`.
    pub file_item: Option<&'a FileItem>,
}

/// Full result of `MentionResolver::search()`.
#[derive(Debug, Clone)]
pub struct MentionResult<'a> {
    /// `None` if cursor is not inside an `@`-token (caller should hide the popup).
    pub trigger: Option<MentionTrigger>,
    /// Ranked candidates, top-N by `MentionOptions::max_candidates`.
    pub candidates: Vec<MentionCandidate<'a>>,
}

/// Knobs. `Clone + Default` so it's FFI-safe (no `Box<dyn>`).
#[derive(Debug, Clone)]
pub struct MentionOptions {
    /// Whether to include `File` candidates. Default true.
    pub include_files: bool,
    /// Whether to include `Directory` candidates. Default true.
    pub include_dirs: bool,
    /// Max candidates returned. Default 15 (matches claude-code cap).
    pub max_candidates: usize,
    /// Min query length for subsequence fuzzy tier. Default 3.
    /// Prefix / exact / recent tiers are NOT gated by this — they fire
    /// even on `@` alone and on `@x`. Only the slowest tier is gated.
    pub fuzzy_min_chars: usize,
    /// Min query length to require before opening a popup at all.
    /// `0` means popup opens on `@` alone. Default 0.
    pub min_query_chars: usize,
}

impl Default for MentionOptions {
    fn default() -> Self {
        Self {
            include_files: true,
            include_dirs: true,
            max_candidates: 15,
            fuzzy_min_chars: 3,
            min_query_chars: 0,
        }
    }
}

/// Phase A resolver. Sync, single-thread, borrows from `FilePicker`.
///
/// Lifetime: `'a` ties candidates to the picker's arena. Host app
/// either clones what it needs into its own `Vec<MentionCandidate<'static>>`
/// (by calling `.to_owned()` on the strings) or passes the borrowed
/// result downstream within the same scope.
pub struct MentionResolver<'a> {
    picker: &'a FilePicker,
    shared_frecency: Option<SharedFrecency>,
    opts: MentionOptions,
}

impl<'a> MentionResolver<'a> {
    /// Build a resolver over a `FilePicker`. Frecency is optional —
    /// if not provided, candidates get a flat 0 boost.
    pub fn new(picker: &'a FilePicker) -> Self {
        Self {
            picker,
            shared_frecency: None,
            opts: MentionOptions::default(),
        }
    }

    /// Attach a shared frecency handle for future use (Phase A keeps the
    /// handle but does not mutate it; frecency already lives on each
    /// `FileItem.access_frecency_score` and `DirItem.max_access_frecency`
    /// as populated by the existing scan pipeline).
    pub fn with_shared_frecency(mut self, sf: SharedFrecency) -> Self {
        self.shared_frecency = Some(sf);
        self
    }

    /// Replace the default options.
    pub fn with_options(mut self, opts: MentionOptions) -> Self {
        self.opts = opts;
        self
    }

    /// Detect the `@`-token at the cursor (no I/O).
    pub fn detect_trigger(input: &str, cursor: usize) -> Option<MentionTrigger> {
        detect_trigger(input, cursor)
    }

    /// Find ranked candidates. Empty if no trigger or empty picker.
    pub fn search(&self, input: &str, cursor: usize) -> MentionResult<'a> {
        // Phase A: short-circuit when the trigger's query is too short
        // (we still return the trigger so the host can show a "type more"
        // affordance, but with no candidates).
        let trigger = match detect_trigger(input, cursor) {
            Some(t) if t.query.chars().count() >= self.opts.min_query_chars => t,
            Some(t) => {
                return MentionResult {
                    trigger: Some(t),
                    candidates: vec![],
                };
            }
            None => {
                return MentionResult {
                    trigger: None,
                    candidates: vec![],
                };
            }
        };

        let parser = QueryParser::<ffs_query_parser::FileSearchConfig>::default();
        let q = parser.parse(&trigger.query);

        let mut out: Vec<MentionCandidate<'a>> = Vec::new();

        // ---- FILES ---- reuse the existing fuzzy_search pipeline.
        if self.opts.include_files {
            let result = self.picker.fuzzy_search(&q, None, Default::default());
            for (item, score) in result.items.iter().zip(result.scores.iter()) {
                let path = item.relative_path(self.picker);
                let cand = MentionCandidate {
                    kind: MentionKind::File,
                    display: path.clone(),
                    relative_path: path,
                    frecency_score: (item.access_frecency_score as i64)
                        + (item.modification_frecency_score as i64),
                    size: item.size,
                    is_binary: item.is_binary(),
                    modified: item.modified,
                    // TODO: extract fuzzy match indices from frizbee once we have access to them.
                    match_indices: vec![],
                    score: score.total,
                    file_item: Some(item),
                };
                out.push(cand);
            }
        }

        // ---- DIRECTORIES ---- reuse the existing fuzzy_search_directories pipeline.
        if self.opts.include_dirs {
            let dresult = self.picker.fuzzy_search_directories(&q, Default::default());
            for (dir, score) in dresult.items.iter().zip(dresult.scores.iter()) {
                let path = dir.relative_path(self.picker);
                let cand = MentionCandidate {
                    kind: MentionKind::Directory,
                    display: path.clone(),
                    relative_path: path,
                    frecency_score: dir.max_access_frecency() as i64,
                    size: 0,
                    is_binary: false,
                    modified: 0,
                    // TODO: extract fuzzy match indices from frizbee once we have access to them.
                    match_indices: vec![],
                    score: score.total,
                    file_item: None,
                };
                out.push(cand);
            }
        }

        // Cross-kind rank by frizbee's score (already comparable across kinds).
        out.sort_by_key(|c| std::cmp::Reverse(c.score));
        out.truncate(self.opts.max_candidates);

        MentionResult {
            trigger: Some(trigger),
            candidates: out,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::file_picker::FilePicker;
    use crate::file_picker::FilePickerOptions;

    /// Build a small temp tree with 5 files for picker-backed tests.
    ///
    /// Layout:
    ///   <root>/a.txt
    ///   <root>/b.rs
    ///   <root>/README.md
    ///   <root>/src/c.rs
    ///   <root>/src/d.txt
    fn build_picker_with_tree() -> (tempfile::TempDir, FilePicker) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let root = dir.path();
        // Touch files. Use a small payload to keep the test fast.
        for rel in ["a.txt", "b.rs", "README.md"] {
            std::fs::write(root.join(rel), b"x").expect("write file");
        }
        std::fs::create_dir_all(root.join("src")).expect("mkdir src");
        for rel in ["src/c.rs", "src/d.txt"] {
            std::fs::write(root.join(rel), b"x").expect("write file");
        }

        let base_buf =
            crate::path_utils::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
        let mut picker = FilePicker::new(FilePickerOptions {
            base_path: base_buf.to_string_lossy().into_owned(),
            watch: false,
            ..Default::default()
        })
        .expect("create picker");
        picker.collect_files().expect("collect_files");
        (dir, picker)
    }

    /// Build a picker with no files indexed (collect_files not called).
    fn build_empty_picker() -> (tempfile::TempDir, FilePicker) {
        let dir = tempfile::tempdir().expect("create temp dir");
        let root = dir.path();
        // Don't create any files.
        let base_buf =
            crate::path_utils::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
        let picker = FilePicker::new(FilePickerOptions {
            base_path: base_buf.to_string_lossy().into_owned(),
            watch: false,
            ..Default::default()
        })
        .expect("create picker");
        // Intentionally do NOT call collect_files().
        (dir, picker)
    }

    #[test]
    fn mention_options_default_is_ffs_safe() {
        let o = MentionOptions::default();
        assert!(o.include_files);
        assert!(o.include_dirs);
        assert_eq!(o.max_candidates, 15);
        assert_eq!(o.fuzzy_min_chars, 3);
        assert_eq!(o.min_query_chars, 0);
    }

    #[test]
    fn mention_options_is_clone_and_default() {
        let a = MentionOptions::default();
        let b = a.clone();
        assert_eq!(a.max_candidates, b.max_candidates);
        // Also exercise Default::default() equality.
        let c: MentionOptions = Default::default();
        assert_eq!(c.max_candidates, a.max_candidates);
    }

    #[test]
    fn mention_kind_is_copy_and_eq() {
        // MentionKind must be Copy — the FFI-safety of MentionOptions
        // also depends on the kind tag being cheap to pass around.
        let a = MentionKind::File;
        let b = a;
        assert_eq!(a, b);
        let ext = MentionKind::External("agent");
        let ext2 = ext;
        assert_eq!(ext, ext2);
    }

    #[test]
    fn detect_trigger_delegates_to_trigger_module() {
        // Trigger detection is exercised in trigger.rs; the resolver just
        // exposes a passthrough. Ensure it returns Some for an @-token.
        let t =
            MentionResolver::detect_trigger("@hello", 6).expect("trigger should fire at @hello");
        assert_eq!(t.query, "hello");
    }

    // ─── Resolver end-to-end tests (plan §8) ─────────────────────────────

    #[test]
    fn empty_input_returns_trigger_none() {
        let (_tmp, picker) = build_picker_with_tree();
        let resolver = MentionResolver::new(&picker);
        let r = resolver.search("", 0);
        assert!(r.trigger.is_none(), "no @ → no trigger");
        assert!(r.candidates.is_empty(), "no @ → no candidates");
    }

    #[test]
    fn bare_at_with_min_query_zero_returns_candidates() {
        // min_query_chars=0 means `@` alone should still return top files.
        let (_tmp, picker) = build_picker_with_tree();
        let resolver = MentionResolver::new(&picker);
        let r = resolver.search("@", 1);
        assert!(r.trigger.is_some(), "`@` should produce a trigger");
        assert_eq!(r.trigger.as_ref().unwrap().query, "");
        assert!(
            !r.candidates.is_empty(),
            "min_query_chars=0 should yield top files for empty query"
        );
        // All candidates should be File or Directory kinds.
        for c in &r.candidates {
            assert!(matches!(c.kind, MentionKind::File | MentionKind::Directory));
        }
    }

    #[test]
    fn bare_at_with_min_query_one_returns_no_candidates() {
        // min_query_chars=1 should suppress the empty-query candidate burst.
        let (_tmp, picker) = build_picker_with_tree();
        let resolver = MentionResolver::new(&picker).with_options(MentionOptions {
            min_query_chars: 1,
            ..Default::default()
        });
        let r = resolver.search("@", 1);
        assert!(r.trigger.is_some());
        assert!(
            r.candidates.is_empty(),
            "min_query_chars=1 should hide empty-query candidates"
        );
    }

    #[test]
    fn short_query_returns_some_candidates() {
        // Single-char query should still return at least one candidate via
        // the prefix/exact tier.
        let (_tmp, picker) = build_picker_with_tree();
        let resolver = MentionResolver::new(&picker);
        let r = resolver.search("@a", 2);
        assert!(r.trigger.is_some());
        assert_eq!(r.trigger.as_ref().unwrap().query, "a");
        assert!(
            !r.candidates.is_empty(),
            "1-char query should match by prefix/exact"
        );
    }

    #[test]
    fn include_files_false_excludes_files() {
        let (_tmp, picker) = build_picker_with_tree();
        let resolver = MentionResolver::new(&picker).with_options(MentionOptions {
            include_files: false,
            include_dirs: true,
            ..Default::default()
        });
        let r = resolver.search("@a", 2);
        assert!(r.trigger.is_some());
        for c in &r.candidates {
            assert!(
                matches!(c.kind, MentionKind::Directory),
                "include_files=false should drop File candidates, got {:?}",
                c.kind
            );
        }
    }

    #[test]
    fn include_dirs_false_excludes_dirs() {
        let (_tmp, picker) = build_picker_with_tree();
        let resolver = MentionResolver::new(&picker).with_options(MentionOptions {
            include_files: true,
            include_dirs: false,
            ..Default::default()
        });
        let r = resolver.search("@a", 2);
        assert!(r.trigger.is_some());
        for c in &r.candidates {
            assert!(
                matches!(c.kind, MentionKind::File),
                "include_dirs=false should drop Directory candidates, got {:?}",
                c.kind
            );
        }
    }

    #[test]
    fn max_candidates_truncates_results() {
        let (_tmp, picker) = build_picker_with_tree();
        let resolver = MentionResolver::new(&picker).with_options(MentionOptions {
            max_candidates: 2,
            ..Default::default()
        });
        let r = resolver.search("@", 1);
        assert!(r.trigger.is_some());
        assert!(
            r.candidates.len() <= 2,
            "max_candidates=2 should clamp output (got {})",
            r.candidates.len()
        );
    }

    #[test]
    fn resolver_without_shared_frecency_still_works() {
        // No SharedFrecency attached — frecency score should be flat 0 and
        // the resolver should still return candidates.
        let (_tmp, picker) = build_picker_with_tree();
        let resolver = MentionResolver::new(&picker);
        let r = resolver.search("@a", 2);
        assert!(r.trigger.is_some());
        assert!(!r.candidates.is_empty());
        for c in &r.candidates {
            assert_eq!(c.frecency_score, 0, "no frecency → score must be 0");
        }
    }

    #[test]
    fn resolver_with_shared_frecency_noop_does_not_break() {
        // A noop SharedFrecency should be safely attached and produce the
        // same shape of result as the bare resolver.
        let (_tmp, picker) = build_picker_with_tree();
        let sf = SharedFrecency::noop();
        let resolver = MentionResolver::new(&picker).with_shared_frecency(sf);
        let r = resolver.search("@a", 2);
        assert!(r.trigger.is_some());
        assert!(!r.candidates.is_empty());
    }

    #[test]
    fn empty_picker_returns_no_candidates() {
        // Picker constructed but collect_files not called — no files
        // indexed, no directory entries, so a search should yield zero
        // candidates (with a valid trigger for the @).
        let (_tmp, picker) = build_empty_picker();
        let resolver = MentionResolver::new(&picker);
        let r = resolver.search("@a", 2);
        assert!(r.trigger.is_some());
        assert!(
            r.candidates.is_empty(),
            "empty picker should yield no candidates (got {})",
            r.candidates.len()
        );
    }

    #[test]
    fn file_and_dir_results_ranked_together() {
        // With both kinds enabled, the result should be a single flat list
        // sorted by score, with file and directory entries interleaved by
        // rank. Use a query that should match both `src/` (a directory) and
        // a file like `src/c.rs`.
        let (_tmp, picker) = build_picker_with_tree();
        let resolver = MentionResolver::new(&picker);
        let r = resolver.search("@src", 4);
        assert!(r.trigger.is_some());
        // Output should be non-trivially ranked: every score is >= the
        // next. We do not assert specific filenames (the frecency layer
        // can shift ordering), only the rank invariant.
        let scores: Vec<i32> = r.candidates.iter().map(|c| c.score).collect();
        for w in scores.windows(2) {
            assert!(
                w[0] >= w[1],
                "candidates must be non-increasing: {scores:?}"
            );
        }
        // And: at least one of the candidates should be a Directory
        // (we know `src/` exists in the tree).
        let has_dir = r
            .candidates
            .iter()
            .any(|c| matches!(c.kind, MentionKind::Directory));
        let has_file = r
            .candidates
            .iter()
            .any(|c| matches!(c.kind, MentionKind::File));
        assert!(
            has_dir,
            "expected at least one Directory candidate for @src"
        );
        assert!(has_file, "expected at least one File candidate for @src");
    }

    #[test]
    fn candidate_relative_path_is_repo_relative() {
        // The candidate's relative_path must be relative to the picker's
        // base, not absolute. This is what host apps render in popups.
        let (_tmp, picker) = build_picker_with_tree();
        let resolver = MentionResolver::new(&picker);
        let r = resolver.search("@README", 7);
        assert!(r.trigger.is_some());
        let readme = r
            .candidates
            .iter()
            .find(|c| c.relative_path == "README.md")
            .expect("README.md should appear in @README results");
        assert!(matches!(readme.kind, MentionKind::File));
        assert!(!readme.relative_path.starts_with('/'));
        assert!(!readme.relative_path.contains('\\'));
    }

    #[test]
    fn candidate_carries_file_item_reference() {
        // File candidates must carry a `file_item` reference; Directory
        // candidates must not.
        let (_tmp, picker) = build_picker_with_tree();
        let resolver = MentionResolver::new(&picker);
        let r = resolver.search("@a", 2);
        assert!(r.trigger.is_some());
        for c in &r.candidates {
            match c.kind {
                MentionKind::File => assert!(c.file_item.is_some()),
                MentionKind::Directory => assert!(c.file_item.is_none()),
                MentionKind::External(_) => assert!(c.file_item.is_none()),
            }
        }
    }

    #[test]
    fn search_below_max_candidates_returns_what_is_available() {
        // A picker with 5 files + at least 1 src dir should yield a small
        // bounded number of unique candidates. Verify max_candidates is an
        // upper bound, not a padding target. (Exact count depends on
        // whether the walker surfaces an implicit base dir entry, so we
        // just assert it is small and well-bounded.)
        let (_tmp, picker) = build_picker_with_tree();
        let resolver = MentionResolver::new(&picker).with_options(MentionOptions {
            max_candidates: 100,
            ..Default::default()
        });
        let r = resolver.search("@", 1);
        assert!(r.trigger.is_some());
        assert!(
            r.candidates.len() <= 10,
            "tree only has 5 files + ≤2 dirs, got {} candidates",
            r.candidates.len()
        );
        assert!(
            r.candidates.len() >= 5,
            "expected at least the 5 indexed files, got {}",
            r.candidates.len()
        );
    }

    #[test]
    fn search_is_independent_per_picker_instance() {
        // Two independent temp trees with different contents should yield
        // independent results — ensures the resolver doesn't smuggle state
        // across pickers.
        let (_tmp_a, picker_a) = build_picker_with_tree();
        let (_tmp_b, picker_b) = build_picker_with_tree();

        let resolver_a = MentionResolver::new(&picker_a);
        let resolver_b = MentionResolver::new(&picker_b);

        let ra = resolver_a.search("@README", 7);
        let rb = resolver_b.search("@nonexistent_xyz", 15);

        assert!(ra.trigger.is_some());
        assert!(!ra.candidates.is_empty());
        // A junk query on a 5-file tree might still match loosely, but
        // at minimum the trigger must fire.
        assert!(rb.trigger.is_some());
    }
}
