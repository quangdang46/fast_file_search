<img alt="ffs" src="./assets/logo-orange.png" width="300">

<p>
  <i>A code-aware file search toolkit for humans and AI agents. Really fast.</i>
</p>

Typo-resistant path and content search, frecency-ranked file access, a background watcher, a tree-sitter symbol index, bloom + bigram pre-filters, and a token-budget aware reader. One long-lived process, one walk over the repo, every consumer (CLI, MCP, Neovim, Node, Bun, C) reading from the same warm caches.

Way faster than CLIs like ripgrep and fzf in any long-running process that searches more than once. Originally a [Neovim plugin](#neovim-plugin) people loved; the same Rust core now ships through every surface listed below.

---

Pick what you are interested in:

<details id="cli">
<summary>
<h2>CLI (<code>ffs</code>)</h2>
</summary>

`ffs` is a single binary that replaces `find`, `fd`, `grep`, `rg`, `glob`, `cat`, and a handful of code-navigation tools, behind a unified subcommand surface.

### Sub-commands

```
ffs find        Find files by name (replaces `find`, `fd`).
ffs glob        Match files by glob (replaces `glob`, shell `**`).
ffs grep        Search file contents (replaces `grep`, `rg`).
ffs read        Read a file with token-budget aware truncation (replaces `cat`).
ffs outline     Render a file's structural outline (functions, classes, …).
ffs symbol      Look up symbol definitions (tree-sitter powered).
ffs callers     List call sites of a symbol.
ffs callees     List symbols referenced inside a symbol body.
ffs refs        Definitions + single-hop usages of a symbol in one shot.
ffs flow        Drill-down envelope per definition (def + body + callees + callers).
ffs siblings    Sibling symbols (peers in the same parent scope).
ffs deps        File imports + the workspace files that depend on it.
ffs impact      Rank files by how much they'd be affected if a symbol changed.
ffs dispatch    Auto-classify a free-form query and route it to the right backend.
ffs index       Build / refresh the on-disk indexes (Bigram, Bloom, Symbol, Outline).
ffs map         Render the workspace as a tree annotated with file count and tokens.
ffs overview    High-signal summary of the workspace (languages, top symbols, …).
ffs mcp         Run as an MCP server over stdio.
ffs guide       Print the embedded agent guide.
```

Output defaults to plain text; pass `--format json` for machine-readable output. `--root <path>` overrides the working directory globally.

### Architecture

```
                ┌─────────────────────────────────────────────────────┐
                │  Consumers                                          │
                │  ──────────────────────────────────────────────     │
                │  ffs CLI │ ffs-mcp │ ffs.nvim │ @ff-labs/ffs-node    │
                │ (ffs-cli)  (ffs-mcp)  (ffs-nvim)  / ffs-bun / -c     │
                └───────────────────────┬─────────────────────────────┘
                                        │
                                        ▼
                ┌─────────────────────────────────────────────────────┐
                │  ffs-engine — Engine                                │
                │    OnceCell<Arc<Engine>> + Mutex<()> warm caches    │
                │    dispatch · classify · ranking · prefilter        │
                └─────────┬───────────────────┬───────────────┬───────┘
                          │                   │               │
                          ▼                   ▼               ▼
                ┌──────────────┐    ┌────────────────┐    ┌──────────┐
                │ ffs-symbol   │    │ ffs-budget     │    │ ffs-core │
                │  scanner     │    │  tokens, filter│    │  (fuzzy/ │
                │  bigram +    │    │  truncate w/   │    │  grep/   │
                │  bloom cache │    │  preserved     │    │  walker) │
                │  outline     │    │  header+footer │    │          │
                └──────────────┘    └────────────────┘    └──────────┘
                                ▲
                                │ single-pass UnifiedScanner walks
                                │ the repo once, populates all caches
```

Read top to bottom: every consumer calls into the same `Engine`. `UnifiedScanner` runs once at index time and populates the symbol index, the bloom + bigram caches, and the outline cache that all four lookup paths read from.

### What the index actually contains

A single pass over the repo (`ffs index`) builds three caches at once:

- **Symbol index** — tree-sitter AST scan of every supported file, recording every definition with `kind` (function, struct, class, …), `path`, and `line`.
- **Bloom filter cache** — per-file 64-bit bloom of every identifier in the file. First stage of a 2-stage `PreFilterStack` (bigram → bloom → literal-text confirm) so symbol/caller/callee queries don't have to walk every file.
- **Outline cache** — comment/header/footer-aware byte slice for every file, used by `ffs read --budget` and the agent-facing token-budget reader.

Once the index is warm, every subcommand reuses it. There is no second walk.

### Query path

The pre-filter stack is what makes `ffs symbol`, `ffs callers`, and `ffs callees` cheap on large repos:

```
   query: "UnifiedScanner"
        │
        ▼
   ┌─────────────────────────────────────────────┐
   │ Stage 1 — Bigram filter                     │
   │   skip files whose 2-gram set does not      │
   │   include every bigram of the query         │
   └────────────────────┬────────────────────────┘
                        ▼
   ┌─────────────────────────────────────────────┐
   │ Stage 2 — Bloom filter (per file)           │
   │   skip files whose 64-bit bloom does not    │
   │   carry the identifier hash                 │
   └────────────────────┬────────────────────────┘
                        ▼
   ┌─────────────────────────────────────────────┐
   │ Stage 3 — Confirm                           │
   │   exact `String::contains` on surviving     │
   │   files, lookup line in symbol index        │
   └────────────────────┬────────────────────────┘
                        ▼
   ┌─────────────────────────────────────────────┐
   │ Ranking                                     │
   │   definition > usage, comment match         │
   │   demoted, kind-aware tie-break             │
   └─────────────────────────────────────────────┘
```

Stages 1 and 2 are pure metadata lookups; the expensive `String::contains` only ever runs on the survivors. On this repo (3.3k symbols across 229 files) a typical `ffs callers` query inspects fewer than 30 files.

### Token-budgeted read

The killer feature for AI harnesses. `ffs read path --budget 5000 --filter minimal`:

1. Converts the budget: `tokens × 0.85 × 4 bytes/token` ≈ `17_000` bytes.
2. Loads the file, applies the requested `--filter` level (`none`, `minimal`, `aggressive`) to drop comments / whitespace while preserving doc-comments and the file header.
3. Truncates from the body, **always preserving** the first ~5 lines (header) and a `[truncated to budget]\n` footer so the agent knows the output was clipped.

The classifier and apply-preserving-footer logic live in [`crates/ffs-budget/`](./crates/ffs-budget/); the symbol scanner is in [`crates/ffs-symbol/`](./crates/ffs-symbol/); the unified `Engine` that ties them together is in [`crates/ffs-engine/`](./crates/ffs-engine/).

### Build and run

```bash
make build                      # builds ffs, ffs-mcp, ffs_nvim, libffs_c
./target/release/ffs index      # one-time warm-up, ~200 ms on a 10k-file repo
./target/release/ffs symbol UnifiedScanner
./target/release/ffs callers UnifiedScanner
./target/release/ffs read crates/ffs-engine/src/lib.rs --budget 5000 --filter minimal
./target/release/ffs dispatch 'where is the user controller'
./target/release/ffs mcp        # JSON-RPC over stdio
```

Source: [`crates/ffs-cli/`](./crates/ffs-cli/), [`crates/ffs-engine/`](./crates/ffs-engine/), [`crates/ffs-symbol/`](./crates/ffs-symbol/), [`crates/ffs-budget/`](./crates/ffs-budget/).

</details>

The single `ffs` binary is the primary entry point. Everything else is a thin wrapper around the same core.

<details id="mcp-server">
<summary>
<h2>MCP server</h2>
</summary>

Works with Claude Code, Codex, OpenCode, Cursor, Cline, and any MCP-capable client. Fewer grep roundtrips, less wasted context, faster answers.

![Benchmark chart comparing ffs against the built-in AI file-search tools](./chart.png)

### One-line install

Linux / macOS:

```bash
curl -L https://dmtrkovalenko.dev/install-ffs-mcp.sh | bash
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/dmtrKovalenko/ffs.nvim/main/install-mcp.ps1 | iex
```

The scripts live at [`install-mcp.sh`](./install-mcp.sh) and [`install-mcp.ps1`](./install-mcp.ps1) if you want to read them first.

The installer prints the exact wiring instructions for your client. Or run the same server from a checkout with `ffs mcp`.

### Recommended agent prompt

Drop this into your project's `CLAUDE.md` or equivalent:

```markdown
For any file search, grep, or symbol lookup in the current git-indexed
directory, use ffs tools.
```

### Tools registered

| Tool            | What it answers                                                                                                                            |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `find_files`    | Fuzzy file-name search. Smart-case, frecency-ranked, glob constraints (`src/**/*.{ts,tsx} !test/`), git-aware.                              |
| `grep`          | Content search. Plain / regex / fuzzy auto-detect, pagination cursor, `classifyDefinitions` flag for definition-first hinting.              |
| `multi_grep`    | OR-logic multi-pattern content search via SIMD Aho-Corasick. Faster than regex alternation for literal text.                                |
| `ffs_dispatch`  | Auto-classify a free-form query (path, glob, identifier, concept phrase) and route it through the engine.                                   |
| `ffs_symbol`    | Exact + prefix lookup over the tree-sitter symbol index (16 languages).                                                                     |
| `ffs_callers`   | Find call sites of a symbol. Bloom-filter narrowed candidates → literal-text confirm pass.                                                  |
| `ffs_callees`   | Symbols that the body of a definition references.                                                                                           |
| `ffs_read`      | Token-budget aware file read. Maps `maxTokens` to `~85% body × 4 bytes/token` and applies a comment/whitespace filter when requested.       |

### What changes vs. built-in agent search

- Frecency memory. Files you actually open rank higher next time. Warm-up from git touch history runs automatically.
- Definition-first hinting. Lines that look like code definitions are classified on the Rust side, no regex overhead in your prompt.
- Smart-case with auto-fuzzy fallback. `IsOffTheRecord` finds snake_case variants; zero-match queries retry as fuzzy and surface the best approximate hits.
- Git-aware annotations. Modified, untracked, and staged files are tagged so the agent reaches for what you are actively changing.
- Code-aware tools. `ffs_symbol` / `ffs_callers` / `ffs_callees` let an agent ask code questions instead of just file-name questions.

The first call to any `ffs_*` tool lazily builds the engine (`Engine::new` + `engine.index()`) under a `OnceCell<Arc<Engine>>` + double-checked `Mutex<()>`; every subsequent call reuses the warm caches.

Source: [`crates/ffs-mcp/`](./crates/ffs-mcp/).

</details>

The MCP server gives any agent a file-search and code-navigation surface that is faster and more token-efficient than the built-in one.

<details id="pi-extension">
<summary>
<h2>Pi agent extension</h2>
</summary>

### Install

```bash
pi install npm:@ff-labs/pi-ffs
```

### Modes

Three operating modes, switchable at runtime with `/ffs-mode`:

| Mode                     | What it does                                                                      |
| ------------------------ | --------------------------------------------------------------------------------- |
| `tools-and-ui` (default) | Adds `ffsgrep` and `ffsfind` tools, replaces `@`-mention autocomplete with ffs.     |
| `tools-only`             | Only tool injection. Keeps pi's native editor autocomplete.                       |
| `override`               | Replaces pi's built-in `grep`, `find`, and `multi_grep` with ffs implementations. |

Env vars: `PI_FFS_MODE`, `FFS_FRECENCY_DB`, `FFS_HISTORY_DB`. Flags: `--ffs-mode`, `--ffs-frecency-db`, `--ffs-history-db`.

### Agent-facing tools

- `ffsgrep`. Content search. Accepts `path`, `exclude` (comma, space, or array; leading `!` optional), `caseSensitive`, `context`, and cursor pagination. Auto-detects regex, falls back to fuzzy on zero exact matches, rejects `.*`-style wildcard-only patterns up front.
- `ffsfind`. Path and filename search. Matches the whole repo-relative path, not just the filename. Frecency-aware. The weak-match detector flags scattered fuzzy noise before it floods the agent's context.
- `ffs-multi-grep`. OR-logic multi-pattern content search via Aho-Corasick.

### Commands

- `/ffs-mode [tools-and-ui | tools-only | override]`. Show or switch the mode.
- `/ffs-health`. Picker, frecency, and git integration status.
- `/ffs-rescan`. Force a rescan.

Source: [`packages/pi-ffs/`](./packages/pi-ffs/).

</details>

The Pi extension swaps pi's native tools for ffs implementations and feeds the interactive editor's `@`-mention autocomplete from the frecency-ranked index.

<details id="neovim-plugin">
<summary>
<h2>Neovim plugin</h2>
</summary>

Demo on the Linux kernel repo (100k files, 8GB):

https://github.com/user-attachments/assets/5d0e1ce9-642c-4c44-aa88-01b05bb86abb

### Installation

#### lazy.nvim

```lua
{
  'dmtrKovalenko/ffs.nvim',
  build = function()
    -- downloads a prebuilt binary or falls back to cargo build
    require("ffs.download").download_or_build_binary()
  end,
  -- for nixos:
  -- build = "nix run .#release",
  opts = {
    debug = {
      enabled = true,
      show_scores = true,
    },
  },
  lazy = false, -- the plugin lazy-initialises itself
  keys = {
    { "ff", function() require('ffs').find_files() end, desc = 'Find files' },
    { "fg", function() require('ffs').live_grep() end, desc = 'Live grep' },
    { "fz",
      function() require('ffs').live_grep({ grep = { modes = { 'fuzzy', 'plain' } } }) end,
      desc = 'Live fuzzy grep',
    },
    { "fc",
      function() require('ffs').live_grep({ query = vim.fn.expand("<cword>") }) end,
      desc = 'Search current word',
    },
  },
}
```

#### vim.pack

```lua
vim.pack.add({ 'https://github.com/dmtrKovalenko/ffs.nvim' })

vim.api.nvim_create_autocmd('PackChanged', {
  callback = function(ev)
    local name, kind = ev.data.spec.name, ev.data.kind
    if name == 'ffs.nvim' and (kind == 'install' or kind == 'update') then
      if not ev.data.active then vim.cmd.packadd('ffs.nvim') end
      require('ffs.download').download_or_build_binary()
    end
  end,
})

vim.g.ffs = {
  lazy_sync = true,
  debug = { enabled = true, show_scores = true },
}

vim.keymap.set('n', 'ff', function() require('ffs').find_files() end, { desc = 'Find files' })
```

### Public API

```lua
require('ffs').find_files()                        -- find files in current repo
require('ffs').live_grep()                         -- live content grep
require('ffs').scan_files()                        -- force rescan
require('ffs').refresh_git_status()                -- refresh git status
require('ffs').find_files_in_dir(path)             -- find in a specific dir
require('ffs').change_indexing_directory(new_path) -- change root
```

### Commands

- `:FFSScan`. Rescan files.
- `:FFSRefreshGit`. Refresh git status.
- `:FFSClearCache [all|frecency|files]`. Clear caches.
- `:FFSHealth`. Health check.
- `:FFSDebug [on|off|toggle]`. Toggle the scoring display.
- `:FFSOpenLog`. Open `~/.local/state/nvim/log/ffs.log`.



### Configuration

Defaults are sensible. Override only what you care about.

```lua
require('ffs').setup({
  base_path = vim.fn.getcwd(),
  prompt = '> ',
  title = 'Files',
  max_results = 100,
  max_threads = 4,
  lazy_sync = true,
  prompt_vim_mode = false,
  layout = {
    height = 0.8,
    width = 0.8,
    prompt_position = 'bottom',   -- or 'top'
    preview_position = 'right',   -- 'left' | 'right' | 'top' | 'bottom'
    preview_size = 0.5,
    flex = { size = 130, wrap = 'top' },
    show_scrollbar = true,
    path_shorten_strategy = 'middle_number', -- 'middle_number' | 'middle' | 'end'
    anchor = 'center',
  },
  preview = {
    enabled = true,
    max_size = 10 * 1024 * 1024,
    chunk_size = 8192,
    binary_file_threshold = 1024,
    imagemagick_info_format_str = '%m: %wx%h, %[colorspace], %q-bit',
    line_numbers = false,
    cursorlineopt = 'both',
    wrap_lines = false,
    filetypes = {
      svg = { wrap_lines = true },
      markdown = { wrap_lines = true },
      text = { wrap_lines = true },
    },
  },
  keymaps = {
    close = '<Esc>',
    select = '<CR>',
    select_split = '<C-s>',
    select_vsplit = '<C-v>',
    select_tab = '<C-t>',
    move_up = { '<Up>', '<C-p>' },
    move_down = { '<Down>', '<C-n>' },
    preview_scroll_up = '<C-u>',
    preview_scroll_down = '<C-d>',
    toggle_debug = '<F2>',
    cycle_grep_modes = '<S-Tab>',
    cycle_previous_query = '<C-Up>',
    toggle_select = '<Tab>',
    send_to_quickfix = '<C-q>',
    focus_list = '<leader>l',
    focus_preview = '<leader>p',
  },
  frecency = {
    enabled = true,
    db_path = vim.fn.stdpath('cache') .. '/ffs_nvim',
  },
  history = {
    enabled = true,
    db_path = vim.fn.stdpath('data') .. '/ffs_queries',
    min_combo_count = 3,
    combo_boost_score_multiplier = 100,
  },
  git = {
    status_text_color = false, -- true to color filenames by git status
  },
  grep = {
    max_file_size = 10 * 1024 * 1024,
    max_matches_per_file = 100,
    smart_case = true,
    time_budget_ms = 150,
    modes = { 'plain', 'regex', 'fuzzy' },
    trim_whitespace = false,
  },
  debug = { enabled = false, show_scores = false },
  logging = {
    enabled = true,
    log_file = vim.fn.stdpath('log') .. '/ffs.log',
    log_level = 'info',
  },
})
```

### Live grep modes

`<S-Tab>` cycles between `plain`, `regex`, and `fuzzy`. The list is configurable via `grep.modes`, and single-mode setups hide the indicator entirely.

Per-call override:

```lua
require('ffs').live_grep({ grep = { modes = { 'fuzzy', 'plain' } } })
require('ffs').live_grep({ query = 'search term' }) -- pre-fill
```

### Constraints

Both find and grep accept these tokens to refine a query:

- `git:modified`. One of `modified`, `staged`, `deleted`, `renamed`, `untracked`, `ignored`.
- `test/`. Any deeply nested children of `test/`.
- `!something`, `!test/`, `!git:modified`. Exclusion.
- `./**/*.{rs,lua}`. Any valid glob, powered by [zlob](https://github.com/dmtrKovalenko/zlob).

Grep-only:

- `*.md`, `*.{c,h}`. Extension filter.
- `src/main.rs`. Grep inside a single file.

Mix freely: `git:modified src/**/*.rs !src/**/mod.rs user controller`.

### Multi-select and quickfix

- `<Tab>`. Toggle selection (shows a thick `▊` in the signcolumn).
- `<C-q>`. Send selected files to the quickfix list and close the picker.

### Git status highlighting

Sign-column indicators are on by default. To color filename text by git status, set `git.status_text_color = true` and adjust the `hl.git_*` groups. See `:help ffs.nvim` for the full list.

### File filtering

ffs honours `.gitignore`. For picker-only ignores that do not touch git, add a sibling `.ignore` file:

```gitignore
*.md
docs/archive/**/*.md
```

Run `:FFSScan` to force a rescan.

### Troubleshooting

- `:FFSHealth` verifies picker init, optional dependencies, and DB connectivity.
- `:FFSOpenLog` opens the log file.

### Code-aware Lua module (`ffs.engine`)

The Neovim cdylib (`ffs_nvim`) bundles the symbol index, dispatch, and budgeted read directly. The wrapper at [`lua/ffs/engine.lua`](./lua/ffs/engine.lua) gives you:

```lua
local engine = require('ffs.engine')
engine.init(vim.fn.getcwd())                                  -- one-time index
for _, hit in ipairs(engine.symbol('FilePicker')) do
  print(hit.path, hit.line, hit.kind)
end
local res = engine.read('lua/ffs/main.lua', 5000, 'minimal')  -- token-budget read
print(engine.dispatch('UnifiedScanner'))                      -- auto-classify
engine.rebuild()                                              -- refresh caches
```

Inputs are validated with `vim.validate()`.

The standard `require('ffs').find_files()` picker API is **unchanged** in either build mode.

</details>

The best file search picker for Neovim. Period. Faster and more intuitive queries, frecency ranking, definition classification, and a code-aware extension behind a single feature flag.

<details id="node-sdk">
<summary>
<h2>Node & Bun SDK</h2>
</summary>

```bash
npm install @ff-labs/ffs-node
# or
bun add @ff-labs/ffs-bun
```

```ts
import { FileFinder } from "@ff-labs/ffs-node";

const finder = FileFinder.create({ basePath: process.cwd(), aiMode: true });
if (!finder.ok) throw new Error(finder.error);
await finder.value.waitForScan(10_000);

const files = finder.value.fileSearch("incognito profile", { pageSize: 20 });
const hits = finder.value.grep("GetOffTheRecordProfile", {
  mode: "plain",
  smartCase: true,
  beforeContext: 1,
  afterContext: 1,
  classifyDefinitions: true,
});

finder.value.destroy();
```

Every method returns a `Result<T>` (`{ ok: true, value } | { ok: false, error }`). Full type reference: [`packages/ffs-node/src/types.ts`](./packages/ffs-node/src/types.ts).

</details>

TypeScript wrapper over the C library for Node.js and Bun. Build custom agent tools, CLIs, or IDE integrations on top of ffs.

<details id="rust-crate">
<summary>
<h2>Rust crate</h2>
</summary>

### Add the dependency

ffs is written in Rust, so this is the lowest-overhead way to use it.

```toml
[dependencies]
ffs-search = "0.7"
```

Full API documentation: [docs.rs/ffs-search](https://docs.rs/ffs-search).

</details>

Native Rust crate that powers all the search. Stable and well documented.

<details id="c-library">
<summary>
<h2>C library</h2>
</summary>

### Build

```bash
# Builds only the C cdylib (fastest):
make build-c-lib

# or directly with cargo:
cargo build --release -p ffs-c --features zlob
```

The output is a `cdylib` (`libffs_c.so` / `libffs_c.dylib` / `ffs_c.dll`). The header lives at [`crates/ffs-c/include/ffs.h`](./crates/ffs-c/include/ffs.h).

Prebuilt binaries for every version, including every commit on main, are on the [releases page](https://github.com/dmtrKovalenko/ffs.nvim/releases). The same binaries also ship inside the `@ff-labs/ffs-bin-*` npm packages.

### Install

```bash
# System-wide (needs sudo):
sudo make install

# User-local, no sudo:
make install PREFIX=$HOME/.local

# Staged install for packagers:
make install DESTDIR=/tmp/pkgroot PREFIX=/usr
```

Drops `libffs_c.{so,dylib,dll}` into `$(PREFIX)/lib` and the header into `$(PREFIX)/include/ffs.h`. Remove with `make uninstall`, which honours the same `PREFIX` and `DESTDIR`.

Link against it after install:

```bash
cc my_app.c -lffs_c -o my_app
```

Ensure `$(PREFIX)/lib` is on your runtime library search path (`LD_LIBRARY_PATH` on Linux, `DYLD_LIBRARY_PATH` on macOS, or an entry in `/etc/ld.so.conf.d/`).

### Minimal example

```c
#include <ffs.h>
#include <stdio.h>

int main(void) {
    FfsResult *res = ffs_create_instance(
        ".",        // base_path
        "",         // frecency_db_path (empty = default)
        "",         // history_db_path
        false,      // use_unsafe_no_lock
        true,       // enable_mmap_cache
        true,       // enable_content_indexing
        true,       // watch
        false       // ai_mode
    );
    if (!res->success) {
        fprintf(stderr, "init failed: %s\n", res->error);
        ffs_free_result(res);
        return 1;
    }
    void *handle = res->handle;
    ffs_free_result(res);

    // Search
    FfsResult *search = ffs_search(handle, "main.rs", "", 0, 0, 20, 100, 3);
    // ... read FfsSearchResult from search->handle, then ffs_free_search_result()

    ffs_destroy(handle);
    return 0;
}
```

> Function and type names follow the symbols emitted in [`crates/ffs-c/include/ffs.h`](./crates/ffs-c/include/ffs.h); always treat that header as the source of truth.

### Notes

- Every function returning `FfsResult*` allocates with Rust's `Box`. Free it with the corresponding `ffs_free_*` helper, not `malloc`'s `free`.
- Payloads (search results, grep results, scan progress) have their own dedicated free functions listed in the header.
- C strings returned in the `handle` field are freed with `ffs_free_string`.

### Code-aware FFI (`ffs_engine_*`)

The C library exports a code-aware surface backed by the engine: engine creation, dispatch, symbol lookup, callers/callees, and budgeted reads. Functions and types are listed in [`crates/ffs-c/include/ffs.h`](./crates/ffs-c/include/ffs.h) under the `ffs_engine_*` prefix (e.g. `ffs_engine_new`, `ffs_engine_dispatch`, `ffs_engine_symbol`, `ffs_engine_read`).

Source: [`crates/ffs-c/`](./crates/ffs-c/).

</details>

Stable C ABI. Bind from C/C++, Zig, Go via cgo, Python via ctypes, or anything with C FFI.

---

## What is ffs and why use it over ripgrep or fzf?

ffs is a file-search library, not a CLI. Ripgrep and fzf are great tools, but they are command-line programs: every call forks a new process, re-reads `.gitignore`, re-stats directories, and rebuilds whatever state it needs in memory before it can answer. That is fine when you grep once from a shell. It is bad when an editor or an AI agent wants to run hundreds of searches per session.

ffs keeps the index, file cache, and symbol index resident in one long-lived process and exposes the same Rust core through five thin layers: a native crate (`ffs-search`), a C library (`libffs_c`), a Node/Bun SDK (`@ff-labs/ffs-node` / `@ff-labs/ffs-bun`), an MCP server (`ffs-mcp`), and the unified `ffs` CLI. You initialise once, then every subsequent search hits warm memory. On a 500k-file Chromium checkout, that is the difference between 3-9 **seconds** per ripgrep spawn and sub-10 ms per ffs query.

The fuzzy-matching algorithm is much more comprehensive than fzf's: it is **typo-resistant** and exposes a query language with constraint parsing for prefiltering. `*.rs !test/ shcema` is a valid query for ffs, but fzf wouldn't find anything even with a single typo in `shcema`.

### Why a programmatic API matters

- No process spawn. Every call stays in-process and avoids the fork, exec, argv parsing, and stdout pipe setup that dominates short `rg` invocations.
- One FS walk, one metadata collection, one parse of `.gitignore`. The ignore walker runs once at scan time and the result is reused for every search.
- Results come back as typed objects, not text you have to re-parse. The SDK gives you `{ relativePath, lineNumber, lineContent, gitStatus, totalFrecencyScore, isDefinition, ... }` directly.
- Cursor pagination that survives across calls. Ripgrep has no concept of "page 2 of these matches"; ffs does.
- A long-lived process opens up optimisations a one-shot CLI cannot apply: warm caches, incremental re-indexing, cross-query frecency, shared SIMD state.

### What the core actually does

- **Frecency-ranked fuzzy matching.** Every indexed file carries an access score and a modification score. Searches rank files you have opened recently and frequently above cold results.
- **Typo-resistant matching for both paths and content.** Smith-Waterman fuzzy scoring is available on the grep path; path search uses SIMD-accelerated fuzzy matching (via the [`frizbee`](https://github.com/saghm/frizbee)-derived core) that survives dropped characters and reorderings.
- **Content grep with three modes.** Plain literal (SIMD memmem), regex (the Rust `regex` crate), and fuzzy (Smith-Waterman per line). Auto-detects which mode to use from the pattern, falls back to fuzzy when a plain search returns zero hits.
- **Multi-pattern OR search.** SIMD Aho-Corasick for "find any of these 20 identifiers at once", which is faster than regex alternation and a lot faster than 20 separate ripgrep runs.
- **Code-aware queries.** Tree-sitter symbol index, bigram + bloom pre-filter stack, callers / callees / dependents / impact ranking. All built in one pass via `UnifiedScanner`.
- **Background file watcher.** The index updates as files change. You never pay for a rescan on the hot path.
- **Git status awareness.** Modified, staged, untracked, and ignored states are cached and returned with every result, so callers can sort or filter without shelling out to git. The watcher talks to libgit2 directly instead of spawning the `git` CLI.
- **Definition classifier.** A byte-level scanner on the Rust side tags lines that start with `struct`, `fn`, `class`, `def`, `impl`, and friends.

### Performance choices that matter

- Efficient memory allocator and allocation strategy. By default we use `mimalloc`.
- Parallel multi-thread search pipeline that is not contaminated by the orchestration logic.
- SIMD-first algorithms across the board. Efficient, non-allocating sorting.
- Platform-specific optimisations for the FS layer ([getdents64](https://linux.die.net/man/2/getdents64), the NTFS API on Windows, and others).
- Lightweight on-the-fly content index for realtime, even typo-resistant grep.
- Memory-mapped content cache. Some files are stored in virtual memory (the amount is bounded).
- Single contiguous arena for string chunks. Significantly reduces working-set memory and dramatically improves CPU cache behaviour.

### Memory allocation

Yes, ffs fundamentally requires more memory than spawning a single child process. That is the primary source of the speedup. In practice, alongside one of the most popular file pickers for Neovim, ffs ends up using less RAM than a burst of ripgrep invocations.

ffs also keeps a content index, around 360 bytes per indexed file, so roughly 36 MB for a 100k-file repo. Not every file is indexed — binaries, oversized files, and anything not eligible for grep are skipped. If even that footprint is too much, the index can be backed by a memory-mapped file instead of anonymous RAM.

### What this means in practice

If you are building an agent, an IDE extension, a pre-commit check, or any long-running tool that searches the same repository many times, calling ffs as a library is dramatically cheaper than shelling out to ripgrep. The tradeoff is real memory: ffs keeps the index in RAM and warms the content cache. On a 14k-file repo that costs about 26 MB resident. On a 500k-file repo like Chromium, expect a few hundred MB. In exchange, every single search is enriched with git status, frecency ranking, file metadata, timestamps of last access and edit, and so on.

If you are running one grep from a terminal, `rg` is still the right tool. If you run dozens of them inside the same process, ffs will pay for itself starting from the second call. If you work on an AI agent, ffs will finish preparation work before your AI has a chance to call it.

### How it compares

- **ripgrep**: ffs uses the same underlying regex engine and more advanced plain-text matching. Stores a content index and a file tree. Main wins on repeated-search workloads. Loses on "grep once from bash and exit."
- **fzf**: ffs's path search is fuzzy like fzf, but it is also frecency-aware and git-aware, and ships a more typo-tolerant algorithm. fzf is a pure match-and-filter tool; ffs ranks results by how often you actually open them.
- **Telescope / fzf-lua / snacks.picker**: ffs ships its own Neovim picker with the same ranking the MCP server and SDK use. The picker is optional; the core is the same.
- **Tantivy or other full-text search engines**: different class of tool. Tantivy indexes documents for query-time scoring at scale. ffs is scoped to one repository and optimised for sub-10 ms response. It does not persist an inverted index on disk.

---

## Repository layout

**Core (file search):**

- `crates/ffs-core` (publishes as `ffs-search`) — Rust core (fuzzy, grep, walker, watcher).
- `crates/ffs-grep`, `crates/ffs-query-parser` — supporting libraries.
- `crates/ffs-c` — C FFI used by every language binding.
- `crates/ffs-nvim` — Lua/mlua bindings for the Neovim plugin.
- `crates/ffs-mcp` — MCP server binary (`ffs-mcp`).

**Code-aware layer:**

- `crates/ffs-symbol` — tree-sitter symbol scanner + bigram / bloom filter caches.
- `crates/ffs-budget` — token-budget reader, comment/whitespace filter levels, header/footer-preserving truncation.
- `crates/ffs-engine` — unified scanner that builds the symbol / bloom / outline caches in one pass + dispatch / classify / ranking helpers.
- `crates/ffs-cli` — the `ffs` binary. All subcommands (`find`, `glob`, `grep`, `read`, `outline`, `symbol`, `callers`, `callees`, `refs`, `flow`, `siblings`, `deps`, `impact`, `dispatch`, `index`, `map`, `overview`, `mcp`, `guide`).

**Language SDKs and editor integration:**

- `packages/ffs-node` — Node.js SDK (`@ff-labs/ffs-node`).
- `packages/ffs-bun` — Bun SDK (`@ff-labs/ffs-bun`).
- `packages/pi-ffs` — pi extension (`@ff-labs/pi-ffs`).
- `lua/ffs/` — Neovim-side plugin code (`lua/ffs/engine.lua` is the code-aware wrapper).

## Benchmark infrastructure

The code-aware layer ships criterion benchmarks for every cache. Three CI workflows track them:

- **`bench-smoke`** (every PR, in `.github/workflows/rust.yml`) — `cargo bench --no-run -p ffs-symbol -p ffs-budget -p ffs-engine`. Compile-only; catches API breakage in the bench targets without paying for full runs.
- **`bench-track`** (`.github/workflows/bench-track.yml`) — `workflow_dispatch` + weekly cron (Sunday 02:00 UTC). Runs the full criterion suite end-to-end and uploads `target/criterion` as a 30-day artifact.
- **`flamegraph`** (`.github/workflows/flamegraph.yml`) — `workflow_dispatch` only. Profiles a configurable bench (defaults to `dispatch_bench` in `ffs-engine`) under `cargo flamegraph` and uploads the SVG.

The bench binaries themselves live under `crates/ffs-{symbol,budget,engine}/benches/` (`bloom`, `symbol`, `filter`, `truncate`, `dispatch`, …).

## Contributing

Bug reports and pull requests welcome. Agentic coding tools are welcome to be used, but human review is mandatory.

## License

[MIT](./LICENSE) & open source forever.
