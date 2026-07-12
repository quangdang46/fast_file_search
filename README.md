<p align="center">
  <img src="fast_file_search_illustration.webp" alt="ffs" width="720">
</p>

<p align="center">
  <b>A code-aware file search CLI for humans and AI agents. Really fast.</b>
</p>

<p align="center">
  <a href="#"><img src="https://img.shields.io/github/v/release/quangdang46/fast_file_search?logo=github&label=release" alt="Release"></a>
  <a href="#"><img src="https://img.shields.io/github/actions/workflow/status/quangdang46/fast_file_search/ci.yml?branch=main&logo=github&label=CI" alt="CI"></a>
  <a href="#"><img src="https://img.shields.io/badge/license-MIT-green" alt="MIT"></a>
  <a href="#"><img src="https://img.shields.io/badge/Rust-stable-blue?logo=rust" alt="Rust"></a>
</p>

```bash
curl -fsSL "https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.sh?$(date +%s)" | bash
```

---

## TL;DR

**The Problem** — File search and code navigation require a menagerie of tools: `find`, `fd`, `grep`, `rg`, `glob`, `cat`, plus a tree-sitter plugin for symbol queries. Each uses different syntax, has different startup cost, and produces different output formats. AI agents struggle with this combinatorial explosion of invocation patterns.

**The Solution** — `ffs` replaces all of them with a **single binary** that understands code. Typo-resistant fuzzy matching, frecency-ranked results, tree-sitter-powered symbol/ callers/ callees/ refs queries, and a token-budget aware file reader — all from one executable that starts in milliseconds and keeps a warm cache for sub-10 ms subsequent calls.

**Why `ffs` over the Unix tool zoo?**

| Capability | `ffs` | `find`/`fd` | `grep`/`rg` | `glob` | `cat` |
|---|---|---|---|---|---|
| File-name search (fuzzy, frecency) | ✓ | ✓/✓ | — | — | — |
| Content grep (SIMD, typo-tolerant) | ✓ | — | ✓/✓ | — | — |
| **Symbol lookup** (tree-sitter) | ✓ | — | — | — | — |
| **Callers / callees / refs** | ✓ | — | — | — | — |
| **Token-budget file reader** | ✓ | — | — | — | ✓ |
| Globbing | ✓ | — | — | ✓ | — |
| **Single process, warm cache** | ✓ | — | — | — | — |

---

## Quick Example

```bash
# Index once, query forever
ffs index                                     # ~200 ms on 10k-file repo
ffs find UnifiedScanner                       # fuzzy file-name search
ffs find grep --scored                        # role-aware ranking (+20 impl, -15 test)
ffs grep '\bTODO\b' --root crates/            # content grep
ffs grep 'fn main' --group                    # symbol-grouped grep output
ffs symbol FilePicker                         # tree-sitter symbol lookup
ffs callers UnifiedScanner                    # who calls a symbol
ffs read crates/ffs-engine/src/lib.rs --budget 5000  # token-budget reader
ffs map --depth 3                             # workspace tree
ffs mcp                                       # run as MCP server over stdio
```

---

## Design Philosophy

| Principle | Rationale |
|---|---|
| **One binary to rule them all** | `find` + `grep` + `glob` + `cat` + `symbol` + `callers` in a single executable. No pipelining, no format wars. |
| **Typo-tolerant by default** | Query `*.rs !test/ shcema` works even with the typo in `shcema`. |
| **Frecency ranking** | Files you open more often rank higher. Warm-up uses git touch history. |
| **Tree-sitter symbol index** | Answers code questions, not just file-name questions — across 16 languages. |
| **Token-budget reader** | `ffs read path --budget 5000` clips the body but always preserves the file header and a `[truncated to budget]` footer so agents know output was clipped. |
| **One warm process** | After the first call, every subsequent query hits warm memory — no per-call subprocess spawn, no `.gitignore` re-read. |

---

## Installation

