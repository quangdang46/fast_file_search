#!/usr/bin/env python3
"""Benchmark ffs MCP server warm latency.

Opens ffs mcp once, sends N JSON-RPC queries via stdin/stdout, measures
round-trip latency for each. This is the real AI agent use case.

Usage:
    python benches/run_warm_mcp.py <repo> [--queries N] [--warmup N]

Outputs p50/p95/p99 latency and per-query breakdown.
"""
import argparse
import json
import os
import subprocess
import sys
import time
import statistics


def send_request(proc, method: str, params: dict, request_id: int) -> dict:
    """Send a JSON-RPC request and wait for response."""
    request = {
        "jsonrpc": "2.0",
        "id": request_id,
        "method": method,
        "params": params,
    }
    msg = json.dumps(request) + "\n"
    proc.stdin.write(msg.encode())
    proc.stdin.flush()

    # Read response (may have other JSON lines before the response)
    deadline = time.time() + 10
    while time.time() < deadline:
        line = proc.stdout.readline()
        if not line:
            break
        try:
            resp = json.loads(line)
            if resp.get("id") == request_id:
                return resp
        except json.JSONDecodeError:
            continue
    return {"error": "timeout"}


def generate_needles(repo: str, count: int = 20) -> list[str]:
    """Generate search queries of different types for the repo."""
    try:
        result = subprocess.run(
            ["rg", "-o", "--no-filename", r"\b\w{4,12}\b", repo],
            capture_output=True, text=True, timeout=30
        )
        from collections import Counter
        words = Counter(result.stdout.strip().split("\n"))

        # Stratify by frequency
        rare = [w for w, c in words.items() if 1 <= c <= 3]
        mid = [w for w, c in words.items() if 4 <= c <= 10]
        common = [w for w, c in words.items() if c > 10]

        queries = []
        for tier in [common, mid, rare]:
            n = min(count // 3 + 1, len(tier))
            if n > 0:
                import random
                queries.extend(random.sample(tier, n))

        return queries[:count]
    except Exception:
        return ["TODO", "fn ", "struct ", "impl ", "pub "]


def run_mcp_benchmark(repo: str, ffs_bin: str, queries: list[str],
                      warmup: int = 3, mcp_args: list[str] = None) -> dict:
    """Run MCP benchmark: open ffs mcp, send queries, measure latency."""
    cmd = [ffs_bin, "mcp", repo]
    if mcp_args:
        cmd.extend(mcp_args)

    proc = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )

    try:
        # Initialize
        init_resp = send_request(proc, "initialize", {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "bench", "version": "1.0"}
        }, 0)

        # Send initialized notification
        proc.stdin.write(json.dumps({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        }).encode() + b"\n")
        proc.stdin.flush()

        # Wait for index to be ready
        time.sleep(0.5)

        # Warmup
        for i in range(warmup):
            q = queries[i % len(queries)]
            send_request(proc, "tools/call", {
                "name": "ffs_grep",
                "arguments": {"query": q, "maxResults": 10}
            }, 1000 + i)

        # Benchmark
        latencies = []
        for i, q in enumerate(queries):
            t0 = time.perf_counter_ns()
            resp = send_request(proc, "tools/call", {
                "name": "ffs_grep",
                "arguments": {"query": q, "maxResults": 10}
            }, 2000 + i)
            t1 = time.perf_counter_ns()

            latency_ms = (t1 - t0) / 1e6
            latencies.append({
                "query": q,
                "latency_ms": latency_ms,
                "error": resp.get("error") is not None,
            })

        return {
            "total_queries": len(queries),
            "warmup": warmup,
            "latencies": latencies,
        }

    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()


def main():
    parser = argparse.ArgumentParser(description="Benchmark ffs MCP warm latency")
    parser.add_argument("repo", help="Repository path")
    parser.add_argument("--queries", type=int, default=20, help="Number of queries")
    parser.add_argument("--warmup", type=int, default=3, help="Warmup queries")
    parser.add_argument("--ffs-bin", default="./target/release/ffs", help="Path to ffs binary")
    parser.add_argument("--json", action="store_true", help="Output JSON")
    parser.add_argument("--seed", type=int, default=42, help="Random seed")
    args = parser.parse_args()

    import random
    random.seed(args.seed)

    repo = os.path.abspath(args.repo)
    queries = generate_needles(repo, args.queries)

    print(f"=== ffs MCP warm benchmark ===")
    print(f"repo:    {repo}")
    print(f"queries: {len(queries)}")
    print(f"warmup:  {args.warmup}")
    print()

    result = run_mcp_benchmark(repo, args.ffs_bin, queries, args.warmup)

    # Calculate stats
    latencies = [l["latency_ms"] for l in result["latencies"] if not l["error"]]
    errors = sum(1 for l in result["latencies"] if l["error"])

    if latencies:
        latencies_sorted = sorted(latencies)
        n = len(latencies_sorted)
        p50 = latencies_sorted[n // 2]
        p95 = latencies_sorted[int(n * 0.95)]
        p99 = latencies_sorted[int(n * 0.99)]

        print(f"=== Results ===")
        print(f"  Queries: {len(latencies)} ({errors} errors)")
        print(f"  Mean:    {statistics.mean(latencies):.1f} ms")
        print(f"  Median:  {p50:.1f} ms")
        print(f"  p95:     {p95:.1f} ms")
        print(f"  p99:     {p99:.1f} ms")
        print(f"  Min:     {min(latencies):.1f} ms")
        print(f"  Max:     {max(latencies):.1f} ms")
        print()

        # Per-query breakdown
        print(f"=== Per-query latency ===")
        for l in result["latencies"]:
            status = "❌" if l["error"] else "  "
            print(f"  {status} {l['query']:20s} {l['latency_ms']:8.1f} ms")

    if args.json:
        print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
