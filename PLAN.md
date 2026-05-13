# PLAN.md — `fff` + `scry` → `ffs` (Fast File Search) Unification

## Context

This project is dual-identity: **fff.nvim** (the Neovim file picker) and **scry** (the
unified code-search CLI/MCP layer built on top). The codebase currently lives in
`crates/fff-*` and `lua/fff/` but the CLI binary, MCP server, and agent-facing layer
are already named `scry`.

This plan renames **everything** to `ffs` (Fast File Search) — the single unified brand.

---

## Naming Reference

| Old | New | Type |
|---|---|---|
| `fff.nvim` | `ffs.nvim` | Product name |
| `scry` (CLI) | `ffs` | CLI binary |
| `scry` (MCP) | `ffs` | MCP server |
| `fff-search` (package) | `ffs-search` | Rust crate package |
| `fff-*` (crates) | `ffs-*` | Crate directories |
| `scry-*` (crates) | `ffs-*` | Crate directories |
| `lua/fff/` | `lua/ffs/` | Lua module directory |
| `@ff-labs/fff-*` | `@ff-labs/ffs-*` | NPM packages |

---

## What Can Change vs. What Cannot Change

### CANNOT CHANGE (ABI / API contracts — break nothing)

| Layer | What | Why |
|---|---|---|
| **C FFI** | All `fff_*` function symbols (`fff_create_instance`, `fff_search`, etc.) in `fff.h` / `libfff_c` | Public ABI used by Bun, Node.js, Python, Ruby consumers |
| **C header** | `fff.h`, `FFF_C_H` guard, all `Fff*` C types (`FffResult`, `FffSearchResult`, etc.) | Same — byte-for-byte contract |
| **Neovim Lua require paths** | `require('fff')` entry point, all `require('fff.*')` calls | Plugin users reference these directly in configs |
| **Neovim user commands** | `:FFFFind`, `:FFFScan`, `:FFFRefreshGit`, `:FFFClearCache`, `:FFFHealth`, `:FFFDebug`, `:FFFOpenLog` | User keybindings rely on these |
| **Neovim global variables** | `vim.g.fff` config, `vim.g.fff_loaded`, `vim.g.fff_file_tracking` | Persist across sessions, documented in user configs |
| **Neovim highlight groups** | All `FFF*` highlights (`FFFSelected`, `FFFGitModified`, etc.) | User configs reference these directly |
| **Rust external crate deps** | `fff = { package = "fff-search", ... }` (the `package = "fff-search"` alias) | Changing this breaks all `use fff::*` imports |

### CAN CHANGE (everything else is fair game)

