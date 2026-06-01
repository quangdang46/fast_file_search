# Implementation Plan: Unified @-Mention System for ffs (v2 — restructured)

> Generated from research across 9 reference repos + user interview + review
> Goal: make `ffs_search::FilePicker` the fastest mention candidate search backend in the AI-coding-agent space. **ffs is search, not agent runtime.**

**v2 changes from v1** (after review):
1. 4 phases (A→D), not 3. Phase A is core search only; surfaces (CLI/MCP/C-ABI) move to Phase C.
2. **No** `mention_options` field in `FilePickerOptions` (avoids source-breaking on exhaustive struct literals).
3. **Reuse** existing `FrecencyTracker`/`SharedFrecency` (LMDB-backed). No new `frecency.jsonl`.
4. `MentionOptions` is `Clone + Default` only — no `Box<dyn>`. Provider list lives on `MentionResolver`.
5. `MentionResolver` is **sync** in Phase A; no parallel provider fan-out. Phase D evaluates async.
6. `fuzzy_min_chars` gates **subsequence only**; empty/`@` returns recent/prefix, `@x` returns prefix match.
7. Phase A is **File + Directory** only. Agent/tool/skill/image → `MentionKind::External(String)`, registered by host (jcode/Claude Code/Codex/etc.) via Phase D provider protocol.
8. C ABI: `ffs_mention_search_json(handle, input, cursor, options_json) → FfsResult` first. Typed C structs come **after** JSON ABI stabilizes (deferred to post-v0.2.0).

**Companion docs:**
- `docs/FILE_PICKER_INIT_LATENCY_PLAN.md` — cold/warm scan perf (Option A incremental readiness).
- This plan — *frontend-facing* `@` candidate search, layered on top of the same scan.

---

## 1. Executive Summary

ffs is fast at the *scan* but its `@` surface today is just `fuzzy_search()` called with `@`-prefixed query text. Reference repos (codex, opencode, claude-code/CCB, oh-my-pi, codebuff) converge on a layered pipeline: **cursor-aware trigger detect → tiered candidate search → structured dispatch → context attachment with token-cost discipline**. v1 over-scoped: it conflated candidate search (the part that belongs in ffs) with payload resolution, agent dispatch, IDE bridge, and surface plumbing (all of which belong downstream). v2 splits cleanly: **ffs owns the candidate search; the host app owns the rest**.

Phase A ships a `MentionResolver` that:
- Detects the `@`-token at the cursor (cursor-aware, boundary-checked, rejects email/URL/mid-word/escaped)
- Returns ranked `MentionCandidate { File | Directory | External }` borrowing from `FilePicker` (zero-copy)
- Reuses the existing LMDB-backed `FrecencyTracker` (boosted via `FileItem.access_frecency_score` already populated per scan)
- Reuses `FilePicker::fuzzy_search()` and `fuzzy_search_directories()` for ranking (the SIMD-accelerated frizbee engine already there)
- Exposes a `MentionOptions` struct that is `Clone + Default` (no `Box<dyn>`), so the API is trivially usable from C/FFI bindings
- Does **not** read files, resolve payloads, dispatch agents, or own any UX — host app decides

Result: ffs becomes the best candidate-search backend for `@`-mentions without becoming an agent platform. jcode/Claude Code/Codex/Cursor/etc. each get to add their own `agent`/`tool`/`skill` providers externally.

**Targets:**
- Warm p99 per-keystroke: **< 10ms** on 10k files (reuses frizbee)
- Cold p99 per-keystroke: **< 50ms** on 10k files (reuses existing scan)
- Time-to-first-result on 40k cold: **< 100ms** (paired with latency plan)
- Zero source-breaking change (CLAUDE.md)

---

## 2. Architecture Decision

### Chosen Approach — Borrow from existing pipeline, add `@`-trigger layer

**Pattern is borrowed verbatim** from the 5 HIGH-relevance reference repos, but minimal:
- **Trigger detection**: cursor-aware, boundary-checked `@` regex (codex `ends_plaintext_at_mention` + opencode `mentionTriggerIndex` + pi-agent-rust `is_file_ref_boundary`).
- **Candidate ranking**: reuse `FilePicker::fuzzy_search()` and `fuzzy_search_directories()` directly. These already use the SIMD `frizbee` engine with frecency boost.
- **Source mix**: Phase A = File + Directory (from `FilePicker`). Phase D adds `External(String)` for host-registered kinds.
- **Tiered fuzzy**: reuse what `frizbee` already does (typo-tolerant subsequence). We don't reimplement — we expose what's there through the `MentionResolver` API.

### Alternatives Considered

