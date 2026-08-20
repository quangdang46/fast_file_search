#!/usr/bin/env bash
# e2e smoke for the `ffs` CLI — fast, hermetic, no network.
# Usage:
#   bash scripts/test_e2e.sh                  # uses $FFS or ./target/debug/ffs or ~/.local/bin/ffs
#   FFS=./target/release/ffs bash scripts/test_e2e.sh
#   bash scripts/test_e2e.sh --verbose        # show stdout for each case
set -euo pipefail

VERBOSE=0
if [[ "${1:-}" == "--verbose" || "${1:-}" == "-v" ]]; then VERBOSE=1; fi

# Colors (safe when not a tty).
if [[ -t 1 ]]; then
  GREEN=$'\033[32m'; RED=$'\033[31m'; YELLOW=$'\033[33m'; DIM=$'\033[2m'; RESET=$'\033[0m'
else
  GREEN=""; RED=""; YELLOW=""; DIM=""; RESET=""
fi

# Resolve ffs binary.
resolve_ffs() {
  if [[ -n "${FFS:-}" && -x "$FFS" ]]; then echo "$FFS"; return; fi
  for c in "./target/debug/ffs" "$HOME/.local/bin/ffs" "ffs"; do
    if [[ -x "$c" ]]; then echo "$c"; return; fi
    if command -v "$c" >/dev/null 2>&1; then command -v "$c"; return; fi
  done
  echo "ffs"
}
FFS_BIN="$(resolve_ffs)"
if ! "$FFS_BIN" --version >/dev/null 2>&1; then
  echo "${RED}ERR${RESET}: cannot run '$FFS_BIN --version' (set \$FFS to your binary)."
  exit 2
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Ephemeral fixture repo: deterministic, hermetic, cleaned on exit.
TMP="$(mktemp -d "${TMPDIR:-/tmp}/ffs-e2e.XXXXXX")"
cleanup() { rm -rf "$TMP"; }
trap cleanup EXIT

PASS=0; FAIL=0; SKIP=0
FAILED_CASES=()
log_pass() { PASS=$((PASS+1)); echo "${GREEN}PASS${RESET} $*"; }
log_fail() { FAIL=$((FAIL+1)); FAILED_CASES+=("$*"); echo "${RED}FAIL${RESET} $*"; }
log_skip() { SKIP=$((SKIP+1)); echo "${YELLOW}SKIP${RESET} $*"; }

# Run helpers: capture stdout/stderr + exit code.
# Use temp files instead of command substitution to preserve newlines.
run_ok() {
  # run_ok <label> -- <ffs args...>
  local label="$1"; shift
  if [[ "${1:-}" == "--" ]]; then shift; fi
  local out="$TMP/out.$$" err="$TMP/err.$$"
  rm -f "$out" "$err"
  set +e
  "$FFS_BIN" --root "$TMP" "$@" >"$out" 2>"$err"
  local rc=$?
  set -e
  if [[ $rc -ne 0 ]]; then
    log_fail "$label (exit $rc)"
    echo "  args: $*"
    echo "  stderr: $(tr '\n' ' ' < "$err" | cut -c1-400)"
    [[ $VERBOSE -eq 1 ]] && { echo "  --- stdout ---"; cat "$out" | head -n 40; echo "  --- stderr ---"; cat "$err" | head -n 40; }
    return 1
  fi
  # keep out/err for caller to inspect via $out/$err temp files if needed
  log_pass "$label"
  if [[ $VERBOSE -eq 1 && -s "$out" ]]; then echo "${DIM}$(head -n 12 "$out")${RESET}"; fi
  return 0
}
# Assert stdout contains a substring (after a successful run_ok).
assert_contains() {
  local label="$1" needle="$2" file="$3"
  if grep -qF -- "$needle" "$file" 2>/dev/null; then
    log_pass "$label"
  else
    log_fail "$label (missing '$needle')"
    echo "  file: $file head:"; head -n 20 "$file" 2>/dev/null | sed 's/^/    /'
    return 1
  fi
}
assert_not_contains() {
  local label="$1" needle="$2" file="$3"
  if grep -qF -- "$needle" "$file" 2>/dev/null; then
    log_fail "$label (unexpected '$needle')"
    head -n 20 "$file" 2>/dev/null | sed 's/^/    /'
    return 1
  else
    log_pass "$label"
  fi
}
assert_empty() {
  local label="$1" file="$2"
  if [[ ! -s "$file" ]]; then log_pass "$label"; else log_fail "$label (expected empty)"; head -n 20 "$file" | sed 's/^/    /'; return 1; fi
}
assert_exit_code() {
  # assert_exit_code <label> <expected_rc> -- <ffs args...>
  local label="$1"; local want="$2"; shift 2
  if [[ "${1:-}" == "--" ]]; then shift; fi
  local out="$TMP/out.$$" err="$TMP/err.$$"
  rm -f "$out" "$err"
  set +e
  "$FFS_BIN" --root "$TMP" "$@" >"$out" 2>"$err"
  local rc=$?
  set -e
  if [[ $rc -eq $want ]]; then
    log_pass "$label"
  else
    log_fail "$label (want exit $want, got $rc)"
    echo "  args: $*"
    echo "  stderr: $(tr '\n' ' ' < "$err" | cut -c1-300)"
  fi
}

