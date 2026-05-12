# Plan: JS/TS Artifact Mode

**Constraint**: No changes to `crates/fff-core/`.

## Summary

Port srcwalk's artifact mode: support for reading/searching compiled/bundled JS/TS files with export anchor extraction, byte-range section reads, and IIFE detection.

## New Files

### `crates/fff-symbol/src/artifact.rs`
Core artifact detection and extraction module.

```rust
pub struct ArtifactAnchor {
    pub line: u32,
    pub kind: &'static str, // "export" | "mod"
    pub name: String,
}

pub fn is_artifact_js_ts_file(path: &Path) -> bool;
pub fn extract_artifact_anchors(content: &str) -> (Vec<ArtifactAnchor>, usize /* total */);
pub fn search_anchor_matches(content: &str, query: &str) -> Vec<ArtifactAnchor>;
pub fn read_js_ts_symbol_section(path: &Path, symbol: &str, budget: Option<u64>) -> Option<Result<String>>;
```

Internal helpers (ported from srcwalk's `artifact.rs`):
- `extract_es_export_names(line)` — ES module `export { ... }`, `export default/function/class/const/let/var`
- `extract_named_commonjs_exports(line)` — `module.exports.X`, `exports.X`
- `extract_umd_global_exports(line)` — UMD wrapper global assignments
- `extract_named_amd_modules(line)` — `define("name", ...)`
- `clean_export_name(text)` — identifier extraction up to non-alphanum
- `find_js_ts_symbol_span(content, lang, symbol)` — tree-sitter AST symbol location
- `render_artifact_symbol_section(...)` — formatted output with byte span metadata
- `render_artifact_byte_section(...)` — raw byte-range section rendering
- `compact_artifact_text(text, max_chars)` — truncate with `…` marker

### `crates/fff-symbol/src/artifact_test.rs` (optional)
Unit tests for export extraction covering ES, CJS, UMD, AMD patterns.

## Modified Files

### `crates/fff-symbol/src/lib.rs`
- Add `pub mod artifact;`

### `crates/fff-cli/src/commands/read.rs`
- Add `--artifact` flag to `Args`
- In `run()`: after outline/full routing, if `--artifact` and file is JS/TS, call `artifact::add_anchors()` to inject anchor block into output
- Wire `bytes:start-end` syntax in `parse_target()` to route to byte-range section reads

### `crates/fff-cli/src/commands/callees.rs`
- Add `--artifact` flag
- In `single_hop()`: when `--artifact`, use `artifact::read_js_ts_symbol_section()` for artifact-mode callees

### `crates/fff-cli/src/commands/flow.rs`
- Add `--artifact` flag
- Pass through to callees/callers invocations

### `crates/fff-cli/src/commands/deps.rs`
- In `imports_for_file()`: for JS/TS files, also extract artifact anchors as pseudo-imports via `artifact::extract_artifact_anchors()`

### `crates/fff-mcp/src/server.rs`
- Add `artifact: bool` parameter to `scry_read`, `scry_callees`, `scry_flow` tools
- Pass through to CLI commands

## Implementation Steps

1. **Create `crates/fff-symbol/src/artifact.rs`** — port anchor extraction functions from srcwalk's `artifact.rs` lines 317-545 (export extractors, AMD, UMD, clean_export_name). Complexity: **Medium** (~200 lines, mostly string parsing, no dependencies).

2. **Add symbol span finding** — port `find_js_ts_symbol_span`, `ArtifactSymbolSpan`, and `is_named_js_ts_symbol` from srcwalk's artifact.rs lines 68-162. Uses existing tree-sitter grammars in fff-symbol. Complexity: **Medium** (~80 lines).

3. **Add rendering functions** — port `render_artifact_symbol_section` and `render_artifact_byte_section`. Complexity: **Low** (~60 lines).

4. **Wire into read command** — add `--artifact` flag, anchor injection. Complexity: **Low** (~30 lines).

5. **Wire into callees/flow** — pass artifact mode through. Complexity: **Low** (~20 lines per command).

6. **Wire into deps** — extract anchors as pseudo-imports. Complexity: **Low** (~15 lines).

7. **MCP integration** — add artifact parameter. Complexity: **Low** (~10 lines per tool).

## Dependencies
- No new crate dependencies. Reuses existing tree-sitter infrastructure in `fff-symbol`.

## Estimated Total: ~450 lines new code
