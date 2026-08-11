#!/usr/bin/env python3
"""Precise spawn benchmark: ffs vs ripgrep (rg) on real content search.

Measures wall-clock latency of spawning each binary (full process spawn +
walk + prefilter + search + collect), stdout discarded, so both tools pay
identical startup + IO. Reports median of N runs per needle via
time.perf_counter_ns (nanosecond precision, no shell-inflated timing).

Fairness:
  - rg  : rg -F -l <needle> <repo>   (-F literal, -l files-only, gitignore)
  - ffs : ffs grep <needle> --root <repo> -l
Both respect .gitignore; both output only paths; stdout to DEVNULL.

Usage:
  python scripts/bench_ffs_vs_rg.py [repo] [--runs N] [--warm-index] [--needle X]...
Env: FFS_BIN (default ./target/release/ffs.exe), RG_BIN (auto-detect ripgrep).
"""
import argparse
import json
import os
import statistics
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_FFS = os.path.join(HERE, "..", "target", "release", "ffs.exe")

# Candidate paths for the real ripgrep on Windows (a bare `rg` in git-bash may
# be a GNU grep shim, so resolve an explicit ripgrep .exe).
RG_CANDIDATES = [
    r"C:\Users\ADMIN\AppData\Local\Microsoft\WinGet\Packages\BurntSushi.ripgrep.MSVC*\ripgrep-*\rg.exe",
    r"C:\ProgramData\chocolatey\bin\rg.exe",
    r"C:\Program Files\ripgrep\rg.exe",
]


def resolve_rg():
    import glob
    for pat in RG_CANDIDATES:
        for cand in glob.glob(pat):
            try:
                out = subprocess.run(
                    [cand, "--version"], capture_output=True, timeout=10
                ).stdout.decode("utf-8", "replace")
                if "ripgrep" in out.lower():
                    return cand
            except Exception:
                continue
    # Fallback: PATH lookup for rg.exe
    for name in ("rg.exe", "rg"):
        import shutil
        p = shutil.which(name)
        if p:
            return p
    return None


def run_once(cmd):
    t0 = time.perf_counter_ns()
    try:
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                       timeout=120)
    except Exception:
        pass
    t1 = time.perf_counter_ns()
    return (t1 - t0) / 1e6  # ms


def main():
    ap = argparse.ArgumentParser(description="ffs vs rg spawn benchmark")
    ap.add_argument("repo", nargs="?", default=".")
    ap.add_argument("--runs", type=int, default=12)
    ap.add_argument("--warm-index", action="store_true")
    ap.add_argument("--needle", action="append", default=None,
                    help="extra needle (repeatable)")
    ap.add_argument("--json", action="store_true", help="emit machine-readable results")
    args = ap.parse_args()

    repo = os.path.abspath(args.repo)
    ffs = os.environ.get("FFS_BIN", DEFAULT_FFS)
    ffs = os.path.abspath(ffs)
    rg = os.environ.get("RG_BIN") or resolve_rg()

    if not os.path.exists(ffs):
        print(f"ERROR: ffs not found at {ffs}", file=sys.stderr)
        sys.exit(2)
    if not rg:
        print("ERROR: ripgrep not found (set RG_BIN)", file=sys.stderr)
        sys.exit(2)

    ffs_ver = subprocess.run([ffs, "--version"], capture_output=True,
                             text=True).stdout.strip().splitlines()[0]
    rg_ver = subprocess.run([rg, "--version"], capture_output=True,
                            text=True).stdout.strip().splitlines()[0]

    if args.warm_index:
        subprocess.run([ffs, "index", "--root", repo], stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL)

    needles = ["FilePicker", "grep_search", "TODO", "HashMap",
               "match_byte_offsets", "pub fn"]
    if args.needle:
        needles = args.needle

    rows = []
    for n in needles:
        rg_cmd = [rg, "-F", "-l", n, repo]
        ffs_cmd = [ffs, "grep", n, "--root", repo, "-l"]
        # Interleave A/B/A/B per iteration so machine-load drift affects both
        # tools equally instead of biasing one side (running all of rg then all
        # of ffs lets a load spike land on one side and skew the ratio).
        rg_times, ffs_times = [], []
        run_once(rg_cmd)  # warmup
        run_once(ffs_cmd)
        for _ in range(args.runs):
            rg_times.append(run_once(rg_cmd))
            ffs_times.append(run_once(ffs_cmd))
        rg_ms = statistics.median(rg_times)
        ffs_ms = statistics.median(ffs_times)
        ratio = ffs_ms / rg_ms if rg_ms > 0 else float("inf")
        winner = "ffs" if ffs_ms <= rg_ms else "rg"
        rows.append((n, rg_ms, ffs_ms, ratio, winner))

    if args.json:
        print(json.dumps({
            "repo": repo, "ffs": ffs_ver, "rg": rg_ver, "runs": args.runs,
            "results": [{"needle": n, "rg_ms": round(r, 3), "ffs_ms": round(f, 3),
                         "ratio": round(x, 3), "winner": w}
                        for (n, r, f, x, w) in rows],
        }, indent=2))
        return

    print("=== ffs vs rg spawn benchmark ===")
    print(f"repo:  {repo}")
    print(f"ffs:   {ffs} ({ffs_ver})")
    print(f"rg:    {rg} ({rg_ver})")
    print(f"runs:  {args.runs}  warm-index: {args.warm_index}")
    print()
    print(f"{'needle':<22} {'rg (ms)':>10} {'ffs (ms)':>10} {'ffs/rg':>8} {'winner':>7}")
    print("-" * 66)
    for n, r, f, x, w in rows:
        print(f"{n:<22} {r:10.1f} {f:10.1f} {x:8.2f} {w:>7}")
    print()
    wins = sum(1 for (_, _, _, _, w) in rows if w == "ffs")
    losses = sum(1 for (_, _, _, _, w) in rows if w == "rg")
    print(f"ffs wins {wins}, rg wins {losses}  (ffs/rg < 1.0 means ffs faster; goal ffs/rg <= 1.0)")


if __name__ == "__main__":
    main()