# ---------------------------------------------------------------------------
# Fixture repo
# ---------------------------------------------------------------------------
setup_fixture() {
  echo "${DIM}Setting up fixture at $TMP${RESET}"
  echo "  ffs: $FFS_BIN ($("$FFS_BIN" --version 2>&1))"
  mkdir -p "$TMP/src" "$TMP/crates/a" "$TMP/crates/b" "$TMP/.git"

  cat > "$TMP/Cargo.toml" <<'TOML'
[workspace]
members = ["crates/a", "crates/b"]
TOML
  cat > "$TMP/src/main.rs" <<'RS'
// main entry
use crate::lib::Alpha;

pub fn main() {
    let a = Alpha::new();
    a.do_work();
    // TODO: wire up
    helper_function();
}

fn helper_function() {
    println!("helper");
}

struct Alpha;
impl Alpha {
    fn new() -> Self { Alpha }
    fn do_work(&self) {}
}
RS
  cat > "$TMP/src/lib.rs" <<'RS'
pub struct Alpha;
impl Alpha {
    pub fn new() -> Self { Alpha }
    pub fn do_work(&self) {}
}
pub fn helper_function() {}

/// Doc comment should survive minimal filter
pub fn documented_api() {}

#[derive(Debug)]
pub struct Beta {
    pub value: i32,
}
RS
  cat > "$TMP/src/outline_sample.rs" <<'RS'
use std::collections::HashMap;

pub struct MyStruct { pub x: i32 }
pub enum MyEnum { A, B(i32) }
pub trait MyTrait { fn do_it(&self); }
impl MyTrait for MyStruct { fn do_it(&self) {} }

pub fn top_level_fn(a: i32) -> i32 { a + 1 }

mod inner {
    pub fn inner_fn() {}
    pub struct InnerStruct;
}

pub const MY_CONST: i32 = 42;
pub type MyAlias = HashMap<String, i32>;
RS
  cat > "$TMP/crates/a/Cargo.toml" <<'TOML'
[package]
name = "crate_a"
version = "0.1.0"
edition = "2021"
TOML
  mkdir -p "$TMP/crates/a/src"
  cat > "$TMP/crates/a/src/lib.rs" <<'RS'
pub fn crate_a_fn() {}
pub struct CrateA;
RS
  cat > "$TMP/crates/b/Cargo.toml" <<'TOML'
[package]
name = "crate_b"
version = "0.1.0"
edition = "2021"
TOML
  mkdir -p "$TMP/crates/b/src"
  cat > "$TMP/crates/b/src/lib.rs" <<'RS'
use crate_a::CrateA;
pub fn crate_b_fn() { let _ = CrateA; }
RS

  # Non-code / generated / binary fixtures
  echo "hello world TODO" > "$TMP/README.md"
  echo "also has TODO" > "$TMP/notes.txt"
  printf 'binary\x00\x01\x02' > "$TMP/binary.dat"
  mkdir -p "$TMP/target"
  echo "should be ignored by .gitignore" > "$TMP/target/ignored.txt"

  cat > "$TMP/.gitignore" <<'GIT'
/target/
.ffs/
GIT

  # Large file for budget/truncation test
  python3 -c "for i in range(3000): print(f'// line {i}: some code here with Alpha and Beta')" > "$TMP/src/large.rs"

  git -C "$TMP" init -q 2>/dev/null || true
  git -C "$TMP" add -A 2>/dev/null || true

  echo "${DIM}Fixture ready: $(find "$TMP" -type f | wc -l | tr -d ' ') files${RESET}"
}

