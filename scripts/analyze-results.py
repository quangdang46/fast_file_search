#!/usr/bin/env python3
"""Analyze benchmark results across all concepts and iterations."""
import json, os, glob, sys

results_dir = sys.argv[1] if len(sys.argv) > 1 else "scripts/benchmark-results"

# Concept names (order matches benchmark-claude.sh)
CONCEPT_NAMES = [
    "fuzzy-function-search",
    "api-endpoint-discovery",
    "cross-service-config",
    "test-file-discovery",
    "error-type-definition",
    "database-model-search",
    "auth-flow-tracing",
    "todo-tech-debt",
    "cross-language-pattern",
    "broad-pattern-search",
]

def load_iter_results(concept_name, mode):
    results = []
    for i in range(1, 100):
        path = os.path.join(results_dir, f"{concept_name}-{mode}-iter{i}.json")
        if not os.path.exists(path):
            break
        try:
            with open(path) as f:
                data = json.load(f)
                if data.get("total_cost_usd", 0) > 0:
                    results.append(data)
        except:
            pass
    # Also check the non-iter file as fallback
    if not results:
        path = os.path.join(results_dir, f"{concept_name}-{mode}.json")
        if os.path.exists(path):
            try:
                with open(path) as f:
                    data = json.load(f)
                    if data.get("total_cost_usd", 0) > 0:
                        results.append(data)
            except:
                pass
    return results

def load_stream_trace(concept_name, mode, iteration):
    """Load tool call trace from stream file."""
    path = os.path.join(results_dir, f"{concept_name}-{mode}-iter{iteration}.stream.jsonl")
    if not os.path.exists(path):
        path = os.path.join(results_dir, f"{concept_name}-{mode}.stream.jsonl")
    if not os.path.exists(path):
        return []

    tool_calls = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except:
                continue
            if msg.get("type") == "assistant" and "message" in msg:
                for block in msg["message"].get("content", []):
                    if isinstance(block, dict) and block.get("type") == "tool_use":
                        name = block.get("name", "?")
                        inp = block.get("input", {})
                        summary = ""
                        if "query" in inp:
                            summary = inp["query"]
                        elif "pattern" in inp:
                            summary = inp["pattern"]
                        elif "patterns" in inp:
                            summary = str(inp["patterns"])
                        elif "file_path" in inp:
                            summary = inp["file_path"].split("/")[-1]
                        elif "command" in inp:
                            summary = inp["command"][:50]
                        tool_calls.append({"name": name, "summary": summary})
    return tool_calls


print("=" * 90)
print("  ffs MCP vs Native — All Concepts Summary")
print("=" * 90)

total_ffs_cost = 0
total_nat_cost = 0
ffs_wins = 0
nat_wins = 0
ties = 0

concept_data = []

for i, name in enumerate(CONCEPT_NAMES):
    ffs_results = load_iter_results(name, "ffs")
    nat_results = load_iter_results(name, "native")

    if not ffs_results and not nat_results:
        continue

    ffs_avg_cost = sum(r.get("total_cost_usd", 0) for r in ffs_results) / max(len(ffs_results), 1)
    nat_avg_cost = sum(r.get("total_cost_usd", 0) for r in nat_results) / max(len(nat_results), 1)
    ffs_avg_turns = sum(r.get("num_turns", 0) for r in ffs_results) / max(len(ffs_results), 1)
    nat_avg_turns = sum(r.get("num_turns", 0) for r in nat_results) / max(len(nat_results), 1)
    ffs_avg_wall = sum(r.get("wall_ms", 0) for r in ffs_results) / max(len(ffs_results), 1) / 1000
    nat_avg_wall = sum(r.get("wall_ms", 0) for r in nat_results) / max(len(nat_results), 1) / 1000

    if ffs_avg_cost < nat_avg_cost * 0.95:
        winner = "ffs"
        ffs_wins += 1
    elif nat_avg_cost < ffs_avg_cost * 0.95:
        winner = "NATIVE"
        nat_wins += 1
    else:
        winner = "TIE"
        ties += 1

    total_ffs_cost += ffs_avg_cost
    total_nat_cost += nat_avg_cost

    concept_data.append({
        "num": i + 1,
        "name": name,
        "ffs_cost": ffs_avg_cost,
        "nat_cost": nat_avg_cost,
        "ffs_turns": ffs_avg_turns,
        "nat_turns": nat_avg_turns,
        "ffs_wall": ffs_avg_wall,
        "nat_wall": nat_avg_wall,
        "ffs_n": len(ffs_results),
        "nat_n": len(nat_results),
        "winner": winner,
    })

# Print table
print(f"\n  {'#':>2} {'Concept':<28} {'ffs $':>8} {'Nat $':>8} {'ffs T':>5} {'Nat T':>5} {'ffs s':>6} {'Nat s':>6} {'N':>3} {'Winner':>8}")
print(f"  {'─'*2} {'─'*28} {'─'*8} {'─'*8} {'─'*5} {'─'*5} {'─'*6} {'─'*6} {'─'*3} {'─'*8}")

for d in concept_data:
    savings = (1 - d["ffs_cost"] / d["nat_cost"]) * 100 if d["nat_cost"] > 0 else 0
    print(f"  {d['num']:>2} {d['name']:<28} ${d['ffs_cost']:.4f} ${d['nat_cost']:.4f} {d['ffs_turns']:>5.1f} {d['nat_turns']:>5.1f} {d['ffs_wall']:>5.0f}s {d['nat_wall']:>5.0f}s {d['ffs_n']:>3} {d['winner']:>8}")

print(f"\n  Score: ffs {ffs_wins} | Native {nat_wins} | Tie {ties}")
print(f"  Total avg cost: ffs ${total_ffs_cost:.4f} | Native ${total_nat_cost:.4f}")
if total_nat_cost > 0:
    print(f"  Overall savings: {(1 - total_ffs_cost / total_nat_cost) * 100:+.1f}%")

# Show problematic concepts (where native wins by >20%)
print(f"\n{'─' * 90}")
print("  Concepts where ffs loses (native wins by >5%):")
for d in concept_data:
    if d["winner"] == "NATIVE":
        pct = (d["ffs_cost"] / d["nat_cost"] - 1) * 100
        print(f"    #{d['num']} {d['name']}: ffs is {pct:+.0f}% more expensive")
        # Show tool traces for the worst iteration
        traces = load_stream_trace(d["name"], "ffs", 1)
        if traces:
            print(f"      ffs trace: {' → '.join(t['name'].replace('mcp__ffs__','') for t in traces)}")
        traces = load_stream_trace(d["name"], "native", 1)
        if traces:
            print(f"      nat trace: {' → '.join(t['name'] for t in traces)}")

print()
