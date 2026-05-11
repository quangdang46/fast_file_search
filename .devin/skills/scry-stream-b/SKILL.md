---
name: scry-stream-b-workflow
description: How to ship PRs against quangdang46/scry — push/PR workflow without the Devin GitHub integration, lock zones for the Stream B feature plan, and the standard lint/test commands used by every PR.
---

# Working on `quangdang46/scry`

This repo houses **FFF.nvim** (the Neovim file finder) plus the `scry`
workspace of crates that build on top of it. This skill captures the
non-obvious workflow knowledge needed to ship PRs here cleanly.

## Push / PR workflow — Devin GitHub integration NOT installed

As of this session, `quangdang46/scry` does **not** have the Devin
GitHub integration installed, so `git push` through the Devin proxy
returns `403 Forbidden`. The org has a `ghtoken` secret available
instead — use it for both `git push` and the GitHub REST API.

Push a branch:

```bash
git -c "url.https://x-access-token:${ghtoken}@github.com/.pushinsteadof=https://github.com/" \
    -c "url.https://x-access-token:${ghtoken}@github.com/.pushinsteadof=https://git-manager.devin.ai/proxy/github.com/" \
    push -u origin "$(git branch --show-current)"
```

Create a PR via REST (the `git_pr` tool's create action also fails
because of the proxy):

```bash
curl -sS -X POST \
  -H "Authorization: Bearer ${ghtoken}" \
  -H "Accept: application/vnd.github+json" \
  -H "X-GitHub-Api-Version: 2022-11-28" \
  https://api.github.com/repos/quangdang46/scry/pulls \
  -d '{ "title": "...", "body": "...", "head": "<branch>", "base": "main" }'
```

For PR introspection (`git_pr(action="view_pr")`, `pr_checks`,
`ci_job_logs`) and for `git_pr(action="add_labels")` etc., the builtin
tools still work — the proxy only blocks push and PR creation.

If the user can install the Devin GitHub integration
(<https://app.devin.ai/settings/integrations/github>) for this repo,
future sessions can drop the manual workaround.

## Standard lint + test commands

Every PR in this repo runs:

```bash
cargo fmt --all
cargo clippy --workspace --features zlob --exclude fff-nvim -- -D warnings
cargo test  --workspace --features zlob --exclude fff-nvim
```

`fff-nvim` is excluded because it links against `mlua` and pulls in the
Lua C library, which the default sandbox does not have. To clippy-check
the Lua bindings crate, run separately:

```bash
cargo clippy -p fff-nvim --features scry -- -D warnings
```

For the C FFI feature gate:

```bash
cargo build -p fff-c --features scry
```

The Makefile aliases (`make lint`, `make format`, `make test`) wrap the
same commands.

## Lock zones (do NOT modify the public surfaces of these crates)

The Stream B feature plan defines explicit lock zones — public APIs
that must stay byte-identical across the stream. Any new feature
touching these zones must be **purely additive** (new symbol / new
tool / new export / new sub-command), never a rename or signature
change:

* `fff-core`, `fff-grep`, `fff-query-parser` — the picker / search
  internals; B3–B6 work was kept out of these entirely.
* `fff-mcp` — the 3 original tools (`find_files`, `grep`, `multi_grep`)
  plus the first batch of `scry_*` tools (`scry_dispatch`, `scry_symbol`,
  `scry_callers`, `scry_callees`, `scry_read`). New tools are fine; do
  not rename or change parameter shapes on the existing ones.
* `fff-nvim` — the public Lua API exposed by `lib.rs::create_exports`.
  The B7 wiring added new `scry_*` exports inside the existing
  `#[cfg(feature = "scry")]` block; everything else stays as-is.
* `fff-c` — the default ABI (everything in `fff.h` not guarded by
  `#if defined(FFF_SCRY)`). New FFI symbols must be feature-gated
  behind the `scry` Cargo feature **and** `FFF_SCRY` on the C side.
* `lua/fff/*.lua` — the picker UI. Anything UI-related must keep
  navigation / select / preview working unchanged.

Free zones (modify freely): `crates/fff-cli`, `crates/fff-engine`,
`crates/fff-budget`, `crates/fff-symbol`, the on-disk index format
(behind a version bump).

## Scry CLI conventions

The `scry` binary's sub-commands all follow the same patterns:

* Top-level flags: `--root <path>` (default `cwd`), `--format <text|json>`
  (default `text`).
* Pagination flags on listing commands: `--limit N` / `--offset N`,
  with `total` / `has_more` in the JSON payload.
* BFS-based commands (`callers`, `impact`): `--hops <1|2|3>` and
  `--hub-guard <N>` (default 50) for fan-out control.
* Aggregation flags: e.g. `callers --count-by <none|caller|file>`
  appends an `aggregations: [{ key, count }]` field to JSON only when
  enabled.
* Output stays byte-identical when an opt-in flag is absent — the
  caller's contract is never broken by adding new optional fields.

## MCP / Lua / C wiring pattern (B7)

When exposing a new CLI sub-command through MCP / Lua / C, the
cheapest add-only path is to **shell out to the scry binary** via
`std::env::current_exe()` (the MCP server runs as `scry mcp`, so
`current_exe` IS the scry CLI). Each wrapper just builds an
`std::process::Command`, adds `--root <root> --format json <subcmd>`,
appends the flags, and returns stdout. This keeps logic centralised in
the CLI and avoids duplicating BFS / pagination / dedup work in three
places. The MCP camelCase convention (`maxResults`, `calleesTop`,
`callersTop`, `hubGuard`) maps onto the CLI's kebab-case flags inside
the wrapper.

## Reference repos

When designing call-graph / impact features, the maintainer cites:

* <https://github.com/sting8k/srcwalk>
* <https://github.com/rtk-ai/rtk>

as prior art for symbol-index + BFS-based code navigation. Read these
before proposing significant changes to `callers_bfs.rs` or `impact.rs`.