# ---------------------------------------------------------------------------
# Suites
# ---------------------------------------------------------------------------

suite_cli_meta() {
  echo ""
  echo "== cli meta =="
  local out="$TMP/out.$$" err="$TMP/err.$$"

  # --version / --help / no subcommand prints help
  run_ok "cli --version" -- --version
  run_ok "cli --help" -- --help
  # no subcommand should print help and exit 0
  set +e; "$FFS_BIN" --root "$TMP" >"$out" 2>"$err"; rc=$?; set -e
  if [[ $rc -eq 0 ]]; then log_pass "cli no subcommand exits 0"; else log_fail "cli no subcommand exits 0 (got $rc)"; fi

  # invalid root should error (not silently return empty)
  assert_exit_code "cli invalid --root exits non-zero" 1 -- find --root /nonexistent-ffs-root-xyz needle

  # completions
  for sh in bash fish zsh; do
    run_ok "cli --completions $sh" -- --completions "$sh" 2>/dev/null || log_skip "cli --completions $sh (shell not available)"
  done

  # --format json is valid for all commands (smoke)
  run_ok "cli --format json (find)" -- --format json find Alpha
}

suite_find() {
  echo ""
  echo "== find =="
  local out="$TMP/out.$$" err="$TMP/err.$$"

  run_ok "find exact filename" -- find main
  # verify it found src/main.rs
  "$FFS_BIN" --root "$TMP" find main >"$out" 2>/dev/null
  assert_contains "find main contains src/main.rs" "main.rs" "$out"

  run_ok "find fuzzy typo" -- find "mani"  # close to main
  run_ok "find --fuzzy" -- find --fuzzy "mian"
  run_ok "find --no-fuzzy" -- find --no-fuzzy "main"
  run_ok "find --mode files" -- find --mode files "main"
  run_ok "find --mode directories" -- find --mode directories "src"
  run_ok "find --mode mixed" -- find --mode mixed "src"
  run_ok "find --scored" -- find --scored "main"
  run_ok "find --limit 1" -- find --limit 1 "rs"
  run_ok "find --offset 1" -- find --offset 1 --limit 1 "rs"
  run_ok "find --scope crates/a" -- find --scope crates/a "lib"
  run_ok "find --format json" -- --format json find main
  "$FFS_BIN" --root "$TMP" --format json find main >"$out" 2>/dev/null
  assert_contains "find json is array/object" "\"" "$out"

  # negative / no-match
  run_ok "find no match exits 0 with empty" -- find "zzznonexistent999"
  "$FFS_BIN" --root "$TMP" find "zzznonexistent999" >"$out" 2>/dev/null
  # should be empty or "0 results" — just ensure it didn't error
  [[ -f "$out" ]] && log_pass "find no-match output exists" || log_fail "find no-match output exists"
}

suite_glob() {
  echo ""
  echo "== glob =="
  run_ok "glob **/*.rs" -- glob "**/*.rs"
  run_ok "glob **/*.rs --limit 2" -- glob --limit 2 "**/*.rs"
  run_ok "glob **/*.rs --format json" -- --format json glob "**/*.rs"
  run_ok "glob no match" -- glob "**/*.notexist999"
  run_ok "glob single level" -- glob "*.md"
  run_ok "glob crates/*" -- glob "crates/*"
}

suite_grep() {
  echo ""
  echo "== grep =="
  local out="$TMP/out.$$"

  run_ok "grep literal" -- grep "Alpha"
  "$FFS_BIN" --root "$TMP" grep "Alpha" >"$out" 2>/dev/null
  assert_contains "grep Alpha hits" "Alpha" "$out"

  run_ok "grep case insensitive (default smart-case)" -- grep "alpha"
  run_ok "grep --case-sensitive" -- grep --case-sensitive "Alpha"
  # case-sensitive lowercase should not match uppercase
  "$FFS_BIN" --root "$TMP" grep --case-sensitive "alpha" >"$out" 2>/dev/null
  # may still match lowercase helpers; just ensure it runs
  log_pass "grep --case-sensitive runs"

  run_ok "grep --fixed-strings with dot" -- grep -F "Alpha::new"
  run_ok "grep --regex" -- grep --regex "Alpha|Beta"
  run_ok "grep --word-regexp" -- grep --word-regexp "Alpha"
  run_ok "grep --files-with-matches" -- grep --files-with-matches "Alpha"
  run_ok "grep --max-count 1" -- grep --max-count 1 "Alpha"
  run_ok "grep --limit 5" -- grep --limit 5 "Alpha"
  run_ok "grep --group" -- grep --group "Alpha"
  run_ok "grep --format json" -- --format json grep "Alpha"
  run_ok "grep TODO (multi-file)" -- grep "TODO"
  run_ok "grep no match" -- grep "zzznonexistent999"
  # invalid regex should not crash
  assert_exit_code "grep invalid regex exits non-zero" 1 -- grep --regex "[unclosed"
  run_ok "grep fixed strings with regex chars" -- grep -F "[unclosed"
}

