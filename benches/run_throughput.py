#!/usr/bin/env python3
"""Benchmark sequential query throughput: cold spawn vs warm MCP.

Measures total time for N sequential queries. This is the real AI agent
workload: agent makes 20-50 queries in a session.

Usage:
    python benches/run_throughput.py <repo> [--queries N]
"""
import argparse
import json
import os
import subprocess
import sys
import time


def run_cold_spawn(queries: list[str], repo: str, ffs_bin: str) -> dict:
    """Run N queries as separate process spawns (cold each time)."""
    times = []
    for q in queries:
        t0 = time.perf_counter_ns()
        subprocess.run(
            [ffs_bin, "grep", q, "--root", repo, "-l"],
            capture_output=True, timeout=30
        )
        t1 = time.perf_counter_ns()
        times.append((t1 - t0) / 1e6)
    return {"mode": "cold_spawn", "times": times, "total_ms": sum(times)}


def run_warm_mcp(queries: list[str], repo: str, ffs_bin: str) -> dict:
    """Run N queries through MCP (warm, single process)."""
    cmd = [ffs_bin, "mcp", repo]
    proc = subprocess.Popen(
        cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL
    )

    def send(method, params, rid):
        msg = json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params}) + "\n"
        proc.stdin.write(msg.encode())
        proc.stdin.flush()
        deadline = time.time() + 10
        while time.time() < deadline:
            line = proc.stdout.readline()
            if not line:
                break
            try:
                resp = json.loads(line)
                if resp.get("id") == rid:
                    return resp
            except json.JSONDecodeError:
                continue
        return {"error": "timeout"}

    try:
        # Initialize
        send("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                            "clientInfo": {"name": "bench", "version": "1.0"}}, 0)
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}).encode() + b"\n")
        proc.stdin.flush()
        time.sleep(0.5)

        # Warmup
        for i in range(3):
            send("tools/call", {"name": "ffs_grep", "arguments": {"query": queries[i % len(queries)], "maxResults": 10}}, 1000 + i)

        # Benchmark
        times = []
        for i, q in enumerate(queries):
            t0 = time.perf_counter_ns()
            send("tools/call", {"name": "ffs_grep", "arguments": {"query": q, "maxResults": 10}}, 2000 + i)
            t1 = time.perf_counter_ns()
            times.append((t1 - t0) / 1e6)

        return {"mode": "warm_mcp", "times": times, "total_ms": sum(times)}
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


def main():
    parser = argparse.ArgumentParser(description="Benchmark cold vs warm throughput")
    parser.add_argument("repo", help="Repository path")
    parser.add_argument("--queries", type=int, default=20, help="Number of queries")
    parser.add_argument("--ffs-bin", default="./target/release/ffs", help="Path to ffs binary")
    parser.add_argument("--seed", type=int, default=42, help="Random seed")
    args = parser.parse_args()

    import random
    random.seed(args.seed)
    repo = os.path.abspath(args.repo)

    # Generate queries
    try:
        result = subprocess.run(["rg", "-o", "--no-filename", r"\b\w{4,12}\b", repo],
                                capture_output=True, text=True, timeout=30)
        from collections import Counter
        words = Counter(result.stdout.strip().split("\n"))
        rare = [w for w, c in words.items() if 1 <= c <= 3]
        mid = [w for w, c in words.items() if 4 <= c <= 10]
        common = [w for w, c in words.items() if c > 10]
        queries = []
        for tier in [common, mid, rare]:
            n = min(args.queries // 3 + 1, len(tier))
            if n > 0:
                queries.extend(random.sample(tier, n))
        queries = queries[:args.queries]
    except Exception:
        queries = ["TODO", "fn ", "struct ", "impl ", "pub "] * (args.queries // 5 + 1)
        queries = queries[:args.queries]

    print(f"=== Throughput benchmark: {args.queries} queries on {repo} ===")
    print()

    # Cold spawn
    print("Running cold spawn...")
    cold = run_cold_spawn(queries, repo, args.ffs_bin)
    cold_times = sorted(cold["times"])
    n = len(cold_times)
    print(f"  Total:  {cold['total_ms']:.0f} ms")
    print(f"  Mean:   {sum(cold_times)/n:.1f} ms/query")
    print(f"  Median: {cold_times[n//2]:.1f} ms/query")
    print(f"  p95:    {cold_times[int(n*0.95)]:.1f} ms/query")
    print()

    # Warm MCP
    print("Running warm MCP...")
    warm = run_warm_mcp(queries, repo, args.ffs_bin)
    warm_times = sorted(warm["times"])
    n = len(warm_times)
    print(f"  Total:  {warm['total_ms']:.0f} ms")
    print(f"  Mean:   {sum(warm_times)/n:.1f} ms/query")
    print(f"  Median: {warm_times[n//2]:.1f} ms/query")
    print(f"  p95:    {warm_times[int(n*0.95)]:.1f} ms/query")
    print()

    # Comparison
    speedup = cold['total_ms'] / warm['total_ms'] if warm['total_ms'] > 0 else 0
    print(f"=== Summary ===")
    print(f"  Cold spawn total: {cold['total_ms']:.0f} ms ({args.queries} queries)")
    print(f"  Warm MCP total:   {warm['total_ms']:.0f} ms ({args.queries} queries)")
    print(f"  Warm/Cold ratio:  {speedup:.2f}x {'(warm faster)' if speedup > 1 else '(cold faster)'}")


if __name__ == "__main__":
    main()
