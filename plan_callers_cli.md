# Plan: Callers Improvements + Section Disambiguation + Shell Completions

**Constraint**: No changes to `crates/fff-core/`.

## 1. Callers: --count-by Aggregation

### Summary
Add `--count-by <field>` flag that groups caller results by a field and shows counts instead of individual hits.

### Modified Files

#### `crates/fff-cli/src/commands/callers.rs`
- Add flag:
```rust
#[arg(long, value_enum)]
pub count_by: Option<CountByField>,
```
- Add enum:
```rust
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CountByField {
    File,
    Symbol,
    Package,
}
```
- After collecting hits, aggregate:
  - `File`: group by `hit.path`, show `path (N callers)`
  - `Symbol`: group by `hit.enclosing`, show `symbol (N call sites)`
  - `Package`: group by `package_root(hit.path, root)`, show `pkg (N callers)`
- When `--count-by` is set, output aggregated table instead of individual hits.

### Implementation Steps
1. Add `CountByField` enum and `--count-by` flag. Complexity: **Low**.
2. Add aggregation logic (HashMap grouping + sort by count desc). Complexity: **Low** (~40 lines).
3. Add text rendering for aggregated output. Complexity: **Low** (~20 lines).

**Estimated: ~60 lines**

---

## 2. Callers: --skip-hubs Explicit User Override

### Summary
Let users explicitly name hub symbols to skip during BFS: `--skip-hubs "println,eprintln,fmt"`.

### Modified Files

#### `crates/fff-cli/src/commands/callers.rs`
- Add flag:
```rust
#[arg(long, default_value = "")]
pub skip_hubs: String,
```

#### `crates/fff-cli/src/commands/callers_bfs.rs`
- Parse `skip_hubs` CSV into `HashSet<String>` at start of `run_bfs()`.
- In the frontier loop, before processing each name, check against user hubs:
```rust
if user_hubs.contains(name) && depth > 1 {
    stats.hubs_skipped.push(name.clone());
    continue;
}
```
- Root symbol (depth 1) is always explored even if in skip list (matches srcwalk behavior).
- Add `hubs_skipped: Vec<String>` to `BfsTelemetry`.

### Implementation Steps
1. Add flag to `callers.rs`. Complexity: **Very Low** (~3 lines).
2. Parse CSV and filter in `callers_bfs.rs`. Complexity: **Low** (~15 lines).
3. Add skipped hubs to telemetry output. Complexity: **Low** (~10 lines).

**Estimated: ~28 lines**

---

## 3. Suspicious-Hop Detection (Enhanced)

### Summary
Scry already has `SuspiciousHop` detection in `callers_bfs.rs` based on definition-path roots. Enhance it to also check filesystem directory proximity like srcwalk.

### Current State
Scry's current detection (line 197-203 of callers_bfs.rs): flags when a symbol has definitions in >= 2 different package roots. This is name-based.

### Enhancement
Add srcwalk's directory-proximity check: after BFS completes, for each hop >= 2, check what fraction of edges share a parent directory with the previous hop's edges. Flag when < 20% are related (cross-package collision).

### Modified Files

#### `crates/fff-cli/src/commands/callers_bfs.rs`
- Add new function:
```rust
fn compute_suspicious_hops(hits: &[CallerHit], root: &Path) -> Vec<SuspicionInfo> {
    // Group hits by depth.
    // For each depth >= 2 with >= 50 hits:
    //   Collect parent dirs of previous hop's hits.
    //   Count how many current-hop hits share a parent dir.
    //   If related < 20%, flag as suspicious.
}
```
- Port srcwalk's logic (lines 77-114 of srcwalk's bfs.rs) — pure function, ~35 lines.
- Call after BFS loop, merge into `BfsTelemetry.suspicious_hops`.

### Implementation Steps
1. Port `compute_suspicious_hops()` from srcwalk. Complexity: **Low** (~35 lines, pure function).
2. Wire into `run_bfs()` after the main loop. Complexity: **Very Low** (~5 lines).

**Estimated: ~40 lines**

---

## 4. Section Disambiguation

### Summary
When a bare filename matches multiple files, use heuristics to pick the "primary" one: prefer non-test, non-vendor, non-node_modules files closest to root.

### Current State
Scry's `read.rs` has `maybe_resolve_bare()` that finds files by basename. When >1 match, it returns `Ambiguous` with sorted candidates. No automatic primary-pick heuristic.

### Modified Files

#### `crates/fff-cli/src/commands/read.rs`
- Add a primary-pick function:
```rust
fn pick_primary(candidates: &[PathBuf], root: &Path) -> Option<usize> {
    // Filter out non-production directories:
    const NON_PROD: &[&str] = &["test", "tests", "__tests__", "spec", "specs",
        "vendor", "node_modules", "dist", "build", "target", ".git"];
    
    // Score each candidate:
    // 1. Not in NON_PROD dir segment → +100
    // 2. Path depth from root → shorter is better
    // 3. Prefer src/ over other top-level dirs
    
    // If a clear winner exists (score gap > threshold), return it.
    // Otherwise, return None (ambiguous).
}
```
- In `maybe_resolve_bare()`: when `Ambiguous`, try `pick_primary()`. If a clear winner, return `Found` instead.

### Implementation Steps
1. Add `pick_primary()` function. Complexity: **Low** (~40 lines).
2. Wire into `maybe_resolve_bare()`. Complexity: **Very Low** (~5 lines).

**Estimated: ~45 lines**

---

## 5. Shell Completions

### Summary
Add `--completions <shell>` flag to generate shell completion scripts via `clap_complete`.

### Dependencies
- Add `clap_complete` to `crates/fff-cli/Cargo.toml` (check if already present).

### Modified Files

#### `crates/fff-cli/src/cli.rs`
- Add `--completions` flag:
```rust
#[arg(long, value_enum, global = true)]
pub completions: Option<clap_complete::Shell>,
```

#### `crates/fff-cli/src/main.rs`
- Before dispatching to commands:
```rust
if let Some(shell) = cli.completions {
    clap_complete::generate(shell, &mut Cli::command(), "scry", &mut std::io::stdout());
    return Ok(());
}
```

### Implementation Steps
1. Add `clap_complete` dependency. Complexity: **Very Low** (1 line in Cargo.toml).
2. Add flag and generation code. Complexity: **Very Low** (~8 lines).

**Estimated: ~9 lines**

---

## Total Estimated: ~182 lines new code

## Implementation Order
1. Shell completions (smallest, easiest)
2. --skip-hubs
3. --count-by aggregation
4. Section disambiguation
5. Enhanced suspicious-hop detection