```bash
# macOS / Linux — curl pipe
curl -fsSL "https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.sh?$(date +%s)" | bash

# Windows PowerShell
irm https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.ps1 | iex

# From source
git clone https://github.com/quangdang46/fast_file_search.git
cd fast_file_search
cargo build --release -p ffs-cli --features zlob
./target/release/ffs --version
```

The installers detect your platform, fetch the matching binary from GitHub Releases, verify the SHA-256 sidecar, and atomically install to `~/.local/bin/ffs`. MCP server configs are auto-detected for claude-code, codex, cursor, windsurf, vscode, gemini, opencode, etc.

### Supported platforms

- Linux x86_64 / aarch64 (musl-linked, portable across glibc)
- macOS x86_64 / aarch64
- Windows x86_64 / aarch64

---

## Quick Start

```bash
# One-time index (writes <repo>/.ffs/)
ffs index

# File-name search
ffs find UnifiedScanner
ffs find grep --scored

# Content search
ffs grep '\bTODO\b' --root crates/
ffs grep 'fn main' --group

# Code navigation
ffs symbol FilePicker
ffs callers UnifiedScanner
ffs callees UnifiedScanner
ffs refs UnifiedScanner
ffs flow UnifiedScanner
ffs deps crates/ffs-engine/src/lib.rs
ffs impact UnifiedScanner

# Token-budget reader
ffs read crates/ffs-engine/src/lib.rs --budget 5000 --filter minimal

# Workspace overview
ffs map --depth 3
ffs overview

# MCP server
ffs mcp
```

Pass `--format json` for machine-readable output. Use `--root <path>` to override the working directory.

---

## Subcommands

| Command | What it does |
|---|---|
| `find` | Fuzzy file-name search. Frecency-ranked, glob constraints, git-aware. |
| `glob` | Match files by glob (replaces `glob` and shell `**`). |
| `grep` | Content search. Plain / regex / fuzzy auto-detect. |
| `read` | Token-budget aware file read (replaces `cat`). |
| `outline` | Structural outline of a file (functions, classes, top-level decls). |
| `symbol` | Exact + prefix lookup over the tree-sitter symbol index. |
| `callers` | Find call sites of a symbol. Bloom-filter narrowed candidates. |
| `callees` | Symbols referenced inside a symbol body. |
| `refs` | Definitions + single-hop usages of a symbol in one shot. |
| `flow` | Drill-down envelope per definition (def + body + callees + callers). |
| `siblings` | Peers of a symbol in its parent scope. |
| `deps` | A file's imports + files that depend on it. |
| `impact` | Rank workspace files by how much a symbol change would affect them. |
| `index` | Build / refresh on-disk indexes (Bigram, Bloom, Symbol, Outline). |
| `map` | Workspace tree annotated with file count and per-directory tokens. |
| `overview` | High-signal repo summary (languages, top symbols, entry-points). |
| `mcp` | JSON-RPC MCP server over stdio (replaces agent built-ins). |
| `guide` | Embedded agent guide. |

---

## MCP Server

`ffs mcp` exposes 15 tools over stdio JSON-RPC. Any MCP-capable agent (Claude Code, Codex, OpenCode, Cursor, Cline, …) can call them:

| Tool | What it answers |
|---|---|
| `ffs_find` | Fuzzy file-name search |
| `ffs_grep` | Content search (plain / regex / fuzzy) |
| `ffs_multi_grep` | OR-logic multi-pattern search (SIMD Aho-Corasick) |
| `ffs_symbol` | Exact + prefix symbol lookup (tree-sitter) |
| `ffs_callers` | Call sites of a symbol |
| `ffs_callees` | Symbols referenced inside a definition body |
| `ffs_refs` | Definitions + single-hop usages |
| `ffs_flow` | Drill-down per definition (def + body + callees + callers) |
| `ffs_impact` | Files ranked by change impact |
| `ffs_outline` | File structural outline |
| `ffs_siblings` | Peers in the same parent scope |
| `ffs_deps` | File imports + dependents |
| `ffs_map` | Workspace tree with file/token counts |
| `ffs_overview` | High-signal repo summary |
| `ffs_read` | Token-budget aware file read |

