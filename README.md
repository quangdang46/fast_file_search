<img alt="FFF" src="./assets/logo-orange.png" width="300">

<p>
  <i>A file search toolkit for humans and AI agents. Really fast.</i>
</p>

Typo-resistant path and content search, frecency-ranked file access, a background watcher, and a lightweight in-memory content index. Way faster than CLIs like ripgrep and fzf in any long-running process that searches more than once.

Originally started as [Neovim plugin](#neovim-plugin) people loved, but it turned out that plenty of AI harnesses and code editors need the same thing: accurate, fast file search as a library. That is what fff is.

A second, complementary engine called **scry** sits on top of the same core: a unified single-pass scanner that builds a symbol index, a bloom filter cache, and an outline cache in one walk, then exposes them through a sub-CLI (`scry symbol/callers/callees/read/...`), 5 extra MCP tools, an opt-in Lua module, and an opt-in C ABI. Skip to [the scry section](#scry-engine) for the details.

---

Pick what you are interested in:

<details id="mcp-server">
<summary>
<h2>MCP server</h2>
</summary>

Works with Claude Code, Codex, OpenCode, Cursor, Cline, and any MCP-capable client. Fewer grep roundtrips, less wasted context, faster answers.

![Benchmark chart comparing FFF against the built-in AI file-search tools](./chart.png)

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

It prints the exact wiring instructions for your client. Once the server is connected, ask the agent to "use fff" and it picks up the `ffgrep`, `fffind`, and `fff-multi-grep` tools.

### Recommended agent prompt

Drop this into your project's `CLAUDE.md` or equivalent:

```markdown
For any file search or grep in the current git-indexed directory, use fff tools.
```

### What changes

- Frecency memory. Files you actually open rank higher next time. Warm-up from git touch history runs automatically.
- Definition-first hinting. Lines that look like code definitions are classified on the Rust side, no regex overhead in your prompt.
- Smart-case with auto-fuzzy fallback. `IsOffTheRecord` finds snake_case variants; zero-match queries retry as fuzzy and surface the best approximate hits.
- Git-aware annotations. Modified, untracked, and staged files are tagged so the agent reaches for what you are actively changing.

Source: [`crates/ffs-mcp/`](./crates/ffs-mcp/).

#### Optional scry tools

When the MCP server is built (always, no feature flag) it also registers 5 additional tools backed by the [scry engine](#scry-engine). They are off the hot path — the existing `find_files` / `grep` / `multi_grep` are unchanged — but they let an agent ask code-aware questions instead of just file-name questions:

| Tool             | What it answers                                                                                                                            |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `scry_dispatch`  | Auto-classify a free-form query (`Repository`, `find_in_dir`, `git status`, …) and route it to the right backend.                          |
| `scry_symbol`    | Exact + prefix lookup over the symbol index built by the unified scanner (tree-sitter powered).                                            |
| `scry_callers`   | Find call sites of a symbol. Bloom-filter narrowed candidates → literal-text confirm pass.                                                 |
| `scry_callees`   | Symbols that the body of a definition references.                                                                                          |
| `scry_read`      | Token-budget aware file read. Maps `--budget N` to `N × ~85% body × 4 bytes/token` and applies a comment/whitespace filter when requested. |

The first call lazily builds the engine (`Engine::new` + `engine.index()`) under a `OnceCell<Arc<Engine>>` + double-checked `Mutex<()>`; every subsequent call reuses the warm caches.

</details>

The MCP server gives any agent a file search tool that is faster and more token-efficient than the built-in one.

<details id="scry-engine">
<summary>
<h2>scry — code-aware sub-CLI and engine</h2>
</summary>

`scry` is the second binary shipped by this repo (alongside `fff` and the MCP server). It exposes the same Rust core that powers the MCP `scry_*` tools, but as a normal CLI you can call from a shell script or a CI step.

### Architecture

```
                ┌─────────────────────────────────────────────────────┐
                │  Consumers                                          │
                │  ──────────────────────────────────────────────     │
                │  scry CLI │ scry mcp │ fff.scry (Lua) │ fff_scry_*  │
                │  (ffs-cli)  (ffs-mcp)   (ffs-nvim,opt)  (ffs-c,opt) │
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
                │  scanner     │    │  tokens, filter│    │  (existing
                │  bigram +    │    │  truncate w/   │    │  fuzzy / │
                │  bloom cache │    │  preserved     │    │  grep /  │
                │  outline     │    │  header+footer │    │  walker) │
                └──────────────┘    └────────────────┘    └──────────┘
                                ▲
                                │ single-pass UnifiedScanner walks
                                │ the repo once, populates all caches
```

Read top to bottom: every consumer calls into the same `Engine`. The engine ties three single-purpose crates together (`ffs-symbol`, `ffs-budget`, `ffs-core`) and runs `UnifiedScanner` once at index time to populate the caches all four lookup paths read from. Lua + C surfaces are opt-in (off by default, see [Optional `fff.scry` Lua module](#optional-fff-scry-lua-module) and [Optional scry FFI (`fff_scry_*`)](#optional-scry-ffi-fff_scry_)).

### What it does

A single pass over the repo (`scry index`) builds three caches at once:

- **Symbol index** — tree-sitter AST scan of every supported file, recording every definition with `kind` (function, struct, class, …), `path`, and `line`.
- **Bloom filter cache** — per-file 64-bit bloom of every identifier in the file. Used as the first stage of a 2-stage `PreFilterStack` (bigram → bloom → literal-text confirm) so symbol/caller/callee queries don't have to walk every file.
- **Outline cache** — comment/header/footer-aware byte slice for every file, used by `scry read --budget` and by the agent-facing token-budget reader.

Once the index is warm, every subcommand reuses it. There is no second walk.

### Query path

The 2-stage `PreFilterStack` is what makes `scry symbol`, `scry callers`, and `scry callees` cheap on large repos:

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

Stages 1 and 2 are pure metadata lookups; the expensive `String::contains` only ever runs on the survivors. On the fff repo (3.3k symbols across 229 files) a typical `scry callers` query inspects fewer than 30 files.

### Sub-commands

```
scry find      | Find files by name (replaces `find`, `fd`).
scry glob      | Match files by glob (replaces `glob`, shell `**`).
scry grep      | Search file contents (replaces `grep`, `rg`).
scry read      | Read a file with token-budget aware truncation (replaces `cat`).
scry symbol    | Look up symbol definitions (NEW, tree-sitter powered).
scry callers   | List call sites of a symbol (NEW).
scry callees   | List symbols referenced in a symbol body (NEW).
scry dispatch  | Auto-classify a free-form query and route it.
scry index     | Build / refresh the on-disk indexes (Bigram, Bloom, Symbol, Outline).
scry mcp       | Run as MCP server over stdio (replaces agent built-ins Grep/Glob/Read).
```

The novel pieces are **`symbol`**, **`callers`**, **`callees`**, **`read --budget`**, and **`dispatch`**. The rest are convenience wrappers over the existing ffs-search core.

### Token-budgeted read

The killer feature for AI harnesses. `scry read path --budget 5000 --filter minimal`:

1. Converts the budget: `tokens × 0.85 × 4 bytes/token` ≈ `17_000` bytes.
2. Loads the file, applies the requested `--filter` level (`none`, `minimal`, `aggressive`) to drop comments / whitespace while preserving doc-comments and the file header.
3. Truncates from the body, **always preserving** the first ~5 lines (header) and a `[truncated to budget]\n` footer so the agent knows the output was clipped.

The classifier and apply-preserving-footer logic live in [`crates/ffs-budget/`](./crates/ffs-budget/); the symbol scanner is in [`crates/ffs-symbol/`](./crates/ffs-symbol/); the unified `Engine` that ties them together is in [`crates/ffs-engine/`](./crates/ffs-engine/).

### Build and run

`scry` is part of the default workspace build:

```bash
make build                      # builds fff, scry, MCP server, ffs_nvim, fff_c
./target/release/scry index     # one-time warm-up, ~200 ms on a 10k-file repo
./target/release/scry symbol UnifiedScanner
./target/release/scry callers UnifiedScanner
./target/release/scry read crates/ffs-engine/src/lib.rs --budget 5000 --filter minimal
./target/release/scry dispatch 'where is the user controller'
./target/release/scry mcp       # JSON-RPC over stdio with the 5 scry_* tools
```

Output format defaults to plain text; pass `--format json` for machine-readable output.

Source: [`crates/ffs-cli/`](./crates/ffs-cli/), [`crates/ffs-engine/`](./crates/ffs-engine/), [`crates/ffs-symbol/`](./crates/ffs-symbol/), [`crates/ffs-budget/`](./crates/ffs-budget/).

</details>

`scry` is the code-aware companion to fff: same core, three new caches (symbol / bloom / outline), an extra sub-CLI, and an opt-in Lua/C surface for the same engine.

<details id="pi-extension">
<summary>
<h2>Pi agent extension</h2>
</summary>

### Install

```bash
pi install npm:@ff-labs/pi-ffs
```

### Modes

Three operating modes, switchable at runtime with `/fff-mode`:

| Mode                     | What it does                                                                      |
| ------------------------ | --------------------------------------------------------------------------------- |
| `tools-and-ui` (default) | Adds `ffgrep` and `fffind` tools, replaces `@`-mention autocomplete with FFF.     |
| `tools-only`             | Only tool injection. Keeps pi's native editor autocomplete.                       |
| `override`               | Replaces pi's built-in `grep`, `find`, and `multi_grep` with FFF implementations. |

Env vars: `PI_FFF_MODE`, `FFF_FRECENCY_DB`, `FFF_HISTORY_DB`. Flags: `--fff-mode`, `--fff-frecency-db`, `--fff-history-db`.

### Agent-facing tools

- `ffgrep`. Content search. Accepts `path`, `exclude` (comma, space, or array; leading `!` optional), `caseSensitive`, `context`, and cursor pagination. Auto-detects regex, falls back to fuzzy on zero exact matches, rejects `.*`-style wildcard-only patterns up front.
- `fffind`. Path and filename search. Matches the whole repo-relative path, not just the filename. Frecency-aware. The weak-match detector flags scattered fuzzy noise before it floods the agent's context.

### Commands

- `/fff-mode [tools-and-ui | tools-only | override]`. Show or switch the mode.
- `/fff-health`. Picker, frecency, and git integration status.
- `/fff-rescan`. Force a rescan.

Source: [`packages/pi-ffs/`](./packages/pi-ffs/).

</details>

The Pi extension swaps pi's native tools for FFF implementations and feeds the interactive editor's `@`-mention autocomplete from the frecency-ranked index.

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
    require("fff.download").download_or_build_binary()
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
    { "ff", function() require('fff').find_files() end, desc = 'FFFind files' },
    { "fg", function() require('fff').live_grep() end, desc = 'LiFFFe grep' },
    { "fz",
      function() require('fff').live_grep({ grep = { modes = { 'fuzzy', 'plain' } } }) end,
      desc = 'Live fffuzy grep',
    },
    { "fc",
      function() require('fff').live_grep({ query = vim.fn.expand("<cword>") }) end,
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
      require('fff.download').download_or_build_binary()
    end
  end,
})

vim.g.fff = {
  lazy_sync = true,
  debug = { enabled = true, show_scores = true },
}

vim.keymap.set('n', 'ff', function() require('fff').find_files() end, { desc = 'FFFind files' })
```

### Public API

```lua
require('fff').find_files()                        -- find files in current repo
require('fff').live_grep()                         -- live content grep
require('fff').scan_files()                        -- force rescan
require('fff').refresh_git_status()                -- refresh git status
require('fff').find_files_in_dir(path)             -- find in a specific dir
require('fff').change_indexing_directory(new_path) -- change root
```

### Commands

- `:FFFScan`. Rescan files.
- `:FFFRefreshGit`. Refresh git status.
- `:FFFClearCache [all|frecency|files]`. Clear caches.
- `:FFFHealth`. Health check.
- `:FFFDebug [on|off|toggle]`. Toggle the scoring display.
- `:FFFOpenLog`. Open `~/.local/state/nvim/log/fff.log`.

### Configuration

Defaults are sensible. Override only what you care about.

```lua
require('fff').setup({
  base_path = vim.fn.getcwd(),
  prompt = '> ',
  title = 'FFFiles',
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
    db_path = vim.fn.stdpath('data') .. '/fff_queries',
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
    log_file = vim.fn.stdpath('log') .. '/fff.log',
    log_level = 'info',
  },
})
```

### Live grep modes

`<S-Tab>` cycles between `plain`, `regex`, and `fuzzy`. The list is configurable via `grep.modes`, and single-mode setups hide the indicator entirely.

Per-call override:

```lua
require('fff').live_grep({ grep = { modes = { 'fuzzy', 'plain' } } })
require('fff').live_grep({ query = 'search term' }) -- pre-fill
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

FFF honours `.gitignore`. For picker-only ignores that do not touch git, add a sibling `.ignore` file:

```gitignore
*.md
docs/archive/**/*.md
```

Run `:FFFScan` to force a rescan.

### Troubleshooting

- `:FFFHealth` verifies picker init, optional dependencies, and DB connectivity.
- `:FFFOpenLog` opens the log file.

### Optional `fff.scry` Lua module

The Neovim cdylib (`ffs_nvim`) ships with an **opt-in** `scry` Cargo feature that exposes the [scry engine](#scry-engine) directly to Lua. The default build does not include it — `make build` produces a binary symbol-identical to the pre-scry release. To enable it:

```bash
cargo build --release -p ffs-nvim --features scry
```

Once enabled, the wrapper at [`lua/ffs/scry.lua`](./lua/ffs/scry.lua) gives you:

```lua
local scry = require('fff.scry')
scry.init(vim.fn.getcwd())                                    -- one-time index
for _, hit in ipairs(scry.symbol('FilePicker')) do
  print(hit.path, hit.line, hit.kind)
end
local res = scry.read('lua/ffs/main.lua', 5000, 'minimal')    -- token-budget read
print(scry.dispatch('UnifiedScanner'))                        -- auto-classify
scry.rebuild()                                                -- refresh caches
```

If the loaded `ffs_nvim` cdylib was built without `--features scry`, the module raises a clear error explaining how to rebuild. Inputs are validated with `vim.validate()`.

The existing ffs.nvim picker API (`require('fff').find_files()`, etc.) is **unchanged** in either build mode.

</details>

The best file search picker for neovim. Period. Faster and more intuitive queries, frecency ranking, definition classification and much more.

<details id="node-sdk">
<summary>
<h2>Node & Bun SDK</h2>
</summary>

```bash
npm install @ff-labs/ffs-node
# or
bun add @ff-labs/ffs-node
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

Every method returns a `Result<T>` (`{ ok: true, value } | { ok: false, error }`). Full type reference: [`packages/fff-node/src/types.ts`](./packages/fff-node/src/types.ts).

</details>

TypeScript wrapper over the C library for nodejs and bun. Build custom agent tools, CLIs, or IDE integrations on top of FFF.

<details id="rust-crate">
<summary>
<h2>Rust crate</h2>
</summary>

### Add the dependency

FFF is written in Rust, so this is the lowest-overhead way to use it.

```toml
[dependencies]
ffs-search = "0.6"
```

Full API documentation: [docs.rs/ffs-search](https://docs.rs/ffs-search/latest/fff_search/).

</details>

Native rust crate that is performing all the search. Stable and well documented.

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

The output is a `cdylib` (`libffs_c.so` / `libffs_c.dylib` / `ffs_c.dll`). The header lives at [`crates/ffs-c/include/fff.h`](./crates/ffs-c/include/fff.h).

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

Drops `libffs_c.{so,dylib,dll}` into `$(PREFIX)/lib` and the header into `$(PREFIX)/include/fff.h`. Remove with `make uninstall`, which honours the same `PREFIX` and `DESTDIR`.

Link against it after install:

```bash
cc my_app.c -lfff_c -o my_app
```

Ensure `$(PREFIX)/lib` is on your runtime library search path (`LD_LIBRARY_PATH` on Linux, `DYLD_LIBRARY_PATH` on macOS, or an entry in `/etc/ld.so.conf.d/`).

### Minimal example

```c
#include <fff.h>
#include <stdio.h>

int main(void) {
    FffResult *res = fff_create_instance(
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
        fff_free_result(res);
        return 1;
    }
    void *handle = res->handle;
    fff_free_result(res);

    // Search
    FffResult *search = fff_search(handle, "main.rs", "", 0, 0, 20, 100, 3);
    // ... read FffSearchResult from search->handle, then fff_free_search_result()

    fff_destroy(handle);
    return 0;
}
```

### Notes

- Every function returning `FffResult*` allocates with Rust's `Box`. Free with `fff_free_result`, do not use malloc's free
- Payloads (search results, grep results, scan progress) have their own dedicated free functions listed in the header.
- C strings returned in the `handle` field (e.g. from `fff_get_base_path`) are freed with `fff_free_string`.

### Optional scry FFI (`fff_scry_*`)

The C library has an opt-in `scry` Cargo feature that adds a second, code-aware surface backed by the [scry engine](#scry-engine). The default build excludes it — pre-scry consumers see a byte-identical ABI:

```bash
cargo build --release -p ffs-c --features scry
cc my_app.c -DFFF_SCRY -lfff_c -o my_app
```

When `FFF_SCRY` is defined, `fff.h` exposes 7 extra functions:

```c
struct FffScryEngine *fff_scry_engine_new(const char *root, uint64_t total_token_budget);
int32_t                fff_scry_engine_rebuild(struct FffScryEngine *engine);
void                   fff_scry_engine_free(struct FffScryEngine *engine);

struct FffScryResponse *fff_scry_dispatch(struct FffScryEngine *engine, const char *query);
struct FffScryResponse *fff_scry_symbol  (struct FffScryEngine *engine, const char *name);
struct FffScryResponse *fff_scry_read    (struct FffScryEngine *engine, const char *path,
                                          uint64_t budget, const char *filter);

void                   fff_free_scry_response(struct FffScryResponse *response);
```

`FffScryResponse` carries a JSON payload + length + status code. Callers free it with `fff_free_scry_response`. The new types and functions are wrapped in `#if defined(FFF_SCRY)` in [`crates/ffs-c/include/fff.h`](./crates/ffs-c/include/fff.h), so the default include surface is unchanged when the feature is off.

Source: [`crates/ffs-c/`](./crates/ffs-c/).

</details>

Stable C ABI. Bind from C/C++, Zig, Go via cgo, Python via ctypes, or anything with C FFI.

---

## What is FFF and why use it over ripgrep or fzf?

FFF is a file search library, not a CLI. Ripgrep and fzf are great tools, but they are command-line programs: every call forks a new process, re-reads `.gitignore`, re-stats directories, and rebuilds whatever state it needs in memory before it can answer. That is fine when you grep once from a shell. It is bad when an editor or an AI agent wants to run hundreds of searches per session.

FFF keeps the index and the file cache resident in one long-lived process and exposes the same Rust core through four thin layers: a native crate (`ffs-search`), a C library (`libffs_c`), a Node/Bun SDK (`@ff-labs/ffs-node`), and an MCP server. You call `FileFinder.create()` once, then every subsequent search hits warm memory. On a 500k-file Chromium checkout, that is the difference between 3-9 **SECONDS** per ripgrep spawn and sub-10 ms per FFF query.

Algorithm for fuzzy matching is much more comprehensive than fzf's algorithm it is **typo-resistant** and we provide a query language with additional constraint parsing for prefiltering e.g. "*.rs !test/ shcema" is a perfectly valid query for fff, but fzf wouldn't find anything even for a single typo in "shcema".

### Why a programmatic API matters

- No process spawn. Every call stays in-process and avoids the fork, exec, argv parsing, and stdout pipe setup that dominates short `rg` invocations.
- One FS walk, metadata collection, and parse of `.gitignore`. The ignore walker runs once at scan time and the result is reused for every search.
- Results come back as typed objects, not text you have to re-parse. The SDK gives you `{ relativePath, lineNumber, lineContent, gitStatus, totalFrecencyScore, isDefinition, ... }` directly.
- Cursor pagination that survives across calls. Ripgrep has no concept of "page 2 of these matches"; FFF does.
- A long-lived process opens up optimisations that a one-shot CLI cannot apply: warm caches, incremental re-indexing, cross-query frecency, and shared SIMD state.

### What the core actually does

- **Frecency-ranked fuzzy matching.** Every indexed file carries an access score and a modification score. Searches rank files you have opened recently and frequently above cold results. This is the same idea as VS Code's recently-opened list, but applied to every search result, not just a sidebar.
- **Typo-resistant matching for both paths and content.** Smith-Waterman fuzzy scoring is available on the grep path; path search uses SIMD-accelerated fuzzy matching (via the [`frizbee`](https://github.com/saghm/frizbee)-derived core) that survives dropped characters and reorderings.
- **Content grep with three modes.** Plain literal (SIMD memmem), regex (the Rust `regex` crate), and fuzzy (Smith-Waterman per line). Auto-detects which mode to use from the pattern, falls back to fuzzy when a plain search returns zero hits.
- **Multi-pattern OR search.** SIMD Aho-Corasick for "find any of these 20 identifiers at once", which is faster than regex alternation and a lot faster than 20 separate ripgrep runs.
- **Background file watcher.** The index updates as files change. You never pay for a rescan on the hot path.
- **Git status awareness.** Modified, staged, untracked, and ignored states are cached and returned with every result, so callers can sort or filter them without shelling out to git. The watcher talks to libgit2 directly instead of spawning the `git` CLI.
- **Definition classifier.** A byte-level scanner on the Rust side tags lines that start with `struct`, `fn`, `class`, `def`, `impl`, and friends.

### Performance choices that matter

- Efficient memory allocator and memory allocation strategy (see next paragraph). By default we use `mimaloc`
- Parallel multi thread search pipeline that is not contaganted by the orchistration logic
- SIMD first algorithms for everything. Efficinet & non-allocating sorting.
- Platform specific optimizations for FS ([getdents64](https://linux.die.net/man/2/getdents64), NTFS api on windows and others)
- Lightweight on the flight content index for realtime even typo resistant grep
- Memory mapped content cache. We store some of the files in virtual memory (the amount is limited)
- Single contiguous arena storage of string chunks. Significantly reduces the amount of memory to work with and dramatically increases CPU cache hits.

### Memory allocation

Yes, fff fundamentally requires more memory than calling a single child process. That is the primary source of the speedup. In practice, alongside one of the most popular file search pickers for Neovim, [fff ends up using less RAM than a burst of ripgrep invocations](https://x.com/neogoose_btw/status/2041606853155811442).


FFF also keeps a content index, around 360 bytes per indexed file, so roughly 36 MB for a 100k-file repo. Not every file is indexed - binaries, oversized files, and anything not eligible for grep are skipped. If even that footprint is too much, the index can be backed by a memory-mapped file instead of anonymous RAM.

### What this means in practice

If you are building an agent, an IDE extension, a pre-commit check, or any long-running tool that searches the same repository many times, calling FFF as a library is dramatically cheaper than shelling out to ripgrep. The tradeoff is real memory: FFF keeps the index in RAM and warms the content cache. On a 14k-file repo that costs about 26 MB resident. On a 500k-file repo like Chromium, expect a few hundred MB. In exchange, every single search is enriched with git status, frecency ranking, file metadata, timestamps of last access and edit and so on.

If you are running one grep from a terminal, `rg` is still the right tool. If you run dozens of them inside the same process, FFF will pay for itself starting from the second call. If you work on AI agent fff will finish preparation work before your AI will have a chance to call it.

### How it compares

- **ripgrep**: FFF uses the same underlying regex engine and more advanced plain text matching algorithms. Stores content index and file tree. Main wins on repeated-search workloads. Loses on "grep once from bash and exit."
- **fzf**: FFF's path search is fuzzy like fzf, but it is also frecency-aware and git-aware, and ships a more typo-tolerant algorithm. fzf is a pure match-and-filter tool; FFF ranks results by how often you actually open them.
- **Telescope / fzf-lua / snacks.picker**: FFF ships its own Neovim picker with the same ranking the MCP server and SDK use. The picker is optional; the core is the same.
- **Tantivy or other full-text search engines**: different class of tool. Tantivy indexes documents for query-time scoring at scale. FFF is scoped to one repository and optimised for sub-10 ms response. It does not persist an inverted index on disk.

---

## Repository layout

**Core (file search):**

- `crates/ffs-search`, `crates/ffs-grep`, `crates/ffs-query-parser` - Rust core.
- `crates/ffs-c` - C FFI used by every language binding.
- `crates/ffs-nvim` - Lua/mlua bindings for the Neovim plugin.
- `crates/ffs-mcp` - MCP server binary.

**scry engine (code-aware layer):**

- `crates/ffs-symbol` - tree-sitter symbol scanner + bigram / bloom filter caches.
- `crates/ffs-budget` - token-budget reader, comment/whitespace filter levels, header/footer-preserving truncation.
- `crates/ffs-engine` - unified scanner that builds the symbol / bloom / outline caches in one pass + dispatch / classify / ranking helpers.
- `crates/ffs-cli` - the `scry` binary. Subcommands: `find`, `glob`, `grep`, `read`, `symbol`, `callers`, `callees`, `dispatch`, `index`, `mcp`.

**Language SDKs and editor integration:**

- `packages/fff-node` - Node.js SDK (`@ff-labs/ffs-node`).
- `packages/fff-bun` - Bun SDK (`@ff-labs/ffs-node`).
- `packages/pi-ffs` - pi extension (`@ff-labs/pi-ffs`).
- `lua/` - Neovim-side plugin code (`lua/ffs/scry.lua` is the optional scry wrapper).

## Benchmark infrastructure

The scry layer ships criterion benchmarks for every cache. Three CI workflows track them:

- **`bench-smoke`** (every PR, in `.github/workflows/rust.yml`) - `cargo bench --no-run -p ffs-symbol -p ffs-budget -p ffs-engine`. Compile-only; catches API breakage in the bench targets without paying for full runs.
- **`bench-track`** (`.github/workflows/bench-track.yml`) - `workflow_dispatch` + weekly cron (Sunday 02:00 UTC). Runs the full criterion suite end-to-end and uploads `target/criterion` as a 30-day artifact.
- **`flamegraph`** (`.github/workflows/flamegraph.yml`) - `workflow_dispatch` only. Profiles a configurable bench (defaults to `dispatch_bench` in `ffs-engine`) under `cargo flamegraph` and uploads the SVG.

The bench binaries themselves live under `crates/ffs-{symbol,budget,engine}/benches/` (`bloom`, `symbol`, `filter`, `truncate`, `dispatch`, …).

## Contributing

Bug reports and pull requests welcome. Agentic coding tools are welcome to be used, but human review is mandatory.

## License

[MIT](./LICENSE) & open source forever.
