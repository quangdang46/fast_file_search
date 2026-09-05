#!/usr/bin/env bash
#
# Local benchmark regression check.
#
# Usage:
#   ./scripts/bench-check.sh [--threshold N]
#
# Runs Criterion benchmarks and compares against the last saved baseline.
# Exits with code 1 if any benchmark regresses by more than the threshold.
#
# First run: saves results as baseline.
# Subsequent runs: compares against baseline, reports regressions.
set -euo pipefail

THRESHOLD="${1:-10}"
if [ "$THRESHOLD" = "--threshold" ]; then
  THRESHOLD="${2:-10}"
fi

echo "=== Benchmark regression check (threshold: >${THRESHOLD}%) ==="
echo ""

# Run benchmarks (Criterion auto-compares if baseline exists)
cargo bench --features zlob \
  -p ffs-search \
  -p ffs-query-parser \
  -p ffs-symbol \
  -p ffs-budget \
  -p ffs-engine 2>&1 | tee /tmp/bench-output.txt

echo ""
echo "=== Checking for regressions ==="

REGRESSION_THRESHOLD="${THRESHOLD}" python3 - <<'PYEOF'
import json, glob, os, sys

threshold = int(os.environ.get("REGRESSION_THRESHOLD", "10"))

# Benchmarks known to be noisy (sensitive to system load, memory pressure).
# These are excluded from regression detection.
SKIP_PATTERNS = ["filter_apply", "smart_truncate", "fuzzy_score", "mention_candidate"]

regressions = []
improvements = []
no_change = []
skipped = []

for change_file in glob.glob("target/criterion/**/change/estimates.json", recursive=True):
    bench_path = change_file.replace("/change/estimates.json", "")
    bench_name = bench_path.replace("target/criterion/", "")

    # Skip known-noisy benchmarks
    if any(pat in bench_name for pat in SKIP_PATTERNS):
        skipped.append(bench_name)
        continue

    with open(change_file) as f:
        data = json.load(f)

    # mean.point_estimate is the ratio change (positive = slower)
    pct_change = data.get("mean", {}).get("point_estimate", 0) * 100

    if pct_change > threshold:
        regressions.append((bench_name, pct_change))
    elif pct_change < -threshold:
        improvements.append((bench_name, pct_change))
    else:
        no_change.append((bench_name, pct_change))

if regressions:
    print(f"\n❌ REGRESSIONS (threshold: >{threshold}%):")
    for name, pct in sorted(regressions, key=lambda x: -x[1]):
        print(f"  {name}: +{pct:.1f}%")
    print()

if improvements:
    print(f"\n✅ IMPROVEMENTS:")
    for name, pct in sorted(improvements, key=lambda x: x[1]):
        print(f"  {name}: {pct:.1f}%")
    print()

print(f"  No change (within ±{threshold}%): {len(no_change)} benchmarks")
print(f"  Improvements: {len(improvements)} benchmarks")
print(f"  Regressions: {len(regressions)} benchmarks")
print(f"  Skipped (noisy): {len(skipped)} benchmarks")

if regressions:
    print(f"\n💥 FAILED: {len(regressions)} benchmark(s) regressed by more than {threshold}%")
    sys.exit(1)
else:
    print(f"\n✅ PASSED: no regressions above {threshold}% threshold")
PYEOF
