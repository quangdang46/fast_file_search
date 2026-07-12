# ffs — Fast File Search

<div align="center">
  <img src="fast_file_search_illustration.webp" alt="ffs — code-aware file search CLI for humans and AI agents">
</div>

<div align="center">

![Release](https://img.shields.io/github/v/release/quangdang46/fast_file_search?logo=github&label=release)
![CI](https://img.shields.io/github/actions/workflow/status/quangdang46/fast_file_search/ci.yml?branch=main&logo=github&label=CI)
![License](https://img.shields.io/badge/License-MIT-green.svg)
![Rust](https://img.shields.io/badge/Rust-stable-blue?logo=rust)

</div>

**A code-aware file search CLI for humans and AI agents. Really fast.**  
Replaces `find` + `grep` + `glob` + `cat` + tree-sitter with one binary. Typo-tolerant fuzzy matching, frecency ranking, token-budget reader, MCP server.

<div align="center">

```bash
curl -fsSL "https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.sh?$(date +%s)" | bash
```

</div>

---

## 🤖 Agent Quickstart (MCP / Robot Mode)

ffs ships a 15-tool MCP server over stdio. The installer auto-configures it for Claude Code, Codex, Cursor, Windsurf, Gemini, OpenCode, and more.

```bash
# Run as MCP server
ffs mcp

# Agent-friendly JSON queries
ffs grep 'fn main' --format json
ffs symbol FilePicker --format json
ffs read crates/ffs-engine/src/lib.rs --budget 5000 --format json
ffs map --depth 3 --format json
ffs overview --format json
ffs deps path/to/file.rs --format json
```

**Output conventions**
- stdout = structured data (JSON or text)
- stderr = diagnostics, warnings
- exit 0 = success

**MCP tools exposed:** `ffs_find`, `ffs_grep`, `ffs_multi_grep`, `ffs_symbol`, `ffs_callers`, `ffs_callees`, `ffs_refs`, `ffs_flow`, `ffs_impact`, `ffs_outline`, `ffs_siblings`, `ffs_deps`, `ffs_map`, `ffs_overview`, `ffs_read`

---

## TL;DR

### The Problem

File search and code navigation require a menagerie of tools: `find`, `fd`, `grep`, `rg`, `glob`, `cat`, plus a tree-sitter plugin for symbol queries. Each has different syntax, different startup cost, different output formats. AI agents struggle with this combinatorial explosion of invocation patterns — and every pipeline hop loses milliseconds that add up across hundreds of queries.

### The Solution

`ffs` replaces all of them with a **single binary** that understands code. Typo-resistant fuzzy matching, frecency-ranked results, tree-sitter-powered symbol/callers/callees/refs queries, and a token-budget aware file reader — all from one executable that starts in milliseconds and keeps a warm cache for sub-10 ms subsequent calls.

### Why ffs?

| Feature | What it does |
|---------|--------------|
| **One binary to rule them all** | `find` + `grep` + `glob` + `cat` + `symbol` + `callers` + `refs` in a single executable |
| **Typo-tolerant by default** | Query `*.rs !test/ shcema` works even with the typo |
| **Frecency ranking** | Files you open more often rank higher. Warm-up uses git touch history |
| **Tree-sitter symbol index** | Answers code questions across 16 languages |
| **Token-budget reader** | `ffs read path --budget 5000` clips body, preserves header and truncated footer |
| **One warm process** | After first call, every subsequent query hits warm memory — no per-call subprocess spawn |
| **MCP server** | 15-tool stdio JSON-RPC server for AI coding agents |
| **C ABI (.so/.dylib/.dll)** | Stable foreign bindings for Python, Node.js, and more |
| **zlib compression** | Optional `--features zlib` for compressed indexes |

### How ffs Compares

| Capability | ffs | `find`/`fd` | `grep`/`rg` | `glob` | `cat` |
|-----------|-----|-------------|-------------|--------|-------|
| File-name search (fuzzy, frecency) | ✅ | ✅/✅ | — | — | — |
| Content grep (SIMD, typo-tolerant) | ✅ | — | ✅/✅ | — | — |
| **Symbol lookup** (tree-sitter) | ✅ | — | — | — | — |
| **Callers / callees / refs** | ✅ | — | — | — | — |
| **Token-budget file reader** | ✅ | — | — | — | ✅ |
| MCP server | ✅ 15 tools | — | — | — | — |
| **Single process, warm cache** | ✅ | — | — | — | — |
| Globbing | ✅ | — | — | ✅ | — |

---

## Quick Example

```bash
# Index once, query forever
ffs index                                     # ~200 ms on 10k-file repo
ffs find UnifiedScanner                       # fuzzy file-name search
ffs find grep --scored                        # role-aware ranking
ffs grep '\bTODO\b' --root crates/            # content grep
ffs grep 'fn main' --group                    # symbol-grouped grep
ffs symbol FilePicker                         # tree-sitter symbol lookup
ffs callers UnifiedScanner                    # who calls a symbol
ffs read crates/ffs-engine/src/lib.rs --budget 5000  # token-budget reader
ffs map --depth 3                             # workspace tree
ffs mcp                                       # run as MCP server over stdio
```

---

## Design Philosophy

| Principle | Rationale |
|-----------|-----------|
| **One binary to rule them all** | `find` + `grep` + `glob` + `cat` + `symbol` + `callers` — no pipelining, no format wars |
| **Typo-tolerant by default** | Fuzzy auto-detect query mode; `shcema` still finds the right file |
| **Frecency ranking** | Files you open more often rank higher; warm-up uses git touch history |
| **Tree-sitter symbol index** | Answers code questions, not just file-name questions — across 16 languages |
| **Token-budget reader** | Clips body but always preserves header + `[truncated to budget]` footer |
| **One warm process** | After first call, every subsequent query hits warm memory — no per-call subprocess spawn |

---

## Installation

```bash
# macOS / Linux — curl pipe
curl -fsSL "https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.sh?$(date +%s)" | bash

# Windows PowerShell
irm https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.ps1 | iex

# From source (with zlib compression support)
cargo build --release -p ffs-cli --features zlib
./target/release/ffs --version
```

The installers detect platform, fetch the matching release binary, verify SHA-256 sidecar, and atomically install to `~/.local/bin/ffs`. MCP server configs are auto-detected for Claude Code, Codex, Cursor, Windsurf, VS Code, Gemini, OpenCode, and more.

### Supported platforms

Linux x86_64/aarch64, macOS x86_64/aarch64, Windows x86_64/aarch64

---

## Commands

| Command | What it does |
|---------|--------------|
| `find` | Fuzzy file-name search. Frecency-ranked, glob constraints, git-aware |
| `glob` | Match files by glob pattern (replaces shell `**`) |
| `grep` | Content search — plain / regex / fuzzy auto-detect |
| `multi-grep` | OR-logic multi-pattern search (SIMD Aho-Corasick) |
| `read` | Token-budget aware file read (replaces `cat`) |
| `outline` | Structural outline of a file (functions, classes, top-level decls) |
| `symbol` | Exact + prefix lookup over the tree-sitter symbol index |
| `callers` | Find call sites of a symbol. Bloom-filter narrowed candidates |
| `callees` | Symbols referenced inside a symbol body |
| `refs` | Definitions + single-hop usages in one shot |
| `flow` | Drill-down envelope per definition (def + body + callees + callers) |
| `siblings` | Peers of a symbol in its parent scope |
| `deps` | A file's imports + files that depend on it |
| `impact` | Rank workspace files by change impact for a symbol |
| `index` | Build / refresh on-disk indexes (Bigram, Bloom, Symbol, Outline) |
| `map` | Workspace tree annotated with file count and per-directory tokens |
| `overview` | High-signal repo summary (languages, top symbols, entry-points) |
| `mcp` | JSON-RPC MCP server over stdio (replaces agent built-in tools) |
| `guide` | Embedded agent guide |

---

## Performance

All numbers are single-threaded medians on Linux x86-64 (Criterion.rs).

### Engine dispatch (256-file fixture)

| Query type | Median |
|------------|--------|
| Symbol lookup (`worker_05_3`) | **202 ns** |
| Concept / NL query | **205 ns** |
| File path (`mod_03/file_2.rs`) | **2.2 µs** |
| Glob (`**/*.rs`) | **2.8 µs** |

### Bigram index

| Index size | 2-char query | 6-char query | 14-char query |
|-----------|--------------|--------------|---------------|
| 10 K files | 46 ns | 120 ns | 314 ns |
| 500 K files | 761 ns | 1.6 µs | 1.6 µs |

### Symbol index (1 K files)

| Operation | Median |
|-----------|--------|
| Index one Rust file (50 lines) | 219 µs |
| Exact symbol lookup | 95 µs |

---

## Architecture

```
┌──────────────────────────────────────┐
│  Frontends                            │
│  ffs-cli (binary) · ffs-mcp (MCP)    │
│  ffs-c (C ABI .so/.dylib/.dll)        │
└──────────────┬───────────────────────┘
               │ all surfaces share one core
               ▼
┌──────────────────────────────────────┐
│  Engine layer                         │
│  ffs-engine — dispatch · ranking      │
│  ffs-query-parser — DSL + constraints  │
└──────────────┬───────────────────────┘
               ▼
┌──────────────────────────────────────┐
│  Capability layer                     │
│  ffs-symbol — tree-sitter · bloom     │
│  ffs-grep — SIMD literal / regex      │
│  ffs-budget — token-aware reader      │
└──────────────┬───────────────────────┘
               ▼
┌──────────────────────────────────────┐
│  Core layer (ffs-core)                │
│  scan · file_picker · score · git      │
│  frecency · watcher · ignore           │
└──────────────────────────────────────┘
```

---

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| `ffs index` is slow | First build — expected | One-time cost. Subsequent calls skip re-parse |
| Symbol queries return empty | Index stale (git HEAD changed) | Re-run `ffs index` |
| Fuzzy search unexpected results | Query too short (1 char) | Add more characters or path qualifiers (`src/`) |
| MCP server not responding | Server process exited | Keep `ffs mcp` running as background service |

---

## Limitations

| Edge case | Reality |
|-----------|---------|
| **Pre-built index required** | Run `ffs index` once per repo. Index auto-invalidates on HEAD changes |
| **16 languages** | Tree-sitter supports 16; non-listed languages fall back to text-only queries |
| **Frecency needs git** | Warm-up uses git touch history; without git, frecency starts cold |
| **MCP stdio only** | `ffs mcp` speaks JSON-RPC over stdio — no HTTP/WebSocket transport (yet) |

---

## FAQ

### Does ffs need a pre-built index?

Yes — run `ffs index` once per repo. The index is cached in `<repo>/.ffs/` and auto-invalidates on schema bumps, git HEAD changes, or significant file-count drift.

### Can I use ffs as a library from Rust?

Yes — all crates publish via `ffs-*`. The C ABI (`ffs-c`) is stable for foreign bindings (Python, Node.js, etc.).

### What's the difference between `ffs callers` and `ffs refs`?

`callers` lists call sites (who invokes a symbol). `refs` combines definitions and usages in one shot — both call sites and other references.

### Does ffs work on non-git directories?

Yes — it falls back to a full filesystem scan. Some features (frecency warm-up, cache invalidation) require git.

### MCP vs CLI — which should agents use?

Prefer `ffs mcp` for agent contexts — 15 tools via stdio JSON-RPC, no parsing required. Use CLI with `--format json` for one-shot queries.

---

<div align="center">
  <sub>One binary. All your code-search tools. <10ms warm.</sub>
</div>