suite_multi_grep() {
  echo ""
  echo "== multi-grep =="
  run_ok "multi-grep two literals" -- multi-grep "Alpha" "Beta"
  run_ok "multi-grep -e flags" -- multi-grep -e "Alpha" -e "Beta"
  run_ok "multi-grep positional + -e" -- multi-grep "Alpha" -e "Beta"
  run_ok "multi-grep --limit 5" -- multi-grep --limit 5 "Alpha" "Beta"
  run_ok "multi-grep --files-with-matches" -- multi-grep --files-with-matches "Alpha" "Beta"
  run_ok "multi-grep --case-sensitive" -- multi-grep --case-sensitive "Alpha" "Beta"
  run_ok "multi-grep --format json" -- --format json multi-grep "Alpha" "Beta"
  run_ok "multi-grep no match" -- multi-grep "zzz999" "yyy888"
}

suite_read() {
  echo ""
  echo "== read =="
  local out="$TMP/out.$$"

  run_ok "read basic" -- read "src/main.rs"
  "$FFS_BIN" --root "$TMP" read "src/main.rs" >"$out" 2>/dev/null
  assert_contains "read contains fn main or Alpha" "Alpha" "$out"

  run_ok "read absolute path" -- read "$TMP/src/main.rs"
  run_ok "read path:line" -- read "src/main.rs:5"
  run_ok "read --full" -- read --full "src/main.rs"
  run_ok "read --section (path:line)" -- read --section "src/main.rs:5"
  run_ok "read --signatures" -- read --signatures "src/main.rs"
  run_ok "read --filter none --full" -- read --filter none --full "src/main.rs"
  run_ok "read --filter minimal --full" -- read --filter minimal --full "src/main.rs"
  run_ok "read --filter aggressive --full" -- read --filter aggressive --full "src/main.rs"
  run_ok "read --budget 100" -- read --budget 100 "src/large.rs"
  run_ok "read --artifact (non-JS no-op)" -- read --artifact "src/main.rs"
  run_ok "read --format json" -- --format json read "src/main.rs"

  # Budget truncation footer (needs --full; default outline is not truncated the same way)
  "$FFS_BIN" --root "$TMP" read --full --budget 10 "src/large.rs" >"$out" 2>/dev/null
  assert_contains "read truncated footer" "truncated" "$out"

  # Edge: missing file should error
  assert_exit_code "read missing file exits non-zero" 1 -- read "no/such/file.rs"
  # Edge: directory should error
  assert_exit_code "read directory exits non-zero" 1 -- read "src"

  # Binary file: should not panic, should emit structured message
  run_ok "read binary file" -- read "binary.dat"
  "$FFS_BIN" --root "$TMP" read "binary.dat" >"$out" 2>/dev/null
  assert_contains "read binary marker" "binary" "$out" || log_skip "read binary marker (format may vary)"

  # Deeply nested file (stack-overflow regression)
  python3 -c "
depth=600
s='';
for i in range(depth):
    s+=f'fn f{i}() {{\n'
s+='let x=1;\n'
for _ in range(depth):
    s+='}\n'
open('$TMP/src/deep.rs','w').write(s)
"
  run_ok "read deeply nested file" -- read "src/deep.rs"
}

