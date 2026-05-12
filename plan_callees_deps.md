# Plan: --detailed Callees + Multi-language Import Resolution + Export/Import Anchors

**Constraint**: No changes to `crates/fff-core/`.

## 1. --detailed Callees (Call-site Args/Assignments)

### Summary
When `--detailed` flag is set on `scry callees`, show ordered call sites with arguments, assignment context, and return variable tracking.

### New File

#### `crates/fff-cli/src/commands/call_format.rs`
Port from srcwalk's `commands/call_format.rs`:

```rust
pub struct CallSite {
    pub line: u32,
    pub callee: String,
    pub call_text: String,
    pub call_prefix: Option<String>,
    pub args: Vec<String>,
    pub return_var: Option<String>,
    pub is_return: bool,
}

pub fn format_call_site(site: &CallSite) -> String;
fn format_call_with_args(site: &CallSite) -> String;
fn compact_arg(arg: &str) -> String;
```

Output format (from srcwalk):
```
L42 result = foo(arg1=x, arg2=y)
L88 bar()           // no assignment, bare call
L91 ->ret baz(ctx)  // return statement
```

### New File

#### `crates/fff-cli/src/commands/call_site_extract.rs`
Extract call-site details from tree-sitter AST:

```rust
/// Extract detailed call-site info from a code region.
pub fn extract_call_sites(
    content: &str,
    lang: Lang,
    start_line: u32,
    end_line: u32,
    known_symbols: &HashSet<String>,
) -> Vec<CallSite>;
```

Logic per call expression node:
1. Get callee name (skip method chains, use rightmost identifier).
2. Extract arguments: child nodes of `arguments`/`argument_list`.
3. Check if call is in a `return` statement (`is_return`).
4. Check assignment context: `let x = foo()` → `return_var = "x"`.
5. Only include calls to symbols in `known_symbols` (from symbol index).

### Modified Files

#### `crates/fff-cli/src/commands/callees.rs`
- Add `--detailed` flag:
```rust
#[arg(long, default_value_t = false)]
pub detailed: bool,
```
- In `single_hop()`: when `--detailed`, call `extract_call_sites()` instead of just `collect_callees()`.
- Output format switches to `format_call_site()` rendering.

#### `crates/fff-mcp/src/server.rs`
- Add `detailed: bool` to `scry_callees` tool parameters.

### Implementation Steps
1. Create `call_format.rs` — formatting functions. Complexity: **Low** (~50 lines, direct port).
2. Create `call_site_extract.rs` — tree-sitter AST walk. Complexity: **Medium-High** (~150 lines, needs per-language AST handling for call expressions, arguments, assignments, return statements).
3. Wire into callees command. Complexity: **Low** (~30 lines).
4. MCP integration. Complexity: **Low** (~10 lines).

**Estimated: ~240 lines**

---

## 2. Multi-language Import Resolution (Enhanced)

### Summary
Enhance scry's existing `deps_resolve.rs` to match srcwalk's coverage: Rust (`use crate::`, `self::`, `super::`), JS/TS (ESM, CJS, re-exports), Python (`from . import`), C/C++ (`#include`), and Elixir.

### Current State
Scry already has `crates/fff-cli/src/commands/deps_resolve.rs` with `extract_imports()` and `resolve_import()`. Need to read it to determine what's already covered.

### Modified Files

#### `crates/fff-cli/src/commands/deps_resolve.rs`
Enhance with missing resolvers from srcwalk's `read/imports.rs`:

**Rust** (likely partially done):
- Add `try_rust_path()` — progressive path shortening: `cache::OutlineCache` → try `cache/OutlineCache.rs` then `cache.rs`.
- Add `find_src_ancestor()` — walk up to find `src/` directory for `crate::` resolution.

**JS/TS** (likely partially done):
- Add `resolve_js_source_extension()` — `.js` import resolves to `.ts`/`.tsx` source.
- Add index resolution: `./utils` → `utils/index.ts` or `utils/index.js`.
- Add re-export detection: `export { foo } from "./bar"` counts as import.

**Python**:
- Add `resolve_python()` — handle `from .. import` (multi-dot parent traversal).
- Module to path mapping: `foo.bar` → `foo/bar.py` or `foo/bar/__init__.py`.

**C/C++**:
- Add `resolve_c_include()` — `#include "header.h"` resolved relative to file dir.

**Elixir** (new):
- Add detection of `alias`, `import`, `use`, `require` lines.

### Implementation Steps
1. Read current `deps_resolve.rs` to identify gaps. Complexity: **Low** (read-only).
2. Port missing resolvers from srcwalk. Complexity: **Medium** (~120 lines across all languages).
3. Add tests for each resolver. Complexity: **Low** (~60 lines).

**Estimated: ~180 lines**

---

## 3. Export/Import Anchor Extraction

### Summary
Extract ES, CommonJS, UMD, and AMD export names from JS/TS files. This overlaps with the artifact mode plan — the extraction functions live in `fff-symbol/src/artifact.rs` (see plan_artifact_mode.md).

### Dependency
This feature requires the artifact mode anchor extraction to be implemented first (Plan 1). The `extract_artifact_anchors()` and `extract_export_names()` functions will be shared.

### Modified Files

#### `crates/fff-cli/src/commands/deps.rs`
- In `imports_for_file()`: for JS/TS files, also include export anchors from `artifact::extract_artifact_anchors()`.
- Export names become additional "pseudo-imports" showing what the file exposes.

#### `crates/fff-engine/src/dispatch.rs`
- When classifying a JS/TS symbol query, check artifact anchors as a fallback source for symbol locations.

### Implementation Steps
1. Depends on artifact mode (Plan 1).
2. Wire anchor extraction into deps command. Complexity: **Low** (~20 lines).
3. Wire into dispatch for JS/TS symbol lookup. Complexity: **Low** (~15 lines).

**Estimated: ~35 lines**

---

## Total Estimated: ~455 lines new code

## Implementation Order
1. Enhanced import resolution (independent, improves deps immediately)
2. --detailed callees (call-format + call-site extraction)
3. Export/Import anchors (depends on artifact mode from Plan 1)
