# Benchmark baseline

Recorded 2026-10-29 against base commit `d34576d4`.

## How to reproduce

```bash
# 1. Build the two binaries you are comparing.
cargo build --release -p ffs-cli
cp target/release/ffs /tmp/ffs-after
git stash && cargo build --release -p ffs-cli && cp target/release/ffs /tmp/ffs-before && git stash pop

# 2. Run the benchmark. It builds its own fixture, so no external data needed.
python benches/bench_multi_grep.py \
    --before /tmp/ffs-before \
    --after  /tmp/ffs-after \
    --reps 7
```

The script prints raw per-sample timings plus medians and the delta, so the
numbers below can be re-derived rather than trusted.

### Fixture

Created in a temp dir by `build_fixture()` and deleted on exit:

| Property | Value |
|---|---|
| Small files with **no** match | 2,000 |
| One large file | 20,000 lines |
| Matches inside the large file | 2,000 (every 10th line) |
| Query | `NEEDLE` via `ffs_multi_grep` |
| `maxResults` | 2,000 |

The small files must not match: at the tool's default limit the result set
fills before the large file is read, and neither version exercises the code
being measured. That is why the default here is 2,000.

### Measurement

Warm `ffs mcp` process over JSON-RPC; one warmup request (pays the scan), then
N timed `tools/call` round-trips on the same process. Reported statistic is the
median of N samples.

## Result

| | median | mean | min |
|---|---|---|---|
| before | 240.44 ms | 254.85 ms | 230.17 ms |
| after | 26.97 ms | 28.92 ms | 23.95 ms |

**Δ median −88.78%.**

Raw samples (ms), after:
`30.39, 38.58, 29.52, 23.95, 26.71, 26.29, 26.97`

Raw samples (ms), before:
`243.30, 325.96, 235.16, 240.44, 269.71, 230.17, 239.18`

### Why it changed

The old line-number mapping counted every newline in the file prefix for each
match — O(matches × prefix). The new code advances a cursor across
Aho-Corasick's in-order matches — O(file) total, and it does no work for files
with no matches.

## Other paths

Top-level CLI `grep` (the tree walk and matcher were not modified):

| | median |
|---|---|
| before | 18.12 ms |
| after | 18.09 ms |

Δ −0.17% over 120 samples (20 queries × 6 reps) on this workspace.
That path is unchanged; this only confirms the surrounding edits were inert.

`ffs callers FilePicker` on this workspace: 1,386 ms → 351 ms (−74.7%).
This is the parallel-walk reuse in `CandidateFile::collect`; it had previously
re-read every file including non-code ones.

## Caveats

- Single machine, medians of 5–7 samples on a loaded workstation. The
  before/after **ratio** is meaningful; absolute milliseconds are not.
- `run_throughput.py` and `run_warm_mcp.py` measure `ffs_grep`, not
  `ffs_multi_grep`. Use `bench_multi_grep.py` for the multi-grep path.
- No agent-task measurement exists: nothing here shows an agent solving a task
  in fewer turns. Token counts in `guide` / MCP instructions are input-size
  measurements only.
