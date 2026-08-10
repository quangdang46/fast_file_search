#!/usr/bin/env bash
#
# Spawn benchmark: ffs vs ripgrep (rg) on real content search.
#
# Usage:
#   ./scripts/bench_ffs_vs_rg.sh [repo] [--warm-index] [--runs N]
#
# Spawns each binary (full process spawn + walk + prefilter + search), stdout
# to /dev/null, so both pay identical startup+IO. No hyperfine dependency —
# measures wall-clock with a timing loop and reports the median.
#
# Fairness:
#   - rg  : rg -F -l <needle> <repo>   (-F literal, -l files-only, gitignore)
#   - ffs : ffs grep <needle> --root <repo> -l
# Both respect .gitignore; both output only paths; both to /dev/null.
set -euo pipefail

REPO="${1:-$PWD}"
WARM_INDEX=0
RUNS=8
FFS_BIN="${FFS_BIN:-$PWD/target/release/ffs.exe}"

# Resolve the real ripgrep. On git-bash a bare `rg` may be a GNU grep shim.
RG_BIN="${RG_BIN:-}"
if [ -z "$RG_BIN" ]; then
  for cand in \
    /c/Users/ADMIN/AppData/Local/Microsoft/WinGet/Packages/BurntSushi.ripgrep.MSVC_*/ripgrep-*/rg.exe \
    /c/ProgramData/chocolatey/bin/rg.exe \
    /usr/bin/rg.exe
  do
    if [ -x "$cand" ] && "$cand" --version 2>/dev/null | grep -qi ripgrep; then
      RG_BIN="$cand"; break
    fi
  done
fi
[ -z "$RG_BIN" ] && RG_BIN="$(command -v rg.exe 2>/dev/null || command -v rg || true)"

while [ $# -gt 0 ]; do
  case "$1" in
    --warm-index) WARM_INDEX=1; shift ;;
    --runs) RUNS="$2"; shift 2 ;;
    *) shift ;;
  esac
done

if [ ! -x "$FFS_BIN" ]; then
  echo "ERROR: ffs binary not found at $FFS_BIN (build: cargo build --release -p ffs-cli --no-default-features)" >&2
  exit 2
fi
if [ -z "$RG_BIN" ] || ! "$RG_BIN" --version 2>/dev/null | grep -qi ripgrep; then
  echo "ERROR: ripgrep (rg.exe) not found. Set RG_BIN to its path." >&2
  exit 2
fi

echo "=== ffs vs rg spawn benchmark ==="
echo "repo:  $REPO"
echo "ffs:   $FFS_BIN ($("$FFS_BIN" --version | head -1))"
echo "rg:    $RG_BIN ($("$RG_BIN" --version | head -1))"
echo "runs:  $RUNS  warm-index: $WARM_INDEX"

if [ "$WARM_INDEX" = "1" ]; then
  echo "--- building warm .ffs index for $REPO ---"
  "$FFS_BIN" index --root "$REPO" >/dev/null 2>&1 && echo "  index OK" || echo "  index skipped/failed (cold cache)"
fi

NEEDLES=(
  "FilePicker"
  "grep_search"
  "TODO"
  "HashMap"
  "match_byte_offsets"
  "pub fn"
)

# med: run a command N times, print median milliseconds (spawn cost included).
med() {
  local cmd="$*"
  local times=()
  # warmup once
  eval "$cmd" >/dev/null 2>&1 || true
  for _ in $(seq "$RUNS"); do
    local t0 t1
    t0=$(date +%s%N)
    eval "$cmd" >/dev/null 2>&1 || true
    t1=$(date +%s%N)
    times+=("$(( (t1 - t0) / 1000000 ))")
  done
  # median
  local sorted
  sorted=$(printf '%s\n' "${times[@]}" | sort -n)
  local mid
  mid=$(echo "$sorted" | sed -n "$(( (RUNS + 1) / 2 ))p")
  echo "$mid"
}

printf "%-22s %10s %10s %8s %7s\n" "needle" "rg (ms)" "ffs (ms)" "ffs/rg" "winner"
printf "%s\n" "--------------------------------------------------------------------------"

for needle in "${NEEDLES[@]}"; do
  rg_cmd="\"$RG_BIN\" -F -l '$needle' \"$REPO\""
  ffs_cmd="\"$FFS_BIN\" grep '$needle' --root \"$REPO\" -l"

  rg_ms=$(med "$rg_cmd")
  ffs_ms=$(med "$ffs_cmd")

  if [ "$rg_ms" -gt 0 ]; then
    ratio=$(python -c "print(f'{$ffs_ms/$rg_ms:.2f}')")
    winner="ffs"
    [ "$ffs_ms" -gt "$rg_ms" ] && winner="rg"
    printf "%-22s %10s %10s %8s %7s\n" "$needle" "$rg_ms" "$ffs_ms" "$ratio" "$winner"
  else
    printf "%-22s %10s %10s %8s %7s\n" "$needle" "$rg_ms" "$ffs_ms" "-" "-"
  fi
done

echo ""
echo "ffs/rg < 1.0 means ffs is faster. Goal: ffs/rg <= 1.0 on every needle."
