#!/usr/bin/env python3
"""Verify ffs grep returns the same results as rg grep.

Usage:
    python benches/verify_correctness.py <repo> [--needles-file FILE] [--sample N]

Compares output of `ffs grep <needle> --root <repo> -l` vs `rg -F -l <needle> <repo>`
for each needle. Reports mismatches.

Exit code 0 if all needles match, 1 if any mismatch found.
"""
import argparse
import os
import subprocess
import sys
import random


def get_needles_from_file(path: str) -> list[str]:
    """Read needles from a file (one per line)."""
    with open(path) as f:
        return [line.strip() for line in f if line.strip() and not line.startswith("#")]


def sample_needles_from_repo(repo: str, count: int = 30) -> list[str]:
    """Sample needles from repo vocabulary at different frequency tiers."""
    # Get all words from Rust/Python/JS files
    try:
        result = subprocess.run(
            ["rg", "-o", "--no-filename", r"\b\w{4,15}\b", repo],
            capture_output=True, text=True, timeout=30
        )
        words = result.stdout.strip().split("\n")
    except Exception:
        return []

    # Count frequency
    from collections import Counter
    freq = Counter(words)

    # Stratify: rare (1-3 occurrences), mid (4-10), common (11+)
    rare = [w for w, c in freq.items() if 1 <= c <= 3]
    mid = [w for w, c in freq.items() if 4 <= c <= 10]
    common = [w for w, c in freq.items() if c > 10]

    sampled = []
    for tier_name, tier_words in [("rare", rare), ("mid", mid), ("common", common)]:
        n = min(count // 3 + 1, len(tier_words))
        if n > 0:
            sampled.extend(random.sample(tier_words, n))

    random.shuffle(sampled)
    return sampled[:count]


def compare_results(needle: str, repo: str, ffs_bin: str) -> dict:
    """Run ffs and rg on the same needle, compare results."""
    # ffs grep
    try:
        ffs_result = subprocess.run(
            [ffs_bin, "grep", needle, "--root", repo, "-l"],
            capture_output=True, text=True, timeout=30
        )
        ffs_files = set(
            l.strip() for l in ffs_result.stdout.strip().split("\n")
            if l.strip() and not l.startswith("[")
        )
    except Exception as e:
        return {"needle": needle, "error": f"ffs failed: {e}", "match": False}

    # rg grep — match ffs smart-case behavior:
    # ffs is case-insensitive when needle has no uppercase letters.
    has_uppercase = any(c.isupper() for c in needle)
    rg_flags = ["-F", "-l"]
    if not has_uppercase:
        rg_flags.append("-i")  # case-insensitive, like ffs smart-case
    try:
        rg_result = subprocess.run(
            ["rg"] + rg_flags + [needle, repo],
            capture_output=True, text=True, timeout=30
        )
        rg_files = set(l.strip() for l in rg_result.stdout.strip().split("\n") if l.strip())
    except Exception as e:
        return {"needle": needle, "error": f"rg failed: {e}", "match": False}

    # Compare (normalize paths relative to repo)
    def normalize(path_set):
        normalized = set()
        for p in path_set:
            if p.startswith(repo):
                p = p[len(repo):].lstrip(os.sep)
            normalized.add(p)
        return normalized

    ffs_norm = normalize(ffs_files)
    rg_norm = normalize(rg_files)

    only_ffs = ffs_norm - rg_norm
    only_rg = rg_norm - ffs_norm

    return {
        "needle": needle,
        "ffs_count": len(ffs_files),
        "rg_count": len(rg_files),
        "match": ffs_norm == rg_norm,
        "only_ffs": sorted(only_ffs)[:5],
        "only_rg": sorted(only_rg)[:5],
    }


def main():
    parser = argparse.ArgumentParser(description="Verify ffs vs rg correctness")
    parser.add_argument("repo", help="Repository path")
    parser.add_argument("--needles-file", help="File with needles (one per line)")
    parser.add_argument("--sample", type=int, default=30, help="Number of needles to sample")
    parser.add_argument("--ffs-bin", default="./target/release/ffs", help="Path to ffs binary")
    parser.add_argument("--seed", type=int, default=42, help="Random seed for reproducibility")
    args = parser.parse_args()

    random.seed(args.seed)
    repo = os.path.abspath(args.repo)

    # Get needles
    if args.needles_file:
        needles = get_needles_from_file(args.needles_file)
    else:
        print(f"Sampling {args.sample} needles from {repo}...")
        needles = sample_needles_from_repo(repo, args.sample)

    if not needles:
        print("ERROR: No needles found")
        sys.exit(1)

    print(f"Testing {len(needles)} needles on {repo}")
    print()

    # Verify each needle
    results = []
    mismatches = 0

    for needle in needles:
        r = compare_results(needle, repo, args.ffs_bin)
        results.append(r)

        if r.get("error"):
            print(f"  ❌ {needle}: {r['error']}")
            mismatches += 1
        elif not r["match"]:
            print(f"  ❌ {needle}: MISMATCH (ffs={r['ffs_count']}, rg={r['rg_count']})")
            if r["only_ffs"]:
                print(f"     only in ffs: {r['only_ffs']}")
            if r["only_rg"]:
                print(f"     only in rg: {r['only_rg']}")
            mismatches += 1
        else:
            print(f"  ✅ {needle}: match ({r['ffs_count']} files)")

    # Summary
    print()
    print(f"=== Summary ===")
    print(f"  Needles tested: {len(needles)}")
    print(f"  Matches: {len(needles) - mismatches}")
    print(f"  Mismatches: {mismatches}")

    if mismatches > 0:
        print(f"\n❌ FAILED: {mismatches} needle(s) returned different results")
        sys.exit(1)
    else:
        print(f"\n✅ PASSED: all needles returned identical results")
        sys.exit(0)


if __name__ == "__main__":
    main()
