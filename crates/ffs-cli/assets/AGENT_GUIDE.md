# ffs — agent guide

`ffs` is one CLI for file search, content search, and code navigation.
Output is token-budget aware; `--format json` is the stable contract.

Global: `--root <DIR>` (default: cwd), `--format text|json`.

Listing commands (`find`, `symbol`, `callers`, `callees`, `refs`, `flow`,
`siblings`) take `--limit` (default 50–100) and `--offset`. JSON always has
`total`/`offset`/`has_more`; text ends with `[1-50 of 217] — next: --offset 50`.

## Sub-commands

### `find <needle>`
Case-insensitive substring match on file paths. `--scored` adds role
detection and score bonus (`path  [role] (+score)`); roles: implementation
+20, auth +20, ui +20, test −15, docs −25, config 0.

### `glob <pattern>`
Shell glob over file paths (`**`, `*`, `?`, character classes).

### `grep <pattern>`
Content search with ripgrep gitignore semantics. `--group` groups matches
by file and enclosing symbol (JSON schema `v2_grouped`, `files[].groups[]`).
See `ffs grep --help` for the full flag set (context, `-l`, `-m`, `-o`,
glob filters, `--compact`, pagination).

### `read <target>`
Defaults to the agent-style outline (header + sections + next-step hint).
* `read <path>` — outline for code files; non-code falls through to body.
* `read <path>:<N>` — structural section at line N.
* `read <path>:<N> --section` — same, but errors when N is missing.
* `read <path> --full` — whole body.
Bare filename with no separator: resolves by basename; one match drills in
(`resolved_from`), multiple emits `mode: "candidates"` with the top 5
shortest paths.
* `--budget <N>` — token budget (default 25000); ≈15% is reserved for the
  envelope and truncation footer.
* `--filter none|minimal|aggressive` — keep / strip full-line comments /
  collapse impl and class bodies.

### `outline <path>`
Tree-sitter outline of one file.
* `--style agent|tabular|markdown|structured` — text rendering; `agent` is
  the default dense form (`[A-B]` column, bundled imports, drill hint).
  JSON is independent of `--style` and emits `kind`, `name`, `start_line`,
  `end_line`, `signature`, `children`.

### `symbol <name>`
AST-driven definition lookup (not substring). Trailing `*` is a prefix glob.
Comma-separated names emit `{ query, groups[], total_groups }`, one group
per symbol; duplicates are dropped (first-wins), entries trimmed.
`--expand` splits the token budget across hits in all groups.
JSON includes `facets: { total, by_kind }` computed on the full candidate
set (unaffected by `--offset`/`--limit`); text appends a `by kind:` line.

### `callers <name>`
References outside the definition site (bloom-narrowed, literal-confirmed).
* `--hops N` — caller-graph BFS (default 1, max 5).
* `--hub-guard N` — stop propagating from names exceeding N hits in one
  hop (default 50); hits still surface.
* `--count-by none|caller|file` — frequency table; JSON gains
  `aggregations: [{ key, count }]` (omitted when `none`).
* With `--hops > 1`, JSON may include `suspicious_hops`
  (`{ depth, name, roots[] }`) and `auto_hubs_promoted`
  (`{ depth, name, count }`), omitted when empty.

### `callees <name>`
Identifiers in `<name>`'s body, resolved to definitions. Hits with the same
`(name, path, line)` are de-duplicated.
* `--depth N` — callee-graph depth (default 1, max 5); with N > 1 each hit
  carries `depth` and `from`.
* `--hub-guard N` — same semantics and default as `callers`.

### `refs <name>`
Definitions (full, unpaginated) plus single-hop usages (paged by
`--limit`/`--offset`). JSON: `{ name, definitions, usages, total_usages,
offset, has_more }`; each usage carries `enclosing` when resolvable.

### `flow <name>`
One card per definition: def metadata, signature header, budget-capped body
excerpt, top-N callees, top-N callers. JSON:
`{ name, cards: [{ def, header, body, body_start_line, body_end_line,
kept_bytes, footer_bytes, callees, total_callees, callers, total_callers }],
total_cards, offset, has_more }`.
* `--callees-top N` (default 5), `--callers-top N` (default 5).
* `--budget N` — body bytes split across visible cards, 256-byte floor
  (default 10000).
* `--no-did-you-mean` — disable fuzzy/case/prefix suggestions on no-match.

### `siblings <name>`
Peer symbols in the same scope (methods of a class, functions in a file).
`--include-imports` adds the file's import block. Each hit carries `parent`
(`"<file>"` for top-level) and `target_path`/`target_line`.

### `map`
Workspace tree with per-directory file counts and token estimates;
gitignore-aware.
* `--depth N` (default 3) — deeper directories collapse to a summary line
  and JSON marks them `truncated: true`.
* `--max-file-bytes N` (default 1 MiB), `--bytes-per-token N` (default 4).
* `--symbols N` (default 0) — annotate file leaves with the top-N symbols
  by weight; JSON adds `symbols: [{ name, kind, line, weight }]`.

### `impact <symbol>`
Ranks files affected by a change:
`score = direct*3 + imports*2 + transitive`. JSON `results: [{ path, score,
reasons[] }]` sorted desc, ties alphabetical; `reasons` omits zero terms.
* `--hops 1|2|3` — transitive depth (default 3; 1 disables transitive).
* `--hub-guard N` (default 50), `--limit`/`--offset` (defaults 20/0).

### `index`
Build/refresh on-disk indexes (bigram, bloom, symbol, outline).

### `mcp`
Run the MCP server over stdio.

### `guide`
Print this document.

## Engine surfaces (Lua / C ABI)

`refs`/`flow`/`impact` are also exposed behind the additive `ffs` feature:
* Lua: `engine.refs(name, limit?, offset?)`,
  `engine.flow(name, {limit, offset, callees_top, callers_top, budget})`,
  `engine.impact(name, {limit, offset, hops, hub_guard})` — each returns the
  raw JSON payload.
* C ABI: `ffs_engine_refs`, `ffs_engine_flow`, `ffs_engine_impact`, returning
  `FfsEngineResponse`; free with `ffs_engine_free_response`.

Wrappers spawn `ffs` via `std::env::current_exe()`, so the loading binary
must be the `ffs` CLI. Builds default to off.

## Tips

* Listing commands return `total`, so you can widen, paginate, or refine
  without a second round-trip.
* `outline` gives kind/signature; `siblings` gives nearby definitions.
  Together they reconstruct an enclosing scope without re-reading the file.
* Prefer `--format json` for programmatic use; text may evolve, JSON is the
  contract.
* Use `--limit` aggressively: scans are independent of the limit, but output
  marshalling is not.