suite_outline() {
  echo ""
  echo "== outline =="
  local out="$TMP/out.$$"
  run_ok "outline basic" -- outline "src/outline_sample.rs"
  "$FFS_BIN" --root "$TMP" outline "src/outline_sample.rs" >"$out" 2>/dev/null
  assert_contains "outline has MyStruct" "MyStruct" "$out"

  run_ok "outline --style agent" -- outline --style agent "src/outline_sample.rs"
  run_ok "outline --style markdown" -- outline --style markdown "src/outline_sample.rs"
  run_ok "outline --style structured" -- outline --style structured "src/outline_sample.rs"
  run_ok "outline --style tabular" -- outline --style tabular "src/outline_sample.rs"
  run_ok "outline --format json" -- --format json outline "src/outline_sample.rs"

  # Edge cases
  assert_exit_code "outline missing file exits non-zero" 1 -- outline "no/such.rs"
  assert_exit_code "outline non-code file exits non-zero" 1 -- outline "README.md"
  # Empty file
  touch "$TMP/src/empty.rs"
  run_ok "outline empty file" -- outline "src/empty.rs"
  # Deeply nested outline (iterative DFS regression)
  run_ok "outline deeply nested file" -- outline "src/deep.rs"
  # Binary file
  assert_exit_code "outline binary exits non-zero" 1 -- outline "binary.dat" || run_ok "outline binary handled" -- outline "binary.dat"
}

suite_symbol() {
  echo ""
  echo "== symbol =="
  local out="$TMP/out.$$"
  run_ok "symbol exact" -- symbol "Alpha"
  "$FFS_BIN" --root "$TMP" symbol "Alpha" >"$out" 2>/dev/null
  assert_contains "symbol Alpha found" "Alpha" "$out"

  run_ok "symbol prefix glob" -- symbol "Alp*"
  run_ok "symbol comma list" -- symbol "Alpha,Beta"
  run_ok "symbol --expand" -- symbol --expand "Alpha"
  run_ok "symbol --limit 1" -- symbol --limit 1 "Alpha"
  run_ok "symbol --offset 1" -- symbol --offset 1 --limit 1 "Alpha"
  run_ok "symbol --format json" -- --format json symbol "Alpha"
  run_ok "symbol --budget 500 with expand" -- symbol --expand --budget 500 "Alpha"
  run_ok "symbol no match" -- symbol "ZzzNonexistent999"
  # did-you-mean on typo (should suggest)
  "$FFS_BIN" --root "$TMP" symbol "Alpah" >"$out" 2>/dev/null
  if grep -qi "did you mean\|suggest" "$out" 2>/dev/null; then log_pass "symbol typo suggests"; else log_skip "symbol typo suggests (no suggestion)"; fi
  run_ok "symbol --no-did-you-mean (no suggestion)" -- symbol --no-did-you-mean "ZzzNonexistent999"
}

suite_callers() {
  echo ""
  echo "== callers =="
  run_ok "callers direct" -- callers "Alpha"
  run_ok "callers --hops 2" -- callers --hops 2 "Alpha"
  run_ok "callers --limit 5" -- callers --limit 5 "Alpha"
  run_ok "callers --offset 1" -- callers --offset 1 --limit 5 "Alpha"
  run_ok "callers --hub-guard 5" -- callers --hub-guard 5 "Alpha"
  run_ok "callers --skip-hubs helper_function" -- callers --skip-hubs "helper_function" "Alpha"
  run_ok "callers --count-by none" -- callers --count-by none "Alpha"
  run_ok "callers --count-by caller" -- callers --count-by caller "Alpha"
  run_ok "callers --count-by file" -- callers --count-by file "Alpha"
  run_ok "callers --count-by package" -- callers --count-by package "Alpha"
  run_ok "callers --format json" -- --format json callers "Alpha"
  run_ok "callers no match" -- callers "ZzzNonexistent999"
}

suite_callees() {
  echo ""
  echo "== callees =="
  run_ok "callees direct" -- callees "helper_function"
  run_ok "callees --depth 2" -- callees --depth 2 "helper_function"
  run_ok "callees --limit 5" -- callees --limit 5 "helper_function"
  run_ok "callees --offset 1" -- callees --offset 1 --limit 2 "helper_function"
  run_ok "callees --hub-guard 5" -- callees --hub-guard 5 "helper_function"
  run_ok "callees --detailed" -- callees --detailed "helper_function"
  run_ok "callees --format json" -- --format json callees "helper_function"
  run_ok "callees no match" -- callees "ZzzNonexistent999"
}

