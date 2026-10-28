# Verification artifacts

Produced 2026-10-29 against base commit `d34576d4`.

| File | What it proves |
|---|---|
| `report.txt` | `cargo build` / `cargo test` / `clippy -D warnings` / `fmt --check` with exit codes |
| `api_compat.json` | CLI flags, MCP tool schemas, C exports compared base-vs-branch (`verdict: PASS`) |
| `json_equivalence.txt` | 35 CLI command/flag combinations produce byte-identical JSON |
| `verify_api.py` | Regenerates `api_compat.json`; exits non-zero on any lost tool, schema property, required field, or C export |

The C ABI baseline is read from git (`git show <rev>:crates/ffs-c/src/...`)
rather than vendored into this directory, so it cannot drift or rot.

## Reproduce

```bash
# A binary built from the base commit to compare against:
git stash && cargo build -p ffs-cli && cp target/debug/ffs /tmp/ffs-before && git stash pop
cargo build -p ffs-cli

python .verification/verify_api.py \
    /tmp/ffs-before ./target/debug/ffs <fixture-dir> "$PWD" origin/main
```
