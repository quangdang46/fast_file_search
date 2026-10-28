# HANDOFF

State after PR #131 (`refactor: dedupe navigation walker, fix ignored MCP
params, trim agent context`), merged into `main`.

Nothing below is a blocker. They are ordered by value, not by risk.

---

## 1. `CandidateFile::collect` reads every non-code file

**Where:** `crates/ffs-cli/src/commands/callers_bfs.rs`, `CandidateFile::collect`.

The navigation commands (`callers`, `refs`, `flow`, `impact`) read *and keep in
memory* every file the walker yields, including `.md`, `.txt`, `.json`. Non-code
entries carry `lang: None` and are still scanned for literal text.

This is **existing behaviour, deliberately preserved** — the pre-refactor CLI
reported hits inside prose files, and filtering to code-only would have silently
changed output. Do not "fix" it without an output-contract decision.

What to do if it shows up in a profile:

- Split the walk: collect paths first, ask the bloom cache per path, and only
  read survivors (the comment in `engine_tools.rs` already sketches this).
- Verify with the fixture in `.verification/` plus a non-code file containing
  the needle; `callers <name>` must still report that file.

**Not measured.** No profile has been taken on a repo where this matters.

---

## 2. Only `multi_grep` got a perf test — the rest of the MCP surface has none

`benches/bench_multi_grep.py` covers exactly one code path because that is the
one that was changed. The other embedded-MCP handlers (`grep_files`,
`collect_callers`, `collect_callees`, `render_simple_map`) have no benchmark, so
a future optimisation there has no baseline to regress against.

Cheapest next step: extend `bench_multi_grep.py` with a `--tool` flag so the
same fixture drives `ffs_grep` and `ffs_callers`, then record numbers in
`benches/BASELINE.md`.

---

## 3. No agent-task measurement exists

The token reductions (guide 3,375 → 1,586; MCP instructions ~1,012 → ~300) are
**input-size** measurements. Nothing in this repo shows an agent finishing a
task in fewer turns or with fewer tool calls.

If that claim is ever wanted, it needs an eval harness: a set of representative
tasks, an agent driving `ffs mcp`, and a recorded tool-call count per task
before/after. That harness does not exist here and should not be faked.

---

## 4. `benches/run_warm_mcp.py` measures `ffs_grep`, not `ffs_multi_grep`

Documented in `benches/BASELINE.md` as a caveat, but the script's own name is
misleading. Either rename it to `run_warm_mcp_grep.py` or add a `--tool` flag
(see item 2) so the file matches what `BASELINE.md` implies.

Low risk, purely cosmetic, but it caused a real misreading once.

---

## 5. Two MCP surfaces can still drift

`ffs-mcp` (standalone, 16 tools, params in snake_case→camelCase via `serde`) and
the embedded `ffs mcp` in `ffs-cli` (17 tools, hand-written JSON schemas) are
separate implementations. They now share the candidate walker and the outline
lookups, but the *handlers* are still duplicated.

`.verification/verify_api.py` catches schema drift (tool removed, property
lost, required set changed) but not **behavioural** drift — e.g. the embedded
`ffs_deps` takes `path` while the standalone takes `target`. That difference is
intentional today; nothing enforces it stays consistent.

Options, in order of cost:

1. Keep the checker, extend it to call each tool once with a fixed fixture and
   compare the output *shape* (not values).
2. Make the embedded server depend on `ffs-mcp` as a library. `ffs-mcp` is a
   binary crate today (`[[bin]]` only); this means adding a `[lib]` target and
   moving `server.rs`/`engine_tools.rs` behind it. Doable, but it changes the
   crate layout and should not be done without a reason to touch both servers.

---

## 6. `hubGuard` semantics differ subtly between CLI and MCP

Both now stop propagation when a name exceeds the guard, and both keep the hits.
But the CLI counts hits **before** truncating at `--limit` (it walks every
candidate), while the MCP path asks `find_call_sites` for `guard + 1` hits and
stops early. For any name with more than `guard` hits the *decision* is the
same; for names just at the boundary they agree too. The remaining difference is
only in how many hits get counted toward `transitive`, not in whether
propagation stops.

Verified equivalent on a 3-file chain, not proved for large graphs. If exact
parity matters, the fix is to have both call a shared `count_hits_up_to`.

---

## 7. Benchmarks are single-machine medians

`benches/BASELINE.md` reports medians of 5–7 samples on a loaded workstation.
The before/after **ratio** is the deliverable; the absolute milliseconds are
not comparable across machines. CI has `bench-check.yml` (Criterion, 5%
regression threshold) for the paths it covers — the MCP multi-grep path is not
in that gate.

---

## How to re-verify anything here

```bash
cargo build --workspace
cargo test --workspace          # 963 tests
cargo clippy --workspace --features zlob -- -D warnings
cargo fmt --all --check

# API compatibility (needs a base-commit binary to compare against)
python .verification/verify_api.py <before-binary> ./target/debug/ffs <fixture> "$PWD"

# multi-grep benchmark, builds its own fixture
python benches/bench_multi_grep.py --before /tmp/ffs-before --after ./target/release/ffs
```

Artifacts from the last run: `.verification/report.txt`,
`api_compat.json`, `json_equivalence.txt`.