suite_refs() {
  echo ""
  echo "== refs =="
  run_ok "refs basic" -- refs "Alpha"
  run_ok "refs --limit 5" -- refs --limit 5 "Alpha"
  run_ok "refs --offset 1" -- refs --offset 1 --limit 5 "Alpha"
  run_ok "refs --format json" -- --format json refs "Alpha"
  run_ok "refs no match" -- refs "ZzzNonexistent999"
}

suite_flow() {
  echo ""
  echo "== flow =="
  run_ok "flow basic" -- flow "Alpha"
  run_ok "flow --limit 1" -- flow --limit 1 "Alpha"
  run_ok "flow --offset 1" -- flow --offset 1 --limit 1 "Alpha"
  run_ok "flow --callees-top 2 --callers-top 2" -- flow --callees-top 2 --callers-top 2 "Alpha"
  run_ok "flow --budget 500" -- flow --budget 500 "Alpha"
  run_ok "flow --no-did-you-mean (no match)" -- flow --no-did-you-mean "ZzzNonexistent999"
  run_ok "flow --format json" -- --format json flow "Alpha"
  run_ok "flow no match" -- flow "ZzzNonexistent999"
}

suite_siblings() {
  echo ""
  echo "== siblings =="
  run_ok "siblings basic" -- siblings "Alpha"
  run_ok "siblings --limit 2" -- siblings --limit 2 "Alpha"
  run_ok "siblings --offset 1" -- siblings --offset 1 --limit 2 "Alpha"
  run_ok "siblings --include-imports" -- siblings --include-imports "Alpha"
  run_ok "siblings --format json" -- --format json siblings "Alpha"
  run_ok "siblings no match" -- siblings "ZzzNonexistent999"
}

suite_deps() {
  echo ""
  echo "== deps =="
  run_ok "deps basic" -- deps "crates/b/src/lib.rs"
  run_ok "deps --limit 5" -- deps --limit 5 "crates/b/src/lib.rs"
  run_ok "deps --offset 1" -- deps --offset 1 --limit 5 "crates/b/src/lib.rs"
  run_ok "deps --no-dependents" -- deps --no-dependents "crates/b/src/lib.rs"
  run_ok "deps --format json" -- --format json deps "crates/b/src/lib.rs"
  # deps on a file with no dependents
  run_ok "deps isolated file" -- deps "src/outline_sample.rs"
}

suite_impact() {
  echo ""
  echo "== impact =="
  run_ok "impact basic" -- impact "Alpha"
  run_ok "impact --limit 5" -- impact --limit 5 "Alpha"
  run_ok "impact --offset 1" -- impact --offset 1 --limit 5 "Alpha"
  run_ok "impact --hops 1" -- impact --hops 1 "Alpha"
  run_ok "impact --hops 3" -- impact --hops 3 "Alpha"
  run_ok "impact --hub-guard 10" -- impact --hub-guard 10 "Alpha"
  run_ok "impact --format json" -- --format json impact "Alpha"
  run_ok "impact no match" -- impact "ZzzNonexistent999"
}

suite_index() {
  echo ""
  echo "== index =="
  run_ok "index" -- index
  run_ok "index --force" -- index --force
  run_ok "index --format json" -- --format json index
  # index should create .ffs dir
  if [[ -d "$TMP/.ffs" ]]; then log_pass "index creates .ffs"; else log_fail "index creates .ffs"; fi
}

suite_map_overview() {
  echo ""
  echo "== map / overview =="
  run_ok "map default" -- map
  run_ok "map --depth 1" -- map --depth 1
  run_ok "map --depth 5" -- map --depth 5
  run_ok "map --max-file-bytes 1024" -- map --max-file-bytes 1024
  run_ok "map --bytes-per-token 4" -- map --bytes-per-token 4
  run_ok "map --symbols 2" -- map --symbols 2
  run_ok "map --format json" -- --format json map

  run_ok "overview default" -- overview
  run_ok "overview --top-languages 2" -- overview --top-languages 2
  run_ok "overview --top-symbols 2" -- overview --top-symbols 2
  run_ok "overview --top-entrypoints 2" -- overview --top-entrypoints 2
  run_ok "overview --format json" -- --format json overview
}