---

## Performance & Benchmarks

All numbers are single-threaded medians on a Linux x86-64 machine (Criterion.rs).

**Engine dispatch** (query → ranked results, 256-file fixture):

| Query type | Median |
|---|---|
| Symbol lookup (`worker_05_3`) | **202 ns** |
| Concept / NL query | **205 ns** |
| File path (`mod_03/file_2.rs`) | **2.2 µs** |
| Glob (`**/*.rs`) | **2.8 µs** |

**Bigram index**:

| Index size | 2-char query | 6-char query | 14-char query |
|---|---|---|---|
| 10 K files | 46 ns | 120 ns | 314 ns |
| 500 K files | 761 ns | 1.6 µs | 1.6 µs |

**Case-insensitive memmem** (SIMD packed-pair, 80 KB file):

| Needle | `packed_pair` | `memchr2` | Speedup |
|---|---|---|---|
| `"fn"` (hit) | 44 ns | 204 ns | **4.6×** |
| `"search_file"` (hit) | 1.6 µs | 13.6 µs | **8.6×** |

**Symbol index** (1 K files indexed):

| Operation | Median |
|---|---|
| Index one Rust file (50 lines) | 219 µs |
| Exact symbol lookup | 95 µs |
| Prefix lookup (`"Wor"`) | 115 µs |

---

## Architecture

```
┌──────────────────────────────────────────────┐
│  Frontends                                    │
│  ffs-cli (binary) · ffs-mcp (stdio JSON-RPC) │
│  ffs-c (C ABI .so/.dylib/.dll)               │
└────────────────────┬─────────────────────────┘
                     │ all surfaces share one core
                     ▼
┌──────────────────────────────────────────────┐
│  Engine layer                                 │
│  ffs-engine  — dispatch · ranking · memory   │
│  ffs-query-parser — DSL with constraints      │
└────────────────────┬─────────────────────────┘
                     ▼
┌──────────────────────────────────────────────┐
│  Capability layer                              │
│  ffs-symbol  — tree-sitter · bloom · bigram   │
│  ffs-grep    — SIMD literal / regex search    │
│  ffs-budget  — token-aware reader · filters   │
└────────────────────┬─────────────────────────┘
                     ▼
┌──────────────────────────────────────────────┐
│  Core layer (ffs-core)                         │
│  scan · file_picker · score · bigram_filter   │
│  git · frecency · background_watcher · ignore  │
└──────────────────────────────────────────────┘
```

---

## Troubleshooting

| Symptom | Likely Cause | Fix |
|---|---|---|
| `ffs index` is slow | First build — expected. | It's a one-time cost. Subsequent calls skip re-parse. |
| Symbol queries return empty | Index is stale (git HEAD changed). | Re-run `ffs index`. |
| Fuzzy search produces unexpected results | Query is too short (1 char) or too generic. | Add more characters or path qualifiers (`src/`). |
| MCP server not responding | Server process exited. | Keep `ffs mcp` running as a background service. |

---

## FAQ

**Does `ffs` need a pre-built index?** Yes — run `ffs index` once per repo. The index is cached in `<repo>/.ffs/` and auto-invalidates on schema bumps, git HEAD changes, or significant file-count drift.

**Can I use `ffs` as a library from Rust?** Yes — all crates publish via `ffs-*`. The C ABI (`ffs-c`) is stable for foreign bindings (Python, Node, etc.).

**How does the typo-tolerant fuzzy search work?** The query parser classifies input into modes: symbol lookup, file path, glob, concept/NL, or raw text. Each mode applies a different prefilter + scorer. A Bloom filter + bigram index narrows candidates before any scoring runs.

**What's the difference between `ffs callers` and `ffs refs`?** `callers` lists call sites (who invokes a symbol). `refs` combines definitions and usages in one shot — both call sites and other references.

**Does `ffs` work on non-git directories?** Yes — it falls back to a full filesystem scan. Some features (frecency warm-up, cache invalidation) require git.