- Crate directory names (`crates/fff-core/` → `crates/ffs-core/`, etc.)
- Cargo.toml `[package]` name fields
- Rust internal module names
- Rust internal struct/enum/function names (`FffFileItem` → `FfsFileItem`)
- Lua directory name (`lua/fff/` → `lua/ffs/`) — but `lua/fff.lua` stays as compat shim
- NPM package names (`@ff-labs/fff-*` → `@ff-labs/ffs-*`)
- GitHub repository URL (`dmtrKovalenko/fff.nvim` → `dmtrKovalenko/ffs.nvim`)
- CI artifact names (`libfff_c.so` → `libffs_c.so`)
- Makefile targets and install paths
- Feature flag names (the `scry` feature stays — it's internal)
- Internal workspace dependencies
- `_typos.toml` `defaultdict-case` entries
- `AGENTS.md`, `README.md`, documentation

### PARTIALLY CONSTRAINED

- **Rust crate aliases in Cargo.toml**: `fff = { package = "fff-search", ... }`.
  The **package name** (`fff-search`) is the ABI name for `use fff::*` imports.
  To rename crate to `ffs-search`, update the alias: `ffs = { package = "ffs-search", ... }`.
  Then fix all `use fff::` → `use ffs::` internally.

---

## Scope Summary

| Category | Count | Notes |
|---|---|---|
| Crate directory renames | 10 dirs | `fff-core`, `fff-grep`, `fff-query-parser`, `fff-c`, `fff-nvim`, `fff-mcp`, `fff-engine`, `fff-symbol`, `fff-budget`, `fff-cli` |
| Total `fff` refs in `.rs` files | ~746 | Internal renames |
| Total `fff/scry` refs in config/doc | ~1,178 | CI, Makefile, npm, GitHub URLs |
| C FFI `fff_*` functions | ~50 | CANNOT RENAME — ABI |
| C types `Fff*` | ~15 | CANNOT RENAME — ABI |
| Lua `require('fff.*')` paths | 20+ modules | CANNOT RENAME — user contract |
| NPM packages | 6 | `fff-node`, `fff-bun`, `fff-bin-*` |
| Rust crate package names | 10 | `fff-search`, `fff-grep`, etc. |

---

## Phase 1 — Workspace Setup

**Goal**: Rename crate directories and update `Cargo.toml` workspace root before touching any source.

### 1.1 Rename crate directories

```bash
mv crates/fff-core        crates/ffs-core
mv crates/fff-c           crates/ffs-c
mv crates/fff-cli         crates/ffs-cli
mv crates/fff-grep        crates/ffs-grep
mv crates/fff-query-parser crates/ffs-query-parser
mv crates/fff-engine      crates/ffs-engine
mv crates/fff-nvim         crates/ffs-nvim
mv crates/fff-mcp          crates/ffs-mcp
mv crates/fff-symbol       crates/ffs-symbol
mv crates/fff-budget       crates/ffs-budget
```

### 1.2 Update root `Cargo.toml`

```toml
[workspace]
members = [
  "crates/ffs-core",
  "crates/ffs-c",
  "crates/ffs-cli",
  "crates/ffs-grep",
  "crates/ffs-query-parser",
  "crates/ffs-engine",
  "crates/ffs-nvim",
  "crates/ffs-mcp",
  "crates/ffs-symbol",
  "crates/ffs-budget",
]

[workspace.dependencies]
ffs-grep = { version = "0.7.2", path = "crates/ffs-grep" }
ffs-query-parser = { version = "0.7.2", path = "crates/ffs-query-parser", default-features = false }
ffs-symbol = { version = "0.1.0", path = "crates/ffs-symbol" }
ffs-budget = { version = "0.1.0", path = "crates/ffs-budget" }
ffs-engine = { version = "0.1.0", path = "crates/ffs-engine" }
```

Also update `notify-debouncer-full` package override (currently `fff-notify-debouncer-full`).

### 1.3 Update each crate's `Cargo.toml` `[package]` name

| Old crate dir | New crate dir | Old `[package].name` | New `[package].name` |
|---|---|---|---|
| `fff-core` | `ffs-core` | `fff-search` | `ffs-search` |
| `fff-c` | `ffs-c` | `fff-c` | `ffs-c` |
| `fff-cli` | `ffs-cli` | `fff-cli` | `ffs-cli` |
| `fff-grep` | `ffs-grep` | `fff-grep` | `ffs-grep` |
| `fff-query-parser` | `ffs-query-parser` | `fff-query-parser` | `ffs-query-parser` |
| `fff-engine` | `ffs-engine` | `fff-engine` | `ffs-engine` |
| `fff-nvim` | `ffs-nvim` | `fff-nvim` | `ffs-nvim` |
| `fff-mcp` | `ffs-mcp` | `fff-mcp` | `ffs-mcp` |
| `fff-symbol` | `ffs-symbol` | `fff-symbol` | `ffs-symbol` |
| `fff-budget` | `ffs-budget` | `fff-budget` | `ffs-budget` |

### 1.4 Update all internal crate dependency aliases

In each dependent crate's `Cargo.toml`, change:

```toml
# OLD
fff = { package = "fff-search", path = "../fff-core", ... }
fff-query-parser = { path = "../fff-query-parser", ... }
fff-engine = { workspace = true }
fff-symbol = { workspace = true }
fff-budget = { workspace = true }
fff-grep = { workspace = true }

# NEW
ffs = { package = "ffs-search", path = "../ffs-core", ... }
ffs-query-parser = { path = "../ffs-query-parser", ... }
ffs-engine = { workspace = true }
ffs-symbol = { workspace = true }
ffs-budget = { workspace = true }
ffs-grep = { workspace = true }
```

Affected crates: `ffs-core`, `ffs-c`, `ffs-cli`, `ffs-nvim`, `ffs-mcp`, `ffs-engine`.

### 1.5 Verify build

```bash
cargo build --release -p ffs-c --features zlob
cargo build --release -p ffs-nvim
```

All internal `use fff::*` imports will break — that's expected. Phase 2 fixes them.

---

## Phase 2 — Rust Internal Renames

**Goal**: Fix all internal `use fff::*` imports and rename internal Rust symbols.

### 2.1 Fix all `use` imports across all crates

Pattern replacements (apply across all `.rs` files in `crates/ffs-*`):

| Old | New |
|---|---|
| `use fff_search::` | `use ffs_search::` |
| `use fff_core::` | `use ffs_core::` |
| `use fff_grep::` | `use ffs_grep::` |
| `use fff_query_parser::` | `use ffs_query_parser::` |
| `use fff_engine::` | `use ffs_engine::` |
| `use fff_symbol::` | `use ffs_symbol::` |
| `use fff_budget::` | `use ffs_budget::` |
| `use fff_mcp::` | `use ffs_mcp::` |

Also update all `mod` declarations that reference old crate paths.

### 2.2 Rename internal Rust types and structs

| Old | New | Notes |
|---|---|---|
| `FffFileItem` | `FfsFileItem` | Internal only — NOT the C FFI type |
| `FffSearchResult` | `FfsSearchResult` | Internal only |
| `FffGrepResult` | `FfsGrepResult` | Internal only |
| `FffGrepMatch` | `FfsGrepMatch` | Internal only |
| `FffScore` | `FfsScore` | Internal only |
| `FffScanProgress` | `FfsScanProgress` | Internal only |
| `FILE_PICKER` (global) | `FILE_PICKER` | Keep — global state |
| `FRECENCY` (global) | `FRECENCY` | Keep — global state |
| `FilePicker` | `FilePicker` | Keep — core type |
| `FrecencyDb` | `FrecencyDb` | Keep — database type |

**IMPORTANT**: Do NOT rename:
- `Fff*` C FFI types in `ffs-c/src/ffi_types.rs` (those map to `fff.h`)
- Any `#[repr(C)]` structs that correspond to `fff.h` types

### 2.3 C FFI — rename internal holder structs

| Old | New | Notes |
|---|---|---|
| `FffScryEngine` | `FfsEngine` | Internal Rust holder — rename |
| `FffScryResponse` | `FfsResponse` | Internal Rust holder — rename |
| `fff_scry_engine_new` | keep | C ABI — do not rename |
| `fff_scry_*` functions | keep | C ABI — do not rename |

### 2.4 Verify build

```bash
cargo build --release -p ffs-nvim
make build
```

---

## Phase 3 — Lua Migration

**Goal**: Rename `lua/fff/` → `lua/ffs/` with a compatibility shim at `lua/fff.lua`.

### 3.1 Strategy: Move implementation, keep shim

```
lua/fff.lua          ← KEEP (thin compat shim, re-exports to ffs.main)
lua/fff/             ← KEEP as compat shim directory (re-export all modules)
lua/ffs/             ← NEW implementation home (was lua/fff/)
  main.lua
  core.lua
  conf.lua
  picker_ui.lua
  scry.lua          ← wraps Rust ffs_* exports
  rust/init.lua      ← loads libfff_nvim.so (keep DLL name for now)
  ...
```

### 3.2 `lua/fff.lua` compat shim

```lua
-- lua/fff.lua (unchanged — public contract)
return require('fff.main')
```

All `lua/fff/*.lua` files become thin re-exports from `lua/ffs/*.lua`.

### 3.3 Database paths in `lua/ffs/conf.lua`

```lua
-- OLD
db_path = vim.fn.stdpath('cache') .. '/fff_nvim',
db_path = vim.fn.stdpath('data') .. '/fff_queries',
log_file = vim.fn.stdpath('log') .. '/fff.log',

-- NEW
db_path = vim.fn.stdpath('cache') .. '/ffs_nvim',
db_path = vim.fn.stdpath('data') .. '/ffs_queries',
log_file = vim.fn.stdpath('log') .. '/ffs.log',
```

Frecency/history paths in `lua/ffs/core.lua`:
```lua
-- OLD
'/fff_frecency', '/fff_history'
-- NEW
'/ffs_frecency', '/ffs_history'
```

### 3.4 `lua/fff/rust/init.lua` → `lua/ffs/rust/init.lua`

The DLL loader loads `libfff_nvim.so`. **Keep this name unchanged** — it is
the compiled artifact filename, not the API. Users don't reference it directly.
Rename in a future step if desired.

### 3.5 `plugin/fff.lua` → `plugin/ffs.lua`

Rename the plugin file. The internal `vim.g.fff_loaded` can become `vim.g.ffs_loaded`
or stay as-is (it's internal). The user commands (`FFFFind`, etc.) and
`vim.g.fff` config stay unchanged.

### 3.6 Neovim user commands — keep all `FFF*` names

`:FFFFind`, `:FFFScan`, `:FFFRefreshGit`, `:FFFClearCache`, `:FFFHealth`,
`:FFFDebug`, `:FFFOpenLog` — all **cannot** be renamed (user keybindings).

### 3.7 Neovim highlight groups — keep all `FFF*` names

`FFFSelected`, `FFFGitStaged`, etc. — **cannot** be renamed (user configs).

### 3.8 `lua/fff/scry.lua` → `lua/ffs/scry.lua`

This file wraps Rust `scry_*` FFI functions. It stays as `require('fff.scry')`
for the compat shim, but the implementation moves to `lua/ffs/scry.lua`.

### 3.9 `lua/fff/health.lua`

```lua
-- Keep
vim.health.start('fff.nvim')
-- Also add
vim.health.start('ffs.nvim')
```

---

## Phase 4 — C FFI / Header

### 4.1 Rename header file

```
crates/ffs-c/include/fff.h → crates/ffs-c/include/ffs.h
```

Update `cbindgen.toml`:
```toml
[package]
name = "ffs-c"
header = "include/ffs.h"
# guard stays "FFF_C_H" — changing it breaks ABI compat
```

Update `Makefile`:
```makefile
cbindgen --config crates/ffs-c/cbindgen.toml --crate ffs-c --output crates/ffs-c/include/ffs.h
install -m 0644 crates/ffs-c/include/ffs.h $(DESTDIR)$(INCLUDEDIR)/ffs.h
install -m 0644 crates/ffs-c/include/ffs.h $(DESTDIR)$(INCLUDEDIR)/fff.h  # backward compat alias
```

### 4.2 Library artifact names in Makefile

```makefile
# OLD
install -m 0755 target/release/libfff_c.dylib ...
install -m 0755 target/release/libfff_c.so ...
install -m 0755 target/release/fff_c.dll ...

# NEW
install -m 0755 target/release/libffs_c.dylib ...
install -m 0755 target/release/libffs_c.so ...
install -m 0755 target/release/ffs_c.dll ...
```

---

## Phase 5 — CLI Binary & MCP Server

### 5.1 CLI binary name

In `crates/ffs-cli/Cargo.toml`:
```toml
[[bin]]
name = "ffs"   # was "scry"
path = "src/main.rs"
```

Update `crates/ffs-cli/src/main.rs`:
```rust
.about("Fast File Search (FFS) — unified code search and read tool.")
.long_version(env!("CARGO_PKG_VERSION"))
```

All CLI subcommands already have user-facing names (`find`, `grep`, `symbol`, `callers`,
`refs`, `flow`, etc.) — no change needed there.

Update `crates/ffs-cli/assets/AGENT_GUIDE.md` header:
```markdown
# ffs — agent guide
```

### 5.2 MCP server

In `crates/ffs-mcp/src/main.rs`, update module name and binary name:
```toml
[[bin]]
name = "ffs-mcp"   # was "fff-mcp"
```

Update `crates/ffs-cli/src/commands/mcp.rs`:
```json
{"name": "ffs", "version": env!("CARGO_PKG_VERSION")}
```

Rename tool names: `scry_*` → `ffs_*` (the `scry` feature flag stays, but the tool names
exposed to MCP clients change):

| Old | New |
|---|---|
| `scry_grep` | `ffs_grep` |
| `scry_glob` | `ffs_glob` |
| `scry_find` | `ffs_find` |
| `scry_read` | `ffs_read` |
| `scry_symbol` | `ffs_symbol` |
| `scry_dispatch` | `ffs_dispatch` |
| `scry_refs` | `ffs_refs` |
| `scry_flow` | `ffs_flow` |
| `scry_impact` | `ffs_impact` |

### 5.3 Rust internal scry module → ffs

In `crates/ffs-mcp/src/`:
- `mod scry;` → `mod ffs;`
- `scry.rs` → `ffs.rs`
- `crate::scry::` → `crate::ffs::`

In `crates/ffs-c/src/`:
- `mod scry_ffi;` stays (C ABI — keep name)
- `crate::scry_ffi::` → internal renaming as needed

In `crates/ffs-nvim/src/`:
- `mod scry_bindings;` → `mod ffs_bindings;`
- All `scry_init`, `scry_dispatch`, etc. stay as function names (Lua ABI)
- But the module reference in `lib.rs` changes

---

## Phase 6 — CI / Release Artifacts

### 6.1 GitHub Actions

**`.github/workflows/release.yaml`** — rename all artifact patterns:
```
libfff_nvim.so  →  libffs_nvim.so
libfff_nvim.dylib → libffs_nvim.dylib
fff_nvim.dll    →  ffs_nvim.dll
```

**`.github/workflows/external-tests.yml`**:
```
fff_nvim.dll  →  ffs_nvim.dll
```

**`.github/workflows/panvimdoc.yaml`**:
```
vimdoc: ffs.nvim
```

**`.github/workflows/nix.yml`**:
```
nix build .#fff-nvim  →  nix build .#ffs-nvim
```

Update `flake.nix` output attribute names accordingly.

### 6.2 NPM packages

| Old | New |
|---|---|
| `@ff-labs/fff-bin-darwin-arm64` | `@ff-labs/ffs-bin-darwin-arm64` |
| `@ff-labs/fff-bin-darwin-x64` | `@ff-labs/ffs-bin-darwin-x64` |
| `@ff-labs/fff-bin-linux-arm64-gnu` | `@ff-labs/ffs-bin-linux-arm64-gnu` |
| `@ff-labs/fff-bin-linux-x64-gnu` | `@ff-labs/ffs-bin-linux-x64-gnu` |
| `@ff-labs/fff-bin-linux-x64-musl` | `@ff-labs/ffs-bin-linux-x64-musl` |
| `@ff-labs/fff-bin-win32-arm64` | `@ff-labs/ffs-bin-win32-arm64` |
| `@ff-labs/fff-node` | `@ff-labs/ffs-node` |
| `@ff-labs/fff-bun` | `@ff-labs/ffs-bun` |
| `@ff-labs/pi-fff` | `@ff-labs/pi-ffs` |

Update GitHub repo URLs in all `package.json` fields:
```json
"url": "git+https://github.com/dmtrKovalenko/ffs.nvim.git",
"repository": "https://github.com/dmtrKovalenko/ffs.nvim",
"issues": "https://github.com/dmtrKovalenko/ffs.nvim/issues"
```

### 6.3 `install-mcp.sh` / `install-mcp.ps1`

Update repo URL: `dmtrKovalenko/fff.nvim` → `dmtrKovalenko/ffs.nvim`

### 6.4 Binary/npm references in Lua

In `lua/ffs/download.lua`:
```lua
GITHUB_REPO = 'dmtrKovalenko/ffs.nvim'
```

In `lua/ffs/rust/init.lua` — keep `libfff_nvim` DLL name (artifact rename is Phase 6).

---

## Phase 7 — Documentation & Metadata

### 7.1 `README.md`

- Project name: `fff.nvim` → `ffs.nvim`
- Description updates
- Logo references (`logo-dark.png` → keep filenames, update in docs)
- GitHub URLs
- Install instructions
- Keep `FFF_FIND` / `:FFFFind` command docs

### 7.2 `AGENTS.md` (symlinked as `CLAUDE.md`)

- Project description: "FFS.nvim (Fast File Search)" not "FFF.nvim"
- All `fff-*` file paths → `ffs-*`
- All `lua/fff/` paths → `lua/ffs/`
- Keep architecture description accurate

### 7.3 `_typos.toml`

```toml
[defaultdict-case]
"fff" = "ffs"
"scry" = "ffs"
```

### 7.4 `.luacheckrc`

Check for `fff`-specific entries to update.

### 7.5 `doc/fff.nvim.txt` → `doc/ffs.nvim.txt`

### 7.6 Package `main` fields

In npm packages with `libfff_c.*` main files, rename to `libffs_c.*`.

---

## Phase 8 — Verification

### 8.1 Build

```bash
make build
cargo test --workspace
make lint
make format
```

### 8.2 Lua smoke test

```bash
nvim -u empty_config.lua -l tests/fff_core_spec.lua
nvim -u empty_config.lua -l tests/clear_cache_spec.lua
```

### 8.3 CLI smoke test

```bash
cargo run --release -p ffs-cli -- --help
cargo run --release -p ffs-cli -- find main
cargo run --release -p ffs-cli -- guide
```

### 8.4 MCP server smoke test

```bash
cargo run --release -p ffs-mcp -- --root .
```

### 8.5 Neovim plugin test

```bash
nvim -u ~/dev/lightsource/init.lua
:lua require('fff').find_files()
```

---

## File-Level Change Inventory

### Crate directories (10 renames)

```
crates/fff-core/         → crates/ffs-core/
crates/fff-c/            → crates/ffs-c/
crates/fff-cli/          → crates/ffs-cli/
crates/fff-grep/         → crates/ffs-grep/
crates/fff-query-parser/ → crates/ffs-query-parser/
crates/fff-engine/       → crates/ffs-engine/
crates/fff-nvim/         → crates/ffs-nvim/
crates/fff-mcp/          → crates/ffs-mcp/
crates/fff-symbol/       → crates/ffs-symbol/
crates/fff-budget/        → crates/ffs-budget/
```

### Lua directories

```
lua/fff/   → lua/fff/ (shim, re-exports to ffs/)
lua/ffs/   → new implementation home
```

### Plugin file

```
plugin/fff.lua → plugin/ffs.lua
```

### C header

```
crates/ffs-c/include/fff.h → crates/ffs-c/include/ffs.h
```

### Key file groups needing targeted edits

| Group | Files | Action |
|---|---|---|
| Crate Cargo.tomls (10) | `crates/*/Cargo.toml` | Package name + internal deps |
| Rust source (155 files) | `crates/*/src/**/*.rs` | Fix `use fff_*::` imports + internal type renames |
| Lua impl | `lua/fff/*.lua` (moved to `lua/ffs/`) | Fix `require('fff.*')` → `require('ffs.*')`, update db paths |
| Lua compat shims | `lua/fff/*.lua` (re-exports) | Re-export from `lua/ffs/*` |
| Plugin | `plugin/fff.lua` (renamed) | Update internal references |
| C FFI | `crates/ffs-c/src/ffi_types.rs`, `accessors.rs` | Rename internal holders only |
| CI workflows (4 files) | `.github/workflows/*.yml` | Rename artifact patterns, nix output |
| NPM packages (6) | `packages/*/package.json` | Rename packages + repo URLs |
| Makefile | `Makefile` | Rename targets, install paths, cbindgen output |
| Docs | `README.md`, `AGENTS.md`, `doc/fff.nvim.txt` | Full rebrand |
| Config | `_typos.toml`, `.luacheckrc` | Update entries |
| Installers | `install-mcp.sh`, `install-mcp.ps1` | Update repo URL |
| Nix | `flake.nix` | Rename output attributes |

---

## Execution Order

```
Phase 1 (Workspace/dirs)  ──►  Phase 2 (Rust)  ──►  Phase 3 (Lua)  ──►
Phase 4 (C FFI)          ──►  Phase 5 (CLI/MCP) ──►  Phase 6 (CI/npm)  ──►
Phase 7 (Docs)           ──►  Phase 8 (Verify)
```

Each phase is independent and verifiable before proceeding to the next.