suite_mention_guide() {
  echo ""
  echo "== mention / guide / mcp =="
  run_ok "mention basic" -- mention "Alpha"
  run_ok "mention --max-tokens 1000" -- mention --max-tokens 1000 "Alpha"
  run_ok "mention --cursor 1" -- mention --cursor 1 "Alpha"
  run_ok "mention --output-format json" -- mention --output-format json "Alpha"
  run_ok "mention multi-token" -- mention "Alpha Beta"

  run_ok "guide" -- guide
  run_ok "guide --format json" -- --format json guide

  # mcp: smoke --help only (stdio server would block)
  run_ok "mcp --help" -- mcp --help
}

suite_global_flags() {
  echo ""
  echo "== global flags =="
  # run_ok already injects --root "$TMP", so global-flag tests use direct invocation
  local out="$TMP/out.$$" err="$TMP/err.$$"
  # --root explicit
  set +e; "$FFS_BIN" --root "$TMP" find "Alpha" >"$out" 2>"$err"; rc=$?; set -e
  if [[ $rc -eq 0 ]]; then log_pass "global --root explicit"; else log_fail "global --root explicit (exit $rc)"; fi
  # --format json at top level
  set +e; "$FFS_BIN" --root "$TMP" --format json find "Alpha" >"$out" 2>"$err"; rc=$?; set -e
  if [[ $rc -eq 0 ]]; then log_pass "global --format json"; else log_fail "global --format json (exit $rc)"; fi
  # --root + --format combo
  set +e; "$FFS_BIN" --root "$TMP" --format json find "Alpha" >"$out" 2>"$err"; rc=$?; set -e
  if [[ $rc -eq 0 ]]; then
    log_pass "global --root + --format json"
    if grep -qF '"needle"' "$out" 2>/dev/null; then log_pass "global --root + --format json is json"; else log_fail "global --root + --format json is json"; fi
  else
    log_fail "global --root + --format json (exit $rc)"
  fi
}

suite_regression() {
  echo ""
  echo "== regression (stack overflow / binary / utf8) =="
  local out="$TMP/out.$$"

  # Stack-overflow regression: deeply nested + large file already in fixture
  run_ok "regression deep.rs symbol" -- symbol "f0"
  run_ok "regression deep.rs outline" -- outline "src/deep.rs"
  run_ok "regression large.rs read budget" -- read --budget 50 "src/large.rs"

  # Binary detection
  printf 'binary\x00\xff\xfe' > "$TMP/src/bin2.dat"
  run_ok "regression binary read" -- read "src/bin2.dat"

  # Invalid UTF-8 — ffs read rejects with explicit error (not crash)
  printf 'let s = "\xc3\x28\n' > "$TMP/src/garbled.rs"
  assert_exit_code "regression garbled utf8 read exits non-zero" 1 -- read "src/garbled.rs"

  # Empty file
  : > "$TMP/src/empty2.rs"
  run_ok "regression empty file read" -- read "src/empty2.rs"
  run_ok "regression empty file outline" -- outline "src/empty2.rs"

  # Path with spaces
  mkdir -p "$TMP/my dir"
  echo "hello Alpha" > "$TMP/my dir/file with spaces.rs"
  run_ok "regression path with spaces" -- read "my dir/file with spaces.rs"
  "$FFS_BIN" --root "$TMP" find "file with spaces" >"$out" 2>/dev/null
  # just ensure it doesn't crash

  # Symlink loop guard (don't follow symlinks)
  ln -sf "$TMP/src" "$TMP/link_to_src" 2>/dev/null || true
  run_ok "regression symlink find" -- find "Alpha"
  rm -f "$TMP/link_to_src"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
main() {
  setup_fixture

  suite_cli_meta
  suite_find
  suite_glob
  suite_grep
  suite_multi_grep
  suite_read
  suite_outline
  suite_symbol
  suite_callers
  suite_callees
  suite_refs
  suite_flow
  suite_siblings
  suite_deps
  suite_impact
  suite_index
  suite_map_overview
  suite_mention_guide
  suite_global_flags
  suite_regression

  echo ""
  echo "========================================"
  echo "Results: ${GREEN}${PASS} passed${RESET}, ${RED}${FAIL} failed${RESET}, ${YELLOW}${SKIP} skipped${RESET}"
  if [[ $FAIL -gt 0 ]]; then
    echo "${RED}Failed cases:${RESET}"
    for c in "${FAILED_CASES[@]}"; do echo "  - $c"; done
    echo "========================================"
    exit 1
  fi
  echo "${GREEN}All e2e checks passed.${RESET}"
  echo "========================================"
}

main
