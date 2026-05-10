# scry — agent guide

`scry` is a unified code search, read, and symbol-lookup tool. Output is
token-budget aware and uniformly available as either human text or JSON
(`--format json`), so agents can stay on a single CLI for the whole
read-and-reason loop.

This guide describes the public sub-commands as of the version that
shipped this file. Versions are stable; new behaviour is added as new
flags rather than via behaviour changes to existing flags.

## Global flags

* `--root <DIR>` — project root to search/index. Defaults to the
  current working directory.
* `--format text|json` — output format. `text` is the default and is
  intended for humans; `json` is canonical and intended for tools.

All sub-commands accept these.

## Pagination

Listing sub-commands (`find`, `symbol`, `callers`, `callees`,
`siblings`) take:

* `--limit <N>` — max items in this page (defaults differ per command;
  typically 50–100).
* `--offset <N>` — skip this many items before the page starts.
  Defaults to 0.

JSON responses always include `total`, `offset`, `has_more`. Text
responses end with a footer like `[1-50 of 217] — next: --offset 50`
that tells the next page's `--offset` directly.

## Sub-commands

### `find <needle>`
Filename matcher. Substring (case-insensitive) match against full
file paths under `--root`. Useful as the first step when you know a
filename fragment but not the full path.

### `glob <pattern>`
Shell-style glob over file paths (`**`, `*`, `?`, character classes).

### `grep <regex>`
Content search across the project. Powered by `ripgrep` semantics for
gitignore handling.

### `read <target>`
Read a file. Default emits the **agent-style outline** (header +
section list + footer hint) so agents can plan a follow-up drill
without pulling the full body. Pass `--full` for raw contents, or
use `path:line` / `--section` to drill straight into a structural
section.

Routing:
* `read <path>` — outline default (code files only; non-code files
  fall through to full body).
* `read <path>:<N>` — structural section read at line `N`.
* `read <path> --section` (combined with `path:N`) — same as above but
  errors out if the line isn't supplied, useful in scripts.
* `read <path> --full` — whole file body (legacy default).

* `--budget <N>` — total token budget (default 25000). Effective byte
  cap ≈ `tokens × 0.85 × 4`; the remaining ≈15% is reserved for the
  envelope and truncation footer.
* `--filter none|minimal|aggressive` — `none` keeps the file as-is;
  `minimal` strips full-line comments; `aggressive` collapses
  impl/class bodies to fit more files into the same budget.

### `outline <path>`
Tree-sitter outline of a single file: top-level functions, classes,
structs, imports, and their immediate children where applicable.

* `--style agent|tabular|markdown|structured` — text rendering.
  `agent` (default) is the dense, agent-friendly form: header line
  with file totals, `[A-B]` left column, bundled-imports row, indented
  signatures, and a `> Next:` drill hint. `tabular` is fixed-width
  columns (KIND/NAME/LINES/SIGNATURE); `markdown` is nested bullets;
  `structured` is an ASCII tree (`├─` / `└─`).
* JSON output (`--format json`) is independent of `--style` and emits
  the entry tree directly with `kind`, `name`, `start_line`,
  `end_line`, `signature`, `children`.

### `symbol <name>`
Look up a symbol definition by name. Tree-sitter AST-driven, not
substring. Trailing `*` is treated as a prefix glob: `symbol Foo*`
matches every name starting with `Foo`.

JSON response includes a `facets` field: `{ total, by_kind }` counted
on the *full* candidate set (i.e. unaffected by `--offset` /
`--limit`). Text output appends a `by kind: function: 12, struct: 3`
line under the pagination footer.

### `callers <name>`
Find lines that reference `<name>` outside its own definition site.
Bloom-narrowed first, then confirmed with a literal-text pass.

### `callees <name>`
Identifiers referenced inside the body of `<name>`, then resolved
back to definitions via the symbol index. Hits with the same
`(name, path, line)` triple are de-duplicated even when they came
from multiple definition bodies of `<name>`.

### `siblings <name>`
Other symbols defined at the same scope as `<name>` — peer methods of
the same class, peer functions in the same file, peer items in the
same module. Top-level targets get all other top-level entries.

* `--include-imports` — off by default; on returns the file's import
  block as siblings as well.

Each hit carries `parent` (`"<file>"` for top-level, otherwise the
enclosing definition's name) and `target_path`/`target_line` so
multiple definition sites of the same name remain disambiguated.

### `dispatch <query>`
Free-form classifier that routes a query to the right backend
(`symbol`, `symbol_glob`, `file_path`, `glob`, or content fallback).

### `index`
Build / refresh the on-disk indexes (Bigram, Bloom, Symbol, Outline)
without running a query. Useful for warming up before a session.

### `mcp`
Run as an MCP (Model Context Protocol) server over stdio. Replaces
agent built-ins like Grep / Glob / Read while still exposing the same
sub-commands above.

### `guide`
Print this document.

## Conventions and tips

* All listing commands return `total` so an agent can decide whether
  to widen, paginate, or refine the query without a second round-trip.
* `siblings` + `outline` together let an agent reconstruct the
  enclosing scope of any symbol without re-reading the file: outline
  for kind/signature, siblings for "what else is nearby".
* Prefer `--format json` for any programmatic consumer; text output
  may evolve for readability while JSON is treated as part of the
  contract.
* Use `--limit` aggressively. Most lookups want the top 5–20 hits;
  the cost of the underlying scan is independent of `--limit` but
  output marshalling is not.