| Approach | Source | Pros | Cons | Decision |
|----------|--------|------|------|----------|
| Reuse `fuzzy_search()` directly | this repo's `frizbee` | Zero new code for ranking; SIMD-accelerated; frecency-aware already | Bound to existing scoring semantics | **Adopt** |
| New tiered fuzzy scorer | codex, opencode, claude-code | Full control over prefix > substring > subsequence bonuses | Reinventing what frizbee does; breaks perf characteristics | **Reject** |
| New `frecency.jsonl` + own store | opencode | Decouples from LMDB | LMDB already there; ranking inconsistency between ffs find and ffs mention | **Reject** (review fix #2) |
| `Box<dyn MentionProvider>` in `MentionOptions` | v1 plan | All-in-one config | Not `Clone`; not FFI-safe | **Reject** (review fix #3) |
| `mention_options` field in `FilePickerOptions` | v1 plan | One place to configure | Source-breaking for exhaustive struct literal users | **Reject** (review fix #1) |
| Parallel async provider fan-out | v1 plan | Fastest theoretical | `FilePicker` is `!Sync`; `SharedPicker` is the gateway; complex lifetime | **Defer** to Phase D (review fix #5) |
| Payload resolution + line ranges + binary policy in ffs-core | v1 plan | All-in-one | `ffs-budget` already owns token-aware reading; mixing concerns | **Reject** — move to Phase B (review fix #4) |
| File + Directory + Agent + Tool + Skill + Image kinds in Phase A | v1 plan | "Complete" types | Host-specific; drags ffs into agent platform | **Reject** — Phase A is File+Directory, `External(String)` for the rest (review fix #7) |
| Typed C structs (`FfsMentionResult` + nested) in Phase A | v1 plan | Strongly typed | Massive ABI freeze before API stable | **Reject** — JSON C ABI first, typed later (review fix #8) |

---

## 3. Data Structures & Types

```rust
// crates/ffs-core/src/mention/mod.rs (NEW, Phase A only)

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

/// The trigger token extracted from the input at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionTrigger {
    /// Byte offset of the `@` in the input.
    pub start: usize,
    /// Byte offset one past the last byte of the query (== cursor if `query` is empty).
    pub end: usize,
    /// Text after `@` (empty for `@` alone). Always unescaped.
    pub query: String,
    /// `@"foo bar"` form: path is `foo bar`, not `foo`.
    pub quoted: bool,
    /// Line-range suffix parsed: `(start_line, end_line)` if `@path#L10-20`.
    pub line_range: Option<(u32, u32)>,
}

/// Full result of `search_mentions()`.
#[derive(Debug, Clone)]
pub struct MentionResult<'a> {
    /// `None` if cursor is not inside an `@`-token (caller should hide the popup).
    pub trigger: Option<MentionTrigger>,
    /// Ranked candidates, top-N by `MentionOptions::max_candidates`.
    pub candidates: Vec<MentionCandidate<'a>>,
}

/// Knobs. `Clone + Default` so it's FFI-safe (no `Box<dyn>`).
/// No method takes `&self` mutably after construction; behavior is
/// captured at `MentionResolver` build time, not at every call.
#[derive(Debug, Clone, Default)]
pub struct MentionOptions {
    /// Whether to include `File` candidates. Default true.
    pub include_files: bool,
    /// Whether to include `Directory` candidates. Default true.
    pub include_dirs: bool,
    /// Max candidates returned. Default 15 (matches claude-code cap).
    pub max_candidates: usize,
    /// Min query length for subsequence fuzzy tier. Default 3.
    /// Prefix / exact / recent tiers are NOT gated by this — they fire
    /// even on `@` alone (returning recent + top files) and on `@x`
    /// (returning prefix matches). Only the slowest tier is gated.
    pub fuzzy_min_chars: usize,
    /// Min query length to require before opening a popup at all.
    /// `0` means popup opens on `@` alone. Default 0.
    pub min_query_chars: usize,
}
```

```rust
// crates/ffs-core/src/mention/resolver.rs (NEW, Phase A)

use crate::frecency::SharedFrecency;
use crate::FilePicker;

/// Phase A resolver. Sync, single-thread. Borrows from `FilePicker`.
///
/// Phase D will add an `Arc<dyn MentionProvider>` registry for
/// `External` kinds; the Phase A struct already leaves room for that
/// without breaking the Phase A API.
pub struct MentionResolver<'a> {
    picker: &'a FilePicker,
    shared_frecency: Option<SharedFrecency>,
    opts: MentionOptions,
}

impl<'a> MentionResolver<'a> {
    /// Build a resolver over a `FilePicker`. Frecency is optional —
    /// if not provided, candidates get a flat 0 boost.
    pub fn new(picker: &'a FilePicker) -> Self { ... }
    pub fn with_shared_frecency(mut self, sf: SharedFrecency) -> Self { ... }
    pub fn with_options(mut self, opts: MentionOptions) -> Self { ... }

    /// Detect the `@`-token at the cursor (no I/O).
    pub fn detect_trigger(input: &str, cursor: usize) -> Option<MentionTrigger> { ... }

    /// Find ranked candidates. Empty if no trigger or empty picker.
    ///
    /// Phase A is sync: no parallel provider fan-out. Phase D may
    /// introduce async if host-registered external providers require
    /// network I/O; the API will then gain an `async fn`.
    pub fn search(&self, input: &str, cursor: usize) -> MentionResult<'a> { ... }
}
```

**No changes to:**
- `FilePickerOptions` (no `mention_options` field)
- `FilePicker` (no `search_mentions()` method on the picker itself in Phase A — the resolver is the entry point and holds the picker borrow, matching the `fuzzy_search()` lifetime pattern that already exists)
- `SharedFrecency` (reuse as-is)
- C ABI existing exports
- MCP existing tools

---

## 4. Pseudocode — Core Algorithm

### 4.1 Trigger detection (boundary-safe, cursor-anchored)

```
FUNCTION detect_trigger(input: str, cursor: usize) -> Option<MentionTrigger>:
    # clamp cursor to byte boundary
    cur = min(cursor, input.len())
    # walk back from cursor, char-by-char, looking for the @ that opens this token
    i = cur
    while i > 0:
        prev = previous char boundary before i in input
        ch = input[prev..i].chars().next()
        if ch == '@':
            # boundary check: char before @ must be whitespace, open bracket/quote, or start
            boundary_ok = (prev == 0) or
                          input[..prev].chars().last() in {whitespace, '(', '[', '{', '<', '"', '\''}
            if not boundary_ok: return None
            # escaped @ (i.e. \@) was already rejected: the char before @ would be '\\'
            # which is NOT in the boundary set
            raw = input[prev+1..cur]
            (query, quoted, line_range) = parse_mention_suffix(raw)
            return Some(MentionTrigger { start: prev, end: cur, query, quoted, line_range })
        if ch in {whitespace, ')', ']', '}', '>', '"', '\''}:
            return None  # crossed a token boundary; no @ in this token
        i = prev
    return None

FUNCTION parse_mention_suffix(raw: str) -> (query, quoted, line_range):
    # quoted form: @"foo bar" — for paths with whitespace
    if raw.starts_with('"'):
        close = find matching '"' in raw[1..] or return (raw, false, None)
        return (raw[1..close+1], true, None)
    # line range suffix: @path#L10 or @path#L10-20
    if let Some(hash_idx) = raw.find('#'):
        path_part = raw[..hash_idx]
        if let Some(rest) = raw[hash_idx+1..].strip_prefix("L"):
            (start_line, end_line) = parse_line_range(rest) or (path_part, false, None)
            return (path_part, false, Some((start_line, end_line or start_line)))
    return (raw, false, None)
```

### 4.2 Candidate ranking (reuses `fuzzy_search`)

```
FUNCTION search(resolver, input, cursor) -> MentionResult:
    trigger = detect_trigger(input, cursor)
    if trigger is None: return MentionResult { trigger: None, candidates: [] }
    if trigger.query.len() < resolver.opts.min_query_chars:
        return MentionResult { trigger: Some(trigger), candidates: [] }

    out: Vec<MentionCandidate> = []

    # ---- FILES ----
    if resolver.opts.include_files:
        # Build FfsQuery from the trigger text (no @ prefix). Phase A passes
        # the raw text as a single fuzzy query — line range / quoted form are
        # resolved by the host, not the candidate ranker.
        q = FfsQuery::parse(trigger.query)
        # fuzzy_min_chars gating: only the fuzzy SEARCH below is gated.
        # The query is always passed; frizbee's own per-result threshold
        # does the right thing (long query = stricter, short = loose).
        result = resolver.picker.fuzzy_search(q, query_tracker=None, options=Default)
        for r in result.items:
            cand = MentionCandidate {
                kind = File
                display = r.path_str()
                relative_path = r.path_str()
                frecency_score = r.access_frecency_score + r.modification_frecency_score
                size = r.size
                is_binary = r.is_binary()
                modified = r.modified
                match_indices = r.indices.clone()  # frizbee provides these
                score = r.score
                file_item = Some(r.item_ref)
            }
            out.push(cand)

    # ---- DIRECTORIES ----
    if resolver.opts.include_dirs:
        dq = FfsQuery::parse(trigger.query)
        dresult = resolver.picker.fuzzy_search_directories(dq, options=Default)
        for r in dresult.items:
            cand = MentionCandidate {
                kind = Directory
                display = r.path_str()
                relative_path = r.path_str()
                frecency_score = r.max_access_frecency
                size = 0
                is_binary = false
                modified = 0
                match_indices = r.indices.clone()
                score = r.score
                file_item = None
            }
            out.push(cand)

    # ---- RANK ACROSS KINDS, KEEP TOP-N ----
    # frizbee's score is already comparable across files and dirs
    # (both go through the same ScoringContext). Sort by score desc.
    out.sort_by(|a, b| b.score.cmp(&a.score))
    out.truncate(resolver.opts.max_candidates)

    return MentionResult { trigger: Some(trigger), candidates: out }
```

**Why no new tiered fuzzy in Phase A:** `fuzzy_search` already uses the SIMD `frizbee` engine with typo tolerance, frecency boost, and bonus scoring. Adding a parallel tiered scorer would either duplicate it (perf regression) or replace it (lose the existing tuned ranking). The point of the resolver is to *expose* the existing pipeline under a `@`-triggered API, not to fork it.

**Why `fuzzy_min_chars` only gates subsequence:** frizbee's own threshold already handles short queries gracefully — it returns recent + prefix matches for `@x`. The `fuzzy_min_chars` knob is a Phase A escape hatch for callers who want to suppress subsequence fallback specifically (e.g. performance-sensitive TUI that wants only top-N prefix hits on `@`). Default 0 means "let frizbee decide".

---

## 5. Implementation Code (Phase A only)

### 5.1 `crates/ffs-core/src/mention/mod.rs`

```rust
//! Cursor-aware @-mention candidate search for file/directory autocomplete.
//!
//! Phase A: File + Directory kinds only, sync, zero-copy, reuses the
//! existing `fuzzy_search` pipeline and `FrecencyTracker`. Host apps
//! add `External` providers in Phase D.

mod resolver;
mod trigger;

pub use resolver::{MentionResolver, MentionOptions, MentionResult, MentionCandidate, MentionKind, MentionTrigger};
pub use trigger::detect_trigger;
```

### 5.2 `crates/ffs-core/src/mention/trigger.rs`

Cursor-aware `@` token detection. Rejects:
- email: `user@host`
- URL: `https://...`
- mid-word: `foo@bar` (char before `@` is not whitespace/punctuation)
- escaped: `\@foo` (char before `@` is `\\`)
- in-string: `(@"` (closing quote already passed)

```rust
pub fn detect_trigger(input: &str, cursor: usize) -> Option<MentionTrigger> {
    let cur = cursor.min(input.len());
    let mut i = cur;
    while i > 0 {
        let prev = prev_char_boundary(input.as_bytes(), i);
        let ch = input[prev..i].chars().next()?;
        if ch == '@' {
            // Boundary check.
            let boundary_ok = if prev == 0 {
                true
            } else {
                let before_prev = prev_char_boundary(input.as_bytes(), prev);
                let b = input[before_prev..prev].chars().next()?;
                b.is_whitespace() || matches!(b, '(' | '[' | '{' | '<' | '"' | '\'')
            };
            if !boundary_ok { return None; }
            // Escaped \@ — the char before @ is '\\', not in boundary set, so
            // boundary_ok already rejected it. Defensive check for clarity:
            if prev > 0 && input.as_bytes().get(prev - 1).copied() == Some(b'\\') {
                return None;
            }
            let raw = &input[prev + 1..cur];
            let (query, quoted, line_range) = parse_mention_suffix(raw);
            return Some(MentionTrigger { start: prev, end: cur, query, quoted, line_range });
        }
        if ch.is_whitespace() || matches!(ch, ')' | ']' | '}' | '>' | '"' | '\'') {
            return None;
        }
        i = prev;
    }
    None
}

fn parse_mention_suffix(raw: &str) -> (String, bool, Option<(u32, u32)>) {
    // Quoted form: @"foo bar"
    if let Some(rest) = raw.strip_prefix('"') {
        if let Some(close) = rest.find('"') {
            return (rest[..close].to_string(), true, None);
        }
        return (raw.to_string(), false, None);  // unterminated; treat as raw
    }
    // Line range: @path#L10 or @path#L10-20
    if let Some(hash_idx) = raw.find('#') {
        let path_part = &raw[..hash_idx];
        if let Some(rest) = raw[hash_idx + 1..].strip_prefix('L') {
            if let Some((s, e)) = parse_line_range(rest) {
                return (path_part.to_string(), false, Some((s, e)));
            }
        }
    }
    (raw.to_string(), false, None)
}

fn parse_line_range(s: &str) -> Option<(u32, u32)> {
    // accepts "10", "10-20", "10-"
    let mut parts = s.splitn(2, '-');
    let start: u32 = parts.next()?.parse().ok()?;
    let end: u32 = match parts.next() {
        Some("") | None => start,
        Some(rest) => rest.parse().ok()?,
    };
    Some((start, end))
}
```

### 5.3 `crates/ffs-core/src/mention/resolver.rs`

```rust
use crate::frecency::SharedFrecency;
use crate::query_parser::FfsQuery;
use crate::FilePicker;
use crate::types::{FileItem, DirItem};
use super::trigger::detect_trigger;
use super::{MentionCandidate, MentionKind, MentionOptions, MentionResult, MentionTrigger};

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
    pub fn new(picker: &'a FilePicker) -> Self {
        Self { picker, shared_frecency: None, opts: MentionOptions::default() }
    }

    pub fn with_shared_frecency(mut self, sf: SharedFrecency) -> Self {
        self.shared_frecency = Some(sf);
        self
    }

    pub fn with_options(mut self, opts: MentionOptions) -> Self {
        self.opts = opts;
        self
    }

    pub fn detect_trigger(input: &str, cursor: usize) -> Option<MentionTrigger> {
        detect_trigger(input, cursor)
    }

    pub fn search(&self, input: &str, cursor: usize) -> MentionResult<'a> {
        let trigger = match detect_trigger(input, cursor) {
            Some(t) if t.query.chars().count() >= self.opts.min_query_chars => t,
            Some(t) => return MentionResult { trigger: Some(t), candidates: vec![] },
            None => return MentionResult { trigger: None, candidates: vec![] },
        };

        let mut out: Vec<MentionCandidate<'a>> = Vec::new();

        // FILES — reuse the existing fuzzy_search pipeline.
        if self.opts.include_files {
            if let Ok(q) = FfsQuery::parse(&trigger.query) {
                let result = self.picker.fuzzy_search(&q, None, Default::default());
                for r in result.items {
                    let item: &FileItem = self.picker.get_files().get(r.index)?;
                    let cand = MentionCandidate {
                        kind: MentionKind::File,
                        display: r.path_str().to_owned(),
                        relative_path: r.path_str().to_owned(),
                        frecency_score: (item.access_frecency_score as i64)
                                       + (item.modification_frecency_score as i64),
                        size: item.size,
                        is_binary: item.is_binary(),
                        modified: item.modified,
                        match_indices: r.indices.clone(),
                        score: r.score,
                        file_item: Some(item),
                    };
                    out.push(cand);
                }
            }
        }

        // DIRECTORIES — reuse the existing fuzzy_search_directories pipeline.
        if self.opts.include_dirs {
            if let Ok(q) = FfsQuery::parse(&trigger.query) {
                let dresult = self.picker.fuzzy_search_directories(&q, Default::default());
                for r in dresult.items {
                    let dir: &DirItem = self.picker.get_dirs().get(r.index)?;
                    let cand = MentionCandidate {
                        kind: MentionKind::Directory,
                        display: r.path_str().to_owned(),
                        relative_path: r.path_str().to_owned(),
                        frecency_score: r.frecency_score,
                        size: 0,
                        is_binary: false,
                        modified: 0,
                        match_indices: r.indices.clone(),
                        score: r.score,
                        file_item: None,
                    };
                    out.push(cand);
                }
            }
        }

        // Cross-kind rank by frizbee's score (already comparable across kinds).
        out.sort_by(|a, b| b.score.cmp(&a.score));
        out.truncate(self.opts.max_candidates);

        MentionResult { trigger: Some(trigger), candidates: out }
    }
}
```

### 5.4 No changes elsewhere in Phase A

`FilePicker`, `FilePickerOptions`, `SharedFrecency`, `FrecencyTracker`, C ABI, MCP — all untouched.

---

## 6. Configuration & Wiring

**No env vars introduced in Phase A.** Frecency path is already controlled by the existing `frecency_db_path` parameter on `ffs_create_instance2()` (C ABI) and the corresponding `SharedFrecency::init()` call. Mention resolution inherits the picker's frecency state.

**No CLI subcommand in Phase A.** CLI ships in Phase C.

**No new C exports in Phase A.** C ABI ships in Phase C as `ffs_mention_search_json()`.

**No new MCP tools in Phase A.** MCP ships in Phase C.

**Host app integration (Phase A consumer example for jcode):**
```rust
// In jcode's at_picker.rs:
use ffs_search::mention::{MentionResolver, MentionOptions};

let resolver = MentionResolver::new(&picker)
    .with_shared_frecency(picker.shared_frecency().clone())
    .with_options(MentionOptions { max_candidates: 10, ..Default::default() });

let result = resolver.search(input, cursor);
if let Some(trigger) = result.trigger {
    // show popup with result.candidates
}
```

Zero changes to jcode's existing `AtPicker::search()` — the resolver is a new code path that can be used to *replace* the existing one when ready.

---

## 7. Repo References

| Aspect | Source | File | Link |
|--------|--------|------|------|
| Cursor-aware trigger detect | codex | `codex-rs/tui/src/bottom_pane/chat_composer.rs:2377-2430` | https://github.com/openai/codex/blob/main/codex-rs/tui/src/bottom_pane/chat_composer.rs#L2377 |
| Boundary check (whitespace + bracket set) | opencode | `packages/opencode/src/cli/cmd/prompt-display.ts:38` | https://github.com/anomalyco/opencode/blob/main/packages/opencode/src/cli/cmd/prompt-display.ts#L38 |
| Tiered resolution (exact → prefix → fuzzy) | oh-my-pi | `packages/coding-agent/src/utils/file-mentions.ts:120-160` | https://github.com/can1357/oh-my-pi/blob/main/packages/coding-agent/src/utils/file-mentions.ts#L120 |
| Quoted form for paths with spaces | claude-code | `src/utils/attachments.ts:2839` | https://github.com/claude-code-best/claude-code/blob/main/src/utils/attachments.ts#L2839 |
| 3 orthogonal @-kinds extracted in parallel | claude-code | `src/utils/attachments.ts:2839, 2874, 2884` | https://github.com/claude-code-best/claude-code/blob/main/src/utils/attachments.ts#L2839 |
| `parseSlashCommand` + coexist with `@` | claude-code | `src/utils/slashCommandParsing.ts:25` | https://github.com/claude-code-best/claude-code/blob/main/src/utils/slashCommandParsing.ts#L25 |
| Frecency boost multiplier `(1 + frecencyScore)` | opencode | `packages/opencode/src/cli/cmd/tui/component/prompt/autocomplete.tsx:600-615` | https://github.com/anomalyco/opencode/blob/main/packages/opencode/src/cli/cmd/tui/component/prompt/autocomplete.tsx#L600 |
| Auto-quote paths with whitespace | pi-agent-rust | `src/interactive/file_refs.rs:122` | https://github.com/Dicklesworthstone/pi_agent_rust/blob/main/src/interactive/file_refs.rs#L122 |
| Boundary detection (rejects mid-word @) | pi-agent-rust | `src/interactive/file_refs.rs:169` | https://github.com/Dicklesworthstone/pi_agent_rust/blob/main/src/interactive/file_refs.rs#L169 |
| Tab in dropdown = accept | pi-agent-rust | `src/interactive.rs:3185` | https://github.com/Dicklesworthstone/pi_agent_rust/blob/main/src/interactive.rs#L3185 |
| `parseAtInLine` reject email/URL/escape | codebuff | `cli/src/hooks/use-suggestion-engine.ts:106-144` | https://github.com/CodebuffAI/codebuff/blob/main/cli/src/hooks/use-suggestion-engine.ts#L106 |
| LLM-mediated dispatch (rejected) | codebuff | `agents/base2/base2.ts:144` | https://github.com/CodebuffAI/codebuff/blob/main/agents/base2/base2.ts#L144 |
| `ProtocolHandler` interface (Phase D inspiration) | oh-my-pi | `packages/coding-agent/src/internal-urls/types.ts:113-140` | https://github.com/can1357/oh-my-pi/blob/main/packages/coding-agent/src/internal-urls/types.ts#L113 |
| `fileSuggestion.command` external ranker (Phase D) | claude-code | `src/utils/settings/types.ts:304` | https://github.com/claude-code-best/claude-code/blob/main/src/utils/settings/types.ts#L304 |
| IDE bridge via `at_mentioned` MCP (Phase D) | opencode | `packages/opencode/src/cli/cmd/tui/context/editor.ts:76-99` | https://github.com/anomalyco/opencode/blob/main/packages/opencode/src/cli/cmd/tui/context/editor.ts#L76 |
| Custom ranker FFI hook (Phase D) | claude-code | `src/utils/suggestions/commandSuggestions.ts:301` | https://github.com/claude-code-best/claude-code/blob/main/src/utils/suggestions/commandSuggestions.ts#L301 |
| Token-budget discipline (Phase B) | claude-code | `src/constants/apiLimits.ts:83` | https://github.com/claude-code-best/claude-code/blob/main/src/constants/apiLimits.ts#L83 |
| Image auto-resize (Phase B) | oh-my-pi | `packages/coding-agent/src/utils/image-resize.ts` | https://github.com/can1357/oh-my-pi/blob/main/packages/coding-agent/src/utils/image-resize.ts |

---

## 8. Test Cases (Phase A)

### Happy path

```rust
#[test] fn trigger_at_start_of_input() { ... }
#[test] fn trigger_after_space() { ... }
#[test] fn trigger_after_paren() { ... }
#[test] fn trigger_after_brace() { ... }
#[test] fn trigger_at_quote() { ... }
#[test] fn quoted_path_with_spaces() { ... }
#[test] fn line_range_l10() { ... }
#[test] fn line_range_l10_dash_20() { ... }
#[test] fn line_range_l10_dash() { ... }
#[test] fn empty_query_returns_no_candidates() { ... }
#[test] fn empty_query_with_min_chars_zero_returns_top_recent() { ... }
#[test] fn short_query_gates_only_subsequence_tier() { ... }
#[test] fn mention_candidate_includes_frecency_score() { ... }
#[test] fn mention_candidate_includes_match_indices() { ... }
#[test] fn file_and_dir_results_ranked_together() { ... }
#[test] fn include_files_false_excludes_files() { ... }
#[test] fn include_dirs_false_excludes_dirs() { ... }
#[test] fn max_candidates_truncates_results() { ... }
```

### Edge cases

```rust
#[test] fn rejects_email_address() { ... }              // "user@example.com"
#[test] fn rejects_url() { ... }                       // "https://foo.com"
#[test] fn rejects_mid_word_at() { ... }                // "foo@bar"
#[test] fn rejects_escaped_at() { ... }                 // "\@foo"
#[test] fn rejects_in_string_with_close_quote() { ... } // "(see @\")"
#[test] fn rejects_npm_scope() { ... }                  // "@angular/core" — well actually, this IS valid path. Test confirms we accept.
#[test] fn unterminated_quote_treated_as_raw() { ... }  // `@"foo`
#[test] fn cursor_at_zero_returns_none() { ... }
#[test] fn cursor_past_input_len_clamps() { ... }
#[test] fn unicode_query_handled() { ... }              // "@résumé"
#[test] fn multibyte_char_boundary_safe() { ... }       // cursor in middle of 2-byte char
#[test] fn resolver_lifetime_tied_to_picker() { ... }   // compile-time check via NLL
#[test] fn no_frecency_still_works() { ... }            // resolver without SharedFrecency
#[test] fn no_picker_results_returns_empty() { ... }    // picker.scan() not yet called
```

### Integration

```rust
#[test] fn jcode_at_picker_can_replace_existing_with_resolver() { ... }
#[test] fn thread_safety_unchanged_from_picker() { ... } // FilePicker still !Sync; resolver borrows &FilePicker
```

**No tests in Phase A for:** payload resolution, line range injection into reads, binary policy, image handling, agent/tool/skill dispatch, IDE bridge, C ABI, MCP, CLI — all in later phases.

---

## 9. Benchmarks (Phase A only)

```rust
// benches/mention_bench.rs
use criterion::{criterion_group, criterion_main, Criterion};
use ffs_search::mention::{MentionResolver, MentionOptions};

fn bench_trigger_only(c: &mut Criterion) {
    c.bench_function("mention_trigger_only", |b| {
        b.iter(|| MentionResolver::detect_trigger("review @src/main", 16));
    });
}

fn bench_candidate_search_warm_10k(c: &mut Criterion) {
    let picker = build_picker_with_10k_files();
    let resolver = MentionResolver::new(&picker);
    c.bench_function("mention_candidate_search_warm_10k", |b| {
        b.iter(|| resolver.search("review @src/co", 14));
    });
}

fn bench_candidate_search_cold_40k(c: &mut Criterion) {
    let picker = build_picker_with_40k_files();
    let resolver = MentionResolver::new(&picker);
    c.bench_function("mention_candidate_search_cold_40k", |b| {
        b.iter(|| resolver.search("review @src/co", 14));
    });
}

criterion_group!(benches, bench_trigger_only, bench_candidate_search_warm_10k, bench_candidate_search_cold_40k);
criterion_main!(benches);
```

**Targets:**

| Metric | Baseline (v1 fuzzy_search w/ @ prefix) | Target (Phase A) | Source |
|--------|----------------------------------------|------------------|--------|
| Trigger detect only | n/a | **< 50µs** | (no I/O, pure parser) |
| Warm p99 per-keystroke, 10k files | ~120ms | **< 10ms** | reuses frizbee |
| Cold p99 per-keystroke, 40k files | ~250ms | **< 50ms** | reuses frizbee on full scan |
| Time-to-first-result, 40k cold repo | 1.18s | **< 100ms** (paired with latency plan) | |

---

## 10. Migration / Rollout — 4 phases

### Phase A — Core @file/@directory candidate search (this PR)

**Scope:**
- `crates/ffs-core/src/mention/{mod,trigger,resolver}.rs` (NEW)
- Public types: `MentionKind`, `MentionTrigger`, `MentionCandidate`, `MentionResult`, `MentionOptions`, `MentionResolver`
- Reuses `FilePicker::fuzzy_search()` + `fuzzy_search_directories()` + `FrecencyTracker`
- No changes to `FilePicker`, `FilePickerOptions`, `SharedFrecency`, C ABI, MCP, CLI

**Backwards compat:** 100% additive. Zero source-breaking change.

**Feature flag:** none needed. Mention is opt-in by importing the module. Existing ffs users see no change.

**Validation:** all 30+ unit tests + 5 integration tests pass. Benchmarks hit targets.

### Phase B — Resolve selected mentions (later, separate PR)

**Scope:**
- `ResolvedMention` struct (in ffs-engine or ffs-budget, not ffs-core)
- Line range support: `@path#L10-20` → `ResolvedMention { range: Some(10, 20) }`
- Binary detection: `is_binary` already on `FileItem` from Phase A; consumers filter
- `ffs-budget` integration: head-truncate, line-range truncation, hashline rendering
- Image detection by extension: `path.ends_with(".png|.jpg|.jpeg|.gif|.webp")` → `ResolvedMention::Image`
- Dedup-by-turn cache: `MentionResolver::search_cached(turn_id, candidates)` to avoid re-reading same path 5x
- Audit log: `ResolvedMention { audit: MentionAudit { phase, tokens } }` for debugging

**Backwards compat:** 100% additive. New types, new methods, new env vars.

**Host app impact:** jcode/Claude Code/Codex swap from "loop asking ffs to resolve each candidate" to "use ResolvedMention directly".

### Phase C — Surfaces (later, separate PRs)

**Scope:**
- CLI: `ffs mention-search --format json --input "review @src/foo"`
- MCP: `mention_search` tool returning JSON
- C ABI: `ffs_mention_search_json(handle, input, cursor, options_json) → FfsResult` (JSON first, no nested typed structs)
- After JSON ABI stabilizes (3-6 months): typed C structs `FfsMentionResult`, `FfsMentionCandidate`, etc. with proper `*_free` helpers

**Backwards compat:** 100% additive.

### Phase D — Extensibility (later, separate PRs)

**Scope:**
- `MentionProvider` trait registered via `MentionResolver::with_provider(Arc<dyn MentionProvider>)`
- Async API: `pub async fn search_async(&self, input, cursor) -> MentionResult` for host providers with I/O
- `MentionKind::External(&'static str)` extension
- Host-registered `agent` / `tool` / `skill` / `image` providers (jcode's `AtPicker` registers its own)
- MCP resource provider (opencode pattern)
- Reference alias provider (opencode `Reference.Service` pattern)
- IDE bridge via MCP `at_mentioned` notification
- External command ranker (claude-code `fileSuggestion.command` pattern)
- LSP-style `at-path-with-line-range` push from VS Code/JetBrains/Zed

**Backwards compat:** 100% additive. New trait, new method, new feature flags.

**fence:** Phase D evaluators run after Phase A's API is stable (3-6 months in production).

---

## 11. Known Limitations & Future Work

### Phase A limitations (intentional)

- [ ] **No payload resolution** — ffs does not read file contents, truncate, or render hashlines. That's `ffs-budget`'s job (Phase B).
- [ ] **No `External` provider registry** — only `File` and `Directory` work in Phase A. Host-registered kinds are Phase D.
- [ ] **No async / no parallel provider fan-out** — Phase A is sync single-thread over a `&FilePicker` borrow. Matches the existing `fuzzy_search()` API surface.
- [ ] **No mention codec / history round-trip** — that's a host concern (codex `mention_codec.rs` lives in codex, not ffs). Phase D might add a thin helper.
- [ ] **No CLI / C ABI / MCP** — surfaces ship in Phase C.

### All-phase future work

- [ ] **Streaming auto-read** — every reference repo loads the whole file then truncates. No progressive rendering. Defer until measured.
- [ ] **`@`-with-content-block anchors** (e.g. `@file.rs#Symbol` via tree-sitter) — would require ffs-symbol integration. No major repo has this.
- [ ] **Inline content preview in popup** — first-line preview, git-blame badge, last-modified. Future work.
- [ ] **No `@image` shortcut in Phase A** — image attachment is the host app's job in Phase A; ffs just identifies binary files via `is_binary()`.
- [ ] **Audit log for mention resolution** — codebuff's gap; would help debugging typo-silent failures (Phase D concern).

---

## 12. Success Criteria Checklist

- [ ] `MentionResolver::search()` returns ranked candidates in < 10ms p99 on 10k files
- [ ] `MentionResolver::search()` returns ranked candidates in < 50ms p99 on 40k files
- [ ] Trigger detection in < 50µs (pure parser, no I/O)
- [ ] All 30+ unit tests + 5 integration tests pass
- [ ] **No** modifications to `FilePicker`, `FilePickerOptions`, `SharedFrecency`, C ABI, MCP, CLI
- [ ] **No** new public dependencies in ffs-core
- [ ] `MentionOptions` is `Clone + Default` (FFI-safe, no `Box<dyn>`)
- [ ] Reuses `FilePicker::fuzzy_search()` and `fuzzy_search_directories()` directly (no parallel scorer)
- [ ] Reuses existing LMDB-backed `FrecencyTracker` via `SharedFrecency::read()` (no new `frecency.jsonl`)
- [ ] `MentionKind` has exactly 3 variants: `File`, `Directory`, `External(&'static str)`
- [ ] No `mention_options` field added to `FilePickerOptions`
- [ ] No `search_mentions()` method added to `FilePicker` (the resolver is the entry point)
- [ ] 5 HIGH-relevance patterns adopted from research: cursor-aware trigger (codex), boundary check (opencode), tiered resolution (oh-my-pi), quoted form (claude-code), 3 orthogonal @-kinds (claude-code, deferred to D)
- [ ] 1 HIGH-relevance pattern explicitly rejected: LLM-mediated dispatch (codebuff)
- [ ] 2 MEDIUM-relevance patterns considered: pi-agent-rust single-char router (deferred to D), oh-my-openagent backend resolver (not applicable)
- [ ] 2 LOW-relevance patterns documented as contrast: oh-my-claudecode hooks-only, oh-my-codex $-prefix
- [ ] No top-level API breakage (CLAUDE.md mandate) — verifiable by `make test` + `make lint` green

---

# Appendix A — Full Research Report (Phase 2)

[Full per-repo reports from 9 reference repos — see git history of `docs/MENTION_SYSTEM_PLAN.md` v1 for the embedded research. Summary table below.]

| Tier | Repo | Best contribution to ffs |
|------|------|--------------------------|
| HIGH | codex | cursor-aware token detection; mentions_v2 popup; structured `UserInput::Mention` |
| HIGH | opencode | `mentionTriggerIndex` (grapheme/display-width); fuzzysort + frecency; WebSocket editor bridge |
| HIGH | claude-code | 3 orthogonal @-kinds; Unicode typeahead; nucleo + Fuse.js; PDF/image token-cap; `fileSuggestion.command` FFI |
| HIGH | oh-my-pi | tiered resolution (exact → unique prefix → fuzzy subsequence); hashline rendering; layered autocomplete; `ProtocolHandler` interface |
| HIGH | codebuff | spawnable agents filter; 4-agent pipeline; image attachment orthogonal; rejected: LLM-mediated dispatch |
| MEDIUM | pi-agent-rust | file-only @; single-char router; auto-quote paths; `FileCache` TTL 30s; **defer async pattern** |
| MEDIUM | oh-my-openagent | backend @-file resolver; project-root containment; `/@plan` slash command |
| LOW | oh-my-claudecode | hooks-only; relies on host CLI's native @ resolution |
| LOW | oh-my-codex | `$`-prefix for skills; `parseMentionAllowedMentions` is Discord-only |

**Cross-repo patterns (proven by all 5 HIGH repos):**
- Cursor-anchored trigger detection with boundary check
- Tiered fuzzy: prefix > substring > subsequence
- Layered source mix in one ranked popup (file + agent + skill + MCP)
- Typed structured dispatch (`FilePart` / `AgentPart` / `ImageContent`)
- Frecency boost (1 + frecencyScore multiplier)
- Token-cost discipline (max files, head-truncate, PDF threshold)
- Image attachment is **always** a separate channel (paste/drop/slash), not `@`

**Unique divergences (worth knowing):**
- oh-my-pi uses `scheme://` for non-file kinds; we adopt `MentionKind::External(&'static str)` for host-registered kinds (Phase D)
- codebuff uses LLM-mediated dispatch; we reject (ffs is search, not agent runtime)
- claude-code allows custom external command as file ranker; Phase D candidate
- opencode has the only first-class persistent frecency (`frecency.jsonl`); we reuse ffs's existing LMDB-backed `FrecencyTracker` instead

**Common gaps (opportunities):**
- No token-cost preview in popup (could differentiate)
- No inline content preview in picker
- No mention dedup-by-turn (Phase B)
- No streaming auto-read
- No `@image` shortcut in picker (always paste/drop)
- No `@-with-content-block` (e.g. `@file.rs#Symbol`)
- No plugin slot for custom `@` providers (Phase D addresses)
- No audit log for resolution (Phase D)

# Appendix B — v1 → v2 Diff Summary

| Aspect | v1 (over-scoped) | v2 (review-fixed) |
|--------|------------------|-------------------|
| Phases | 3 (A core, B multi-kind, C surfaces) | **4** (A core, B resolve, C surfaces, D extensibility) |
| `FilePickerOptions::mention_options` | added field | **rejected** — would break exhaustive literals |
| Frecency | new `frecency.jsonl` | **reuse** `FrecencyTracker` (LMDB) |
| `Box<dyn MentionProvider>` in `MentionOptions` | yes | **rejected** — breaks `Clone`/FFI |
| Sync/async | parallel async fan-out in Phase A | **sync** Phase A, async **Phase D** |
| `fuzzy_min_chars` | gates all tiers | gates **subsequence only** |
| Mention kinds Phase A | File/Agent/Image/Tool/Skill | **File + Directory** + `External(&'static str)` |
| C ABI Phase A | `FfsMentionResult` typed struct | **deferred** to Phase C (JSON first) |
| Payload resolution | in ffs-core Phase A | **Phase B** in ffs-engine/ffs-budget |
| LOC estimate | ~2000 Rust | ~600-800 Rust (Phase A) |
| PR surface | 1 huge PR | 1 small Phase A PR + 3 follow-ups |
