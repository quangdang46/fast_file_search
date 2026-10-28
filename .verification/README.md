# Verification artifacts

Produced on 2026-10-29 against base commit `d34576d4`.

| File | What it proves |
|---|---|
| `report.txt` | `cargo build` / `cargo test` / `clippy -D warnings` / `fmt --check` output with exit codes |
| `api_compat.json` | CLI flags, MCP tool schemas, and C exports compared before/after (`verdict: PASS`) |
| `json_equivalence.txt` | 35 CLI command/flag combinations produce byte-identical JSON |
| `verify_api.py` | Script that regenerates `api_compat.json` |
| `c_before.rs`, `engine_before.rs` | Base-commit C ABI sources used as the comparison baseline |

## Reproduce

```bash
git stash                                  # base state
cargo build -p ffs-cli && cp target/debug/ffs /tmp/ffs-before
git stash pop
cargo build -p ffs-cli

python .verification/verify_api.py /tmp/ffs-before ./target/debug/ffs <fixture-dir> "$PWD"
```

`verify_api.py` exits non-zero when any tool is missing, a schema property is
lost, a required set changes, or a C export disappears.
