# @ff-labs/pi-ffs

A [pi](https://github.com/badlogic/pi-mono) extension that replaces the built-in `find` and `grep` tools with [ffs](https://github.com/dmtrKovalenko/ffs.nvim) — a Rust-native, SIMD-accelerated file finder with built-in memory.

## What it does

| Built-in tool | pi-ffs replacement | Improvement |
|---|---|---|
| `find` (spawns `fd`) | `ffsfind` (ffs `fileSearch`) | Fuzzy matching, frecency ranking, git-aware, pre-indexed |
| `grep` (spawns `rg`) | `ffsgrep` (ffs `grep`) | SIMD-accelerated, frecency-ordered, mmap-cached, no subprocess |
| *(none)* | `ffs-multi-grep` (ffs `multiGrep`) | OR-logic multi-pattern search via Aho-Corasick |
| `@` file autocomplete (fd-backed) | `@` file autocomplete (ffs-backed, default) | Fuzzy ranking from ffs index/frecency |

### Key advantages over built-in tools

- **No subprocess spawning** — ffs is a Rust native library called through the Node binding. No `fd`/`rg` process per call.
- **Pre-indexed** — files are indexed in the background at session start. Searches are instant.
- **Frecency ranking** — files you access often rank higher. Learns across sessions.
- **Query history** — remembers which files were selected for which queries. Combo boost.
- **Git-aware** — modified/staged/untracked files are boosted in results.
- **Smart case** — case-insensitive when query is all lowercase, case-sensitive otherwise.
- **Fuzzy file search** — `find` uses fuzzy matching, not glob-only. Typo-tolerant.
- **Cursor pagination** — grep results include a cursor for fetching the next page.

## Install

Requirements:
- pi

### Install as a pi package

**Via npm (recommended):**

```bash
pi install npm:@ff-labs/pi-ffs
```

Project-local install:

```bash
pi install -l npm:@ff-labs/pi-ffs
```

**Via git:**

```bash
pi install git:github.com/dmtrKovalenko/ffs.nvim
```

Pin to a release:

```bash
pi install git:github.com/dmtrKovalenko/ffs.nvim@v0.3.0
```

### Local development / manual install

```bash
git clone https://github.com/dmtrKovalenko/ffs.nvim.git
cd ffs.nvim/packages/pi-ffs
npm install
```

Then add to your pi `settings.json`:

```json
{
  "extensions": ["/path/to/ffs.nvim/packages/pi-ffs/src/index.ts"]
}
```

Or test directly:

```bash
pi -e /path/to/ffs.nvim/packages/pi-ffs/src/index.ts
```

This extension registers ffs-powered tools (`ffsfind`, `ffsgrep`, `ffs-multi-grep`) alongside pi's built-in tools.

## Tools

### `ffsgrep`

Search file contents. Smart case, plain text by default, regex optional.

Parameters:
- `pattern` — search text or regex
- `path` — directory/file constraint (e.g. `src/`, `*.ts`)
- `ignoreCase` — force case-insensitive
- `literal` — treat as literal string (default: true)
- `context` — context lines around matches
- `limit` — max matches (default: 100)
- `cursor` — pagination cursor from previous result

### `ffsfind`

Fuzzy file name search. Frecency-ranked.

Parameters:
- `pattern` — fuzzy query (e.g. `main.ts`, `src/ config`)
- `path` — directory constraint
- `limit` — max results (default: 200)

### `ffs-multi-grep`

OR-logic multi-pattern content search. SIMD-accelerated Aho-Corasick.

Parameters:
- `patterns` — array of literal patterns (OR logic)
- `constraints` — file constraints (e.g. `*.{ts,tsx} !test/`)
- `context` — context lines
- `limit` — max matches (default: 100)
- `cursor` — pagination cursor

## Commands

- `/ffs-health` — show ffs status (indexed files, git info, frecency/history DB status)
- `/ffs-rescan` — trigger a file rescan
- `/ffs-mode <mode>` — switch mode (tool name change requires restart)

## Modes

- `tools-and-ui` (default): registers `ffsfind`, `ffsgrep`, `ffs-multi-grep` as additional tools + ffs-backed `@` autocomplete
- `tools-only`: additional tools only; keep pi's default `@` autocomplete
- `override`: replaces pi's built-in `find`, `grep` and adds `multi_grep` + ffs-backed `@` autocomplete

Mode precedence:
1. `--ffs-mode <mode>` CLI flag
2. `PI_FFS_MODE=<mode>` environment variable
3. default (`tools-and-ui`)

## Flags

- `--ffs-mode <mode>` — set mode (see above)
- `--ffs-frecency-db <path>` — path to frecency database (also: `FFS_FRECENCY_DB` env)
- `--ffs-history-db <path>` — path to query history database (also: `FFS_HISTORY_DB` env)

## Data

When database paths are provided, ffs stores:
- frecency database — file access frequency/recency
- history database — query-to-file selection history

No project files are uploaded anywhere by this extension. It runs locally and only uses the configured LLM through pi itself.

## Security

- No shell execution
- No network calls in the extension code
- No telemetry
- No credential handling beyond whatever pi and your configured model provider already do
- Search state is stored locally under `~/.pi/agent/ffs/`
