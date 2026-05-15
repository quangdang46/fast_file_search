<p>
  <i>A code-aware file search CLI for humans and AI agents. Really fast.</i>
</p>

ffs replaces `find`, `fd`, `grep`, `rg`, `glob`, and `cat` with a single
typo-resistant, frecency-ranked binary, and adds tree-sitter powered
code-navigation (`symbol`, `callers`, `callees`, `refs`, `flow`, `impact`)
and a token-budget aware file reader for AI agents.

---

## Install

```bash
curl -fsSL "https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.sh?$(date +%s)" | bash
```

That's it. The script detects your platform, fetches the matching
binary from [GitHub Releases](https://github.com/quangdang46/fast_file_search/releases/latest),
verifies the SHA-256 sidecar, and atomically installs to `~/.local/bin/ffs`.

### Flags

```bash
# Pin a version
curl -fsSL https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.sh \
  | bash -s -- --version v0.1.0

# System-wide install (needs sudo)
curl -fsSL https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.sh \
  | sudo bash -s -- --system

# Auto-update PATH in ~/.bashrc and ~/.zshrc
curl -fsSL https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.sh \
  | bash -s -- --easy-mode

# Build from source instead of downloading
curl -fsSL https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.sh \
  | bash -s -- --from-source

# Uninstall
curl -fsSL https://raw.githubusercontent.com/quangdang46/fast_file_search/main/install.sh \
  | bash -s -- --uninstall
```

Run `bash install.sh --help` after cloning the repo to see all flags.

### Supported platforms

- Linux x86_64 / aarch64 (musl-linked, portable across glibc versions)
- macOS x86_64 / aarch64
- Windows x86_64 / aarch64 (run install.sh from Git Bash or WSL)

---

## Quick start

```bash
ffs --help
ffs index                                  # one-time warm-up (~200ms on a 10k-file repo)
ffs find UnifiedScanner
ffs grep '\bTODO\b' --root crates/
ffs symbol FilePicker
ffs callers UnifiedScanner
ffs read crates/ffs-engine/src/lib.rs --budget 5000 --filter minimal
ffs dispatch 'where is the user controller'
ffs map --depth 3
ffs mcp                                    # run as MCP server over stdio
```

## Subcommands

```
ffs find        Find files by name (replaces find, fd)
ffs glob        Match files by glob (replaces glob, shell **)
ffs grep        Search file contents (replaces grep, rg)
ffs read        Read a file with token-budget aware truncation (replaces cat)
ffs outline     Render a file's structural outline
ffs symbol      Look up symbol definitions (tree-sitter powered)
ffs callers     List call sites of a symbol
ffs callees     List symbols referenced inside a symbol body
ffs refs        Definitions + single-hop usages of a symbol in one shot
ffs flow        Drill-down envelope per definition (def + body + callees + callers)
ffs siblings    Sibling symbols (peers in the same parent scope)
ffs deps        File imports + the workspace files that depend on it
ffs impact      Rank files by how much they'd be affected if a symbol changed
ffs dispatch    Auto-classify a free-form query and route it to the right backend
ffs index       Build / refresh the on-disk indexes (Bigram, Bloom, Symbol, Outline)
ffs map         Render the workspace as a tree annotated with file count and tokens
ffs overview    High-signal summary of the workspace (languages, top symbols, …)
ffs mcp         Run as an MCP server over stdio
ffs guide       Print the embedded agent guide
```

Pass `--format json` for machine-readable output. Use `--root <path>` to override
the working directory globally.

---

## MCP server

`ffs mcp` (or the standalone `ffs-mcp` binary, installed via
[`install-mcp.sh`](./install-mcp.sh)) speaks JSON-RPC over stdio and registers
16 tools that any MCP-capable agent (Claude Code, Codex, OpenCode, Cursor,
Cline, …) can call:

| Tool            | What it answers                                                                                  |
| --------------- | ------------------------------------------------------------------------------------------------ |
| `ffs_find`    | Fuzzy file-name search. Smart-case, frecency-ranked, glob constraints, git-aware.                |
| `ffs_grep`      | Content search. Plain / regex / fuzzy auto-detect, pagination cursor, definition-first hinting.  |
| `ffs_multi_grep`    | OR-logic multi-pattern content search via SIMD Aho-Corasick.                                     |
| `ffs_dispatch`  | Auto-classify a free-form query (path, glob, identifier, concept) and route through the engine.  |
| `ffs_symbol`    | Exact + prefix lookup over the tree-sitter symbol index (16 languages).                          |
| `ffs_callers`   | Find call sites of a symbol. Bloom-filter narrowed candidates → literal-text confirm pass.       |
| `ffs_callees`   | Symbols referenced inside the body of a definition.                                              |
| `ffs_refs`      | Definitions + single-hop usages of a symbol in one shot. Mirrors `ffs refs` from the CLI.        |
| `ffs_flow`      | Drill-down envelope per definition: def metadata + body excerpt + top-N callees + top-N callers. |
| `ffs_impact`    | Rank workspace files by how much they'd be affected if `name` changed.                           |
| `ffs_outline`   | Structural outline of a file (functions, classes, top-level decls). Agent-friendly default view. |
| `ffs_siblings`  | Peers of a symbol in its parent scope — surfaces the rest of the impl/class around a method.     |
| `ffs_deps`      | A file's imports plus the workspace files that depend on it. Blast-radius estimate for changes.  |
| `ffs_map`       | Workspace tree annotated with file count and per-directory token estimate.                       |
| `ffs_overview`  | High-signal repo summary: languages, top-defined symbols, entry-point candidates.                |
| `ffs_read`      | Token-budget aware file read. Maps `maxTokens` to `~85% body × 4 bytes/token`, applies filters.  |

Recommended agent prompt — drop into `CLAUDE.md` (or equivalent):

```markdown
For any file search, grep, or symbol lookup in the current git-indexed
directory, use ffs tools.
```

---

## Why ffs

- **Typo-resistant fuzzy matching** for both paths and content. `*.rs !test/ shcema`
  is a valid query; even with a typo in `shcema` it still finds matches.
- **Frecency-ranked.** Files you open often rank higher next time. Warm-up uses
  git touch history.
- **Tree-sitter symbol index** across 16 languages — answer code questions, not
  just file-name questions.
- **Bigram + Bloom pre-filter stack.** `ffs callers SomeSymbol` typically inspects
  fewer than 30 files on a 10k-file repo.
- **Token-budget aware reader** for AI agents. `ffs read path --budget 5000` clips
  the body but always preserves the file header and a `[truncated to budget]`
  footer so the agent knows the output was clipped.
- **One long-lived process when used via library/MCP.** No per-call subprocess
  spawn, no re-reading `.gitignore`, no rebuilding state. After the first call
  every subsequent search hits warm memory.

On a 500k-file Chromium checkout, that is the difference between 3-9 **seconds**
per ripgrep spawn and sub-10 ms per ffs query.

---

## Other surfaces

The same Rust core powers four other entry points. See each subdirectory for details.

| Surface | Path | What it is |
|---|---|---|
| **Neovim plugin** | [`lua/ffs/`](./lua/ffs/) + [`plugin/ffs.lua`](./plugin/ffs.lua) | High-performance file picker with live grep, frecency ranking, and tree-sitter-aware preview. |
| **MCP server** | [`crates/ffs-mcp/`](./crates/ffs-mcp/) + [`install-mcp.sh`](./install-mcp.sh) | Drop-in replacement for the built-in file-search tools in Claude Code, Codex, OpenCode, Cursor, Cline, and other MCP-capable agents. |
| **Node SDK** | [`packages/ffs-node/`](./packages/ffs-node/) | TypeScript wrapper over the C library for Node.js. |
| **Bun SDK** | [`packages/ffs-bun/`](./packages/ffs-bun/) | TypeScript wrapper over the C library for Bun. |
| **Pi extension** | [`packages/pi-ffs/`](./packages/pi-ffs/) | [pi](https://github.com/badlogic/pi-mono) extension that swaps native `find`/`grep` for ffs. |
| **C ABI** | [`crates/ffs-c/`](./crates/ffs-c/) + [`crates/ffs-c/include/ffs.h`](./crates/ffs-c/include/ffs.h) | Stable C library — bind from C/C++, Zig, Go via cgo, Python via ctypes. |
| **Rust crate** | [`crates/ffs-core/`](./crates/ffs-core/) ([`ffs-search` on crates.io](https://crates.io/crates/ffs-search)) | Native Rust SDK. |

---

## Build from source

```bash
git clone https://github.com/quangdang46/fast_file_search.git
cd fast_file_search
cargo build --release -p ffs-cli --features zlob
./target/release/ffs --version
```

`zlob` enables a Zig-compiled glob matcher; requires Zig at build time.
Without it, ffs falls back to `globset` (pure Rust). Drop `--features zlob`
if you don't have Zig installed.

The full workspace (`make build`) also produces:
- `target/release/ffs-mcp` — MCP server binary
- `target/release/libffs_c.{so,dylib,dll}` — C FFI library
- `target/release/libffs_nvim.{so,dylib,dll}` — Neovim cdylib

---

## Repository layout

```
crates/
  ffs-core/         # Rust core SDK (publishes as `ffs-search`)
  ffs-cli/          # the `ffs` binary
  ffs-mcp/          # MCP server (`ffs-mcp` binary)
  ffs-c/            # C FFI library (libffs_c, header in include/ffs.h)
  ffs-nvim/         # mlua bindings for the Neovim plugin
  ffs-engine/       # unified scanner + dispatch + ranking
  ffs-symbol/       # tree-sitter symbol index, bloom + bigram filters
  ffs-budget/       # token-budget reader, comment/whitespace filters
  ffs-grep/         # SIMD literal/regex grep
  ffs-query-parser/ # query language parser (constraints, fuzzy, regex modes)
lua/ffs/            # Neovim plugin code
plugin/ffs.lua      # Neovim auto-load
packages/
  ffs/              # @ffs-cli/ffs npm wrapper around the CLI binary
  ffs-node/         # @ffs-cli/ffs-node Node SDK
  ffs-bun/          # @ffs-cli/ffs-bun Bun SDK
  pi-ffs/           # @ffs-cli/pi-ffs Pi extension
  ffs-bin-*/        # @ffs-cli/ffs-bin-* prebuilt native libs (per-platform)
install.sh          # CLI installer (this README's curl|bash target)
install-mcp.sh      # MCP server installer
.github/workflows/
  release.yaml      # cross-compile + GitHub Releases on push to main and v* tags
  rust.yml          # fmt + clippy + test on every PR
  …
```

---

## Contributing

PRs welcome. Run `make check` before submitting:
- `make format` (rustfmt + stylua + biome)
- `make lint` (clippy + luacheck + biome)
- `make test`

Agentic coding tools are welcome to be used; human review is mandatory.

## License

[MIT](./LICENSE) — open source forever.
