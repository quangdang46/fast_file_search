# Plan: Multi-scope Find + AhoCorasick Batch Search + Session Dedup

**Constraint**: No changes to `crates/fff-core/`.

## 1. Multi-scope Find

### Summary
Allow multiple `--scope` directories with dedup, overlap detection, and per-scope result headers.

### Modified Files

#### `crates/fff-cli/src/cli.rs`
- Change `root` from single `PathBuf` to accept `--scope` multiple times:
```rust
#[arg(long, global = true, num_args = 1..)]
pub scope: Vec<PathBuf>,
```
- Keep `root` as the primary default (cwd). `scope` overrides when provided.

#### `crates/fff-engine/src/dispatch.rs`
- `Engine::index()` and `Engine::dispatch()` already accept `&Path`. For multi-scope, the CLI layer runs the engine per-scope and merges results.
- No engine changes needed — the merge happens at the CLI layer.

#### `crates/fff-cli/src/commands/find.rs` (or dispatch.rs)
- Add multi-scope runner:
```rust
fn run_multi_scope(args: &Args, scopes: &[PathBuf], format: OutputFormat) -> Result<()> {
    let mut merged: Vec<SearchResult> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for scope in scopes {
        let engine = Engine::default();
        engine.index(scope);
        let results = engine.dispatch(&args.query);
        for r in results {
            let key = format!("{}:{}", r.path, r.line);
            if seen.insert(key) {
                merged.push(r);
            }
        }
    }
    // render with per-scope headers
}
```

### Implementation Steps
1. Add `--scope` multi-value flag to `Cli` struct. Complexity: **Low**.
2. Add multi-scope merge logic in the find/dispatch commands. Complexity: **Medium** (~80 lines).
3. Per-scope headers in text output. Complexity: **Low** (~20 lines).

**Estimated: ~100 lines**

---

## 2. AhoCorasick Batch Symbol Search

### Summary
For multi-symbol queries ("A,B,C"), use a single file walk with AhoCorasick instead of N separate lookups.

### Dependencies
- Add `aho-corasick` to `crates/fff-symbol/Cargo.toml` (check if already present).

### New Files

#### `crates/fff-symbol/src/batch.rs`
```rust
/// Batch multi-symbol search using AhoCorasick for byte-level any-of gating.
pub struct BatchSymbolSearch {
    patterns: Vec<String>,
    ac: aho_corasick::AhoCorasick,
}

pub struct BatchResult {
    pub symbol: String,
    pub locations: Vec<SymbolLocation>,
}

impl BatchSymbolSearch {
    pub fn new(symbols: &[String]) -> Self;
    
    /// Single file walk: for each code file, check if any pattern appears
    /// using AhoCorasick, then confirm with symbol_index lookup.
    pub fn search(&self, index: &SymbolIndex) -> Vec<BatchResult>;
}
```

Logic (ported from srcwalk's `search/symbol/batch.rs`):
1. Build `AhoCorasick` from symbol names.
2. For each code file in the symbol index:
   - Read file bytes.
   - `ac.find_overlapping(bytes)` to get candidate matches.
   - Confirm each candidate against symbol_index.
3. Results bucketed per-symbol.

### Modified Files

#### `crates/fff-symbol/src/lib.rs`
- Add `pub mod batch;`

#### `crates/fff-engine/src/dispatch.rs`
- When query is classified as multi-symbol (comma-separated), route to `BatchSymbolSearch` instead of looping `lookup_exact`.

#### `crates/fff-cli/src/commands/dispatch.rs`
- Parse comma-separated queries and route to batch mode.

### Implementation Steps
1. Add `aho-corasick` dependency. Complexity: **Low** (1 line in Cargo.toml).
2. Create `batch.rs` with AhoCorasick gating. Complexity: **Medium** (~100 lines).
3. Wire into dispatch for multi-symbol queries. Complexity: **Low** (~30 lines).

**Estimated: ~130 lines**

---

## 3. Session Dedup Tracker

### Summary
Track which `path:line` locations have been expanded to avoid re-expanding the same source body during cascade fallbacks.

### New File

#### `crates/fff-cli/src/commands/session.rs`
```rust
pub struct Session {
    expanded: std::cell::RefCell<HashSet<String>>, // "path:line" -> already inlined
}

impl Session {
    pub fn new() -> Self;
    pub fn is_expanded(&self, path: &Path, line: u32) -> bool;
    pub fn record_expand(&self, path: &Path, line: u32);
}
```

Uses `RefCell` instead of `Mutex` since each CLI invocation is single-threaded (unlike srcwalk which uses `Mutex` for potential parallel use).

### Modified Files

#### `crates/fff-cli/src/commands/callers_bfs.rs`
- Create `Session` at top of `run_bfs()`.
- Before expanding a caller hit's source context, check `session.is_expanded()`.
- After expanding, call `session.record_expand()`.

#### `crates/fff-cli/src/commands/callees_bfs.rs`
- Same pattern as callers_bfs.

#### `crates/fff-cli/src/commands/flow.rs`
- Pass session through to callees and callers invocations so dedup works across the combined flow.

### Implementation Steps
1. Create `session.rs`. Complexity: **Very Low** (~30 lines, nearly identical to srcwalk).
2. Wire into callers_bfs. Complexity: **Low** (~10 lines).
3. Wire into callees_bfs. Complexity: **Low** (~10 lines).
4. Wire into flow. Complexity: **Low** (~15 lines).

**Estimated: ~65 lines**

---

## Total Estimated: ~295 lines new code

## Implementation Order
1. Session dedup (smallest, self-contained)
2. Multi-scope find
3. AhoCorasick batch search
