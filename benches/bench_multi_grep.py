#!/usr/bin/env python3
"""Reproducible before/after benchmark for the embedded MCP multi-grep path.

Builds its own fixture, drives a warm `ffs mcp` process over JSON-RPC, and
prints raw per-sample timings plus medians. Designed to be run against two
binaries (e.g. a base-commit build and a working-tree build) so the ratio is
measured on identical data on the same machine.

Usage:
    python benches/bench_multi_grep.py --before /path/to/ffs-base \
                                       --after  /path/to/ffs-new
    python benches/bench_multi_grep.py --after ./target/release/ffs  # solo
"""
import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time


def build_fixture(root: str, small_files: int = 2000, bulk_lines: int = 20000) -> str:
    """2,000 small files with no match + one big file with 2,000 matches.

    The small files must NOT match, otherwise the default result limit fills up
    before the big file is ever read and the measured code never runs.
    """
    src = os.path.join(root, "src")
    os.makedirs(src, exist_ok=True)
    for i in range(small_files):
        with open(os.path.join(src, f"small{i}.rs"), "w") as f:
            f.write(f"fn filler_{i}() {{ return; }}\n" * 3)
    with open(os.path.join(src, "large.rs"), "w") as f:
        for i in range(bulk_lines):
            f.write(f"row {i} NEEDLE payload\n" if i % 10 == 0 else f"row {i} plain\n")
    return src


class Mcp:
    def __init__(self, binary: str, root: str):
        self.p = subprocess.Popen(
            [binary, "mcp", root],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        self._send(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "bench", "version": "1"},
            },
            0,
        )
        self.p.stdin.write(
            json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}).encode()
            + b"\n"
        )
        self.p.stdin.flush()

    def _send(self, method, params, rid):
        msg = json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        self.p.stdin.write(msg.encode() + b"\n")
        self.p.stdin.flush()
        while True:
            line = self.p.stdout.readline()
            if not line:
                return {}
            try:
                resp = json.loads(line)
            except json.JSONDecodeError:
                continue
            if resp.get("id") == rid:
                return resp

    def multi_grep(self, queries, limit=None):
        args = {"queries": queries}
        if limit is not None:
            args["maxResults"] = limit
        return self._send("tools/call", {"name": "ffs_multi_grep", "arguments": args}, 2)

    def close(self):
        try:
            self.p.stdin.close()
            self.p.terminate()
        except Exception:
            pass


def bench(binary: str, root: str, limit, reps: int):
    client = Mcp(binary, root)
    # Warmup: first request pays the scan.
    client.multi_grep(["NEEDLE"], limit)
    samples = []
    for _ in range(reps):
        t0 = time.perf_counter_ns()
        client.multi_grep(["NEEDLE"], limit)
        samples.append((time.perf_counter_ns() - t0) / 1e6)
    client.close()
    return samples


def summarise(samples):
    s = sorted(samples)
    return {
        "n": len(s),
        "median_ms": round(statistics.median(s), 2),
        "mean_ms": round(statistics.mean(s), 2),
        "min_ms": round(s[0], 2),
        "samples_ms": [round(x, 2) for x in samples],
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--before", help="baseline ffs binary")
    ap.add_argument("--after", help="candidate ffs binary")
    ap.add_argument("--reps", type=int, default=7)
    ap.add_argument("--limit", type=int, default=2000,
                    help="maxResults; must exceed the big file's match count "
                         "to exercise the per-match line mapping")
    args = ap.parse_args()

    if not args.before and not args.after:
        ap.error("pass --before and/or --after")

    fixture_root = tempfile.mkdtemp(prefix="ffsbench-")
    try:
        build_fixture(fixture_root)
        result = {
            "fixture": {
                "root": fixture_root,
                "small_files": 2000,
                "large_file_lines": 20000,
                "large_file_matches": 2000,
                "queries": ["NEEDLE"],
                "maxResults": args.limit,
                "reps": args.reps,
            }
        }
        for label in ("before", "after"):
            binary = getattr(args, label)
            if not binary:
                continue
            if not os.path.exists(binary):
                sys.exit(f"{label} binary not found: {binary}")
            result[label] = summarise(bench(binary, fixture_root, args.limit, args.reps))

        if "before" in result and "after" in result:
            b = result["before"]["median_ms"]
            a = result["after"]["median_ms"]
            result["delta_median_pct"] = round((a - b) / b * 100, 2)

        print(json.dumps(result, indent=2))
    finally:
        shutil.rmtree(fixture_root, ignore_errors=True)


if __name__ == "__main__":
    main()
