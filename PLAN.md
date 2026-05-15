# PLAN.md — Cải thiện `ffs` (fast_file_search)

Tham chiếu nguồn:
- Báo cáo kiểm thử v0.7.3 (xem `REPORT.md` đính kèm chat) — Linux kernel 93k files, hyperfine warmup=2 runs=5.
- Repo: `quangdang46/fast_file_search` (fork của `dmtrKovalenko/fff.nvim`).
- Code-base hiện tại — đã đọc:
  - `crates/ffs-cli/src/commands/grep.rs` (toàn bộ logic CLI grep)
  - `crates/ffs-cli/src/commands/find.rs`
  - `crates/ffs-cli/src/commands/index.rs`
  - `crates/ffs-cli/src/commands/mod.rs::walk_files`
  - `crates/ffs-engine/src/dispatch.rs::Engine::index`
  - `install-mcp.sh` (fork) và `install-mcp.sh` (upstream fff.nvim)
- Reference design:
  - `dmtrKovalenko/fff.nvim` (upstream — đặc biệt `crates/fff-mcp/`, `pi-fff` extension, frecency DB)
  - `BurntSushi/ripgrep` (parallel walker + SIMD + early-exit)
  - `helix-editor/nucleo` (fuzzy matcher, lock-free streaming)
  - `openai/codex` CLI `codex mcp add` UX
  - Anthropic `claude mcp add -s user`
  - Cursor `~/.cursor/mcp.json` / `.cursor/mcp.json`
  - OpenCode `~/.config/opencode/opencode.json` (`mcp.*.command`)
  - Cline VSCode extension `cline_mcp_settings.json` (workspace state)
  - Continue.dev `~/.continue/config.json` hoặc `.continue/mcpServers/*.yaml`

---

## 0. Executive summary

Có **6 vấn đề lớn** cần fix. Thứ tự ưu tiên theo impact:

| # | Vấn đề | Impact ước tính | Effort |
|---|---|---|---|
| P1 | `ffs grep` chỉ làm substring, không phải regex; chạy single-thread sequential | Sửa xong: `ffs grep` ≈ `rg` (≤1.5×) thay vì 7× chậm hơn | M |
| P2 | `ffs index` không persist trên đĩa → mỗi CLI invoc rebuild ~80s trên repo lớn | Sửa xong: `ffs symbol/callers/flow` từ 80s → 50-200ms | L |
| P3 | `ffs find` không fuzzy/typo-resistant ở CLI (chỉ `.contains()`) | Match README quảng cáo, parity với MCP | S |
| P4 | Thiếu **auto-install MCP cho 6 provider** (Claude Code, Codex, Cursor, Cline, OpenCode, Continue) — hiện chỉ in instruction copy-paste | UX: 1 lệnh cài xong toàn bộ tooling | M |
| P5 | `ffs find/glob` chậm hơn GNU `find` 2-2.5× do `walk_files` không dùng parallel walker | Speed-up ~2× cho file-name search | S |
| P6 | `--help`/README sai lệch với hành vi thật (regex, fuzzy, on-disk index) | Trust + docs | XS |

**Effort guide:** XS = < 1 ngày, S = 1-3 ngày, M = 1 tuần, L = 2 tuần.

Tổng quan kỹ thuật: các crate `ffs-grep` (SIMD), `ffs-symbol` (bigram + Bloom), `regex 1.11`, `aho-corasick`, `memchr`, `heed` (LMDB), `rayon`, `memmap2`, `ignore`, `neo_frizbee` **đã có sẵn trong workspace**. Vấn đề chính: **CLI commands không gọi đến các crate này**. Hầu hết fix là wiring chứ không phải invent mới.

---

## 1. P1 — Sửa `ffs grep`: regex thật + parallel + early-exit

### 1.1 Root cause

Toàn bộ implementation của `ffs grep` (CLI) nằm trong `crates/ffs-cli/src/commands/grep.rs` chỉ 91 dòng:

```rust
// crates/ffs-cli/src/commands/grep.rs:37-71 (rút gọn)
let files = super::walk_files(root);           // sequential walker
let needle = if args.case_sensitive { args.needle.clone() }
             else { args.needle.to_lowercase() };
for path in &files {                            // single-thread loop
    let Ok(content) = std::fs::read_to_string(path) else { continue };
    for (lineno, line) in content.lines().enumerate() {
        let haystack = if args.case_sensitive { Cow::Borrowed(line) }
                       else { Cow::Owned(line.to_lowercase()) }; // alloc per line!
        if haystack.contains(&needle) {        // literal substring only
            hits.push(GrepHit { ... });
            if hits.len() >= args.limit { break; }
        }
    }
}
```

Vấn đề cụ thể:
1. `haystack.contains(&needle)` — không phải regex. Mọi metachar (`^`, `$`, `[]`, `\s`, `|`, `.*`) bị xử lý như literal.
2. Single-thread `for path in &files` — 2 vCPU / 8 core đều idle.
3. `to_lowercase()` allocate `String` cho từng dòng — GC pressure cao.
4. `std::fs::read_to_string` — không mmap, không streaming, copy toàn bộ file vào heap.
5. **Không gọi `ffs-grep` crate hoặc `ffs-engine`** (đã có Bigram pre-filter, SIMD, Aho-Corasick) — đây là điều **shocking nhất**. CLI `ffs grep` không xài 1 thuật toán nào của project.

### 1.2 Phương án

**Wiring lại CLI grep qua `ffs-grep` crate** (workspace đã có, MCP đang dùng):

```rust
// crates/ffs-cli/src/commands/grep.rs (sketch)
use ffs_grep::{GrepEngine, GrepMode, GrepRequest};
use ffs_engine::Engine;

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    // Detect regex vs literal automatically (giống rg --auto-mode hoặc nhánh
    // dispatch.rs đã có sẵn). Heuristic: nếu needle có metachar chưa escape →
    // compile regex; lỗi compile → fallback literal.
    let mode = detect_grep_mode(&args.needle, args.regex_explicit);
    let engine = Engine::with_index_root(root);          // mmap-load index
    let req = GrepRequest {
        needle: args.needle.clone(),
        mode,
        case_sensitive: args.case_sensitive,
        limit: args.limit,
        max_count_per_file: args.max_count.unwrap_or(usize::MAX),
        paths: vec![root.into()],
    };
    let hits = engine.grep(&req)?;                       // parallel internally
    // emit ...
}

fn detect_grep_mode(needle: &str, regex_explicit: bool) -> GrepMode {
    if regex_explicit { return GrepMode::Regex; }
    // Strict heuristic — copy from ffs-engine/dispatch.rs::classify
    let has_meta = needle.bytes().any(|b| matches!(b,
        b'^' | b'$' | b'[' | b']' | b'(' | b')' | b'{' | b'}'
        | b'*' | b'+' | b'?' | b'|' | b'\\' | b'.'
    ));
    if has_meta && regex::Regex::new(needle).is_ok() {
        GrepMode::Regex
    } else {
        GrepMode::Literal
    }
}
```

Khuyên thêm flags để parity với rg / fix tính minh bạch:
- `-r`/`--regex` (explicit regex)
- `-F`/`--fixed-strings` (explicit literal — để tắt auto-detect)
- `-w`/`--word-regexp` (word boundary)
- `--multi <patterns>` (multi-pattern OR — gọi `ffs_multi_grep` đã có ở MCP)
- `-A/-B/-C <n>` context lines
- `-l` files-with-matches mode
- `--max-count <n>` per-file early-exit (đã có trong rg)

### 1.3 Tham khảo hiệu suất

Ripgrep nhanh nhờ (nguồn: BurntSushi blog, ripgrep FAQ):
1. **Parallel walker** — `ignore::WalkBuilder::threads(N).build_parallel()` (hiện `walk_files` của ffs dùng `.build()` single-thread).
2. **SIMD memmem** cho literal — `memchr` crate đã trong deps.
3. **Aho-Corasick** cho multi-pattern OR — đã trong deps.
4. **`regex` crate** với DFA + lazy DFA + Pike VM — auto-fallback.
5. **mmap cho file ≥ ~64 KB**, đọc thường cho file nhỏ — `memmap2` đã trong deps.
6. **Skip binary files** — đã có `bindet` trong deps.
7. **Reuse buffer per thread** — không alloc lowercase mỗi line.
8. **Early termination** sau khi đủ `--limit` — yêu cầu atomic counter + thread cancellation.

### 1.4 Expected outcome

Benchmark trên Linux kernel 93k files (mục tiêu, đo lại sau fix):

| Pattern | Hiện tại | Mục tiêu | Ghi chú |
|---|---|---|---|
| `EXPORT_SYMBOL_GPL` (common) | 3.21 s | ≤ 700 ms | Trong khoảng 1.5× của `rg` (449 ms) |
| `kobject_create_and_add` (rare) | 2.98 s | ≤ 600 ms | Bigram pre-filter cắt ≥ 90% file candidate |
| `kobject_create_and_add --limit 10` | 722 ms | ≤ 40 ms | Atomic limit + early-exit per-thread |

---

## 2. P2 — Persistent on-disk index

### 2.1 Root cause

`Engine::index` trong `crates/ffs-engine/src/dispatch.rs:88` chỉ build trong RAM rồi vứt khi process exit:

```rust
pub fn index(&self, root: &Path) -> ScanReport {
    let report = self.scanner.scan(root);  // build in-memory
    self.guard(...);                        // apply budget
    report                                  // (no fs write)
}
```

Không tìm thấy bất kỳ `serialize`/`save_to_disk`/`persist`/`on_disk` nào trong `ffs-engine` lẫn `ffs-symbol`. `--help` của `ffs index` ghi "Build / refresh the **on-disk** indexes (Bigram, Bloom, Symbol, Outline)" — sai lệch hành vi.

Hệ quả thực đo:
- `ffs index` Linux kernel cold = 79 s, re-run = 81 s (không hề tận dụng kết quả lần trước).
- Mỗi `ffs symbol` CLI = ~79 s (= chi phí re-index).

### 2.2 Phương án — LMDB-backed index store

Dùng `heed` (đã có trong workspace dependency, đang được dùng cho frecency DB) làm storage layer. Lý do chọn LMDB:
- Memory-mapped, zero-copy đọc → khởi động ms-scale.
- Single-writer multi-reader → an toàn khi `ffs index` đang refresh và `ffs grep` đang đọc.
- Battle-tested, đã trong project, không add dep mới.

**Layout đề xuất** (lưu tại `<root>/.ffs/` hoặc `$XDG_CACHE_HOME/ffs/<repo-fingerprint>/`):

```
.ffs/
├── meta.db                # version, fingerprint, last_indexed_ns, file_count
├── files.db               # path_id (u32) -> { path, mtime_ns, size, blake3, lang }
├── path_trie.bin          # sorted path list + bigram index cho ffs find
├── symbols.db             # symbol_name -> [(path_id, line, kind, parent_id)]
├── symbols_kind.db        # kind -> [(symbol_name, path_id)]
├── bigram.bin             # 2-gram → roaring bitmap of path_ids (cho grep pre-filter)
├── bloom.bin              # per-file 32 KB Bloom filter (cho callers/refs narrowing)
├── outline.db             # path_id -> serialized outline (postcard)
└── frecency.db            # đã tồn tại
```

Format: dùng `postcard` (no_std, compact) hoặc `bincode` 2.0 cho serde. Roaring bitmap dùng `roaring` crate cho bigram inverted index — đây là format mà rg-internals & lucene cùng dùng.

**Invalidation** — incremental refresh:
1. Mở `meta.db`. So sánh `git rev-parse HEAD` (nếu là git repo) và mtime của thư mục root.
2. Walk file system → so file path + mtime với `files.db`.
   - File mới / mtime thay đổi → re-index file đó.
   - File đã xóa → tombstone trong `files.db`, dọn dẹp ở `bigram.bin` qua compaction định kỳ.
3. Lazy: nếu một query hit path đã tombstone, fallback fresh-walk path đó.

**Mode chạy ngầm** — daemon (optional, phase 2):
- `ffs serve --root . --watch` chạy background, dùng `notify` crate (đã có chain qua `ffs-core/src/background_watcher.rs`) để auto-refresh.
- CLI subcommand check `.ffs/socket` → nếu có daemon, dispatch qua Unix socket; nếu không, fallback CLI in-process.
- Tham khảo: `nucleo` worker thread + snapshot pattern (helix-editor).

### 2.3 Compat & rollout

- `ffs index --force` để rebuild from scratch.
- `ffs index --no-cache` để verify behavior cũ.
- Add `.ffs/` vào `.gitignore` mặc định khi init (tương tự `.ripgreprc` không-commit).
- Schema version trong `meta.db` — bump version khi format thay đổi, tự rebuild khi mismatch.

### 2.4 Expected outcome

| Lệnh | Hiện tại (Linux kernel) | Mục tiêu warm | Ghi chú |
|---|---|---|---|
| `ffs index` cold | 79 s | 79 s (giữ nguyên) | Lần đầu vẫn phải walk + parse |
| `ffs index` warm (no changes) | 81 s | < 500 ms | Mtime scan + load meta |
| `ffs symbol` | 79 s | < 50 ms | Hash lookup trong `symbols.db` |
| `ffs callers` | ~80 s | < 200 ms | Bloom prefilter + literal confirm |
| `ffs flow` | ~80 s | < 200 ms | Same as callers + outline read |
| `ffs grep` rare pattern | 3 s | < 200 ms | Bigram inverted index narrow → SIMD verify |

---

## 3. P3 — `ffs find` fuzzy / typo-resistant CLI

### 3.1 Root cause

`crates/ffs-cli/src/commands/find.rs:61-75`:

```rust
fn search_matches(scopes: &[PathBuf], needle: &str) -> Vec<String> {
    let needle_lower = needle.to_lowercase();
    for scope in scopes {
        for p in super::walk_files(scope) {
            if s.to_lowercase().contains(&needle_lower) { ... }
        }
    }
}
```

Substring match thuần. Không xài `neo_frizbee` (workspace dep, dùng trong Neovim plugin) lẫn smart-case auto-fuzzy fallback mà README quảng cáo.

### 3.2 Phương án

Pipeline ba lớp, fallback theo thứ tự:

1. **Smart-case literal substring** (như hiện tại nhưng tôn trọng case khi needle có uppercase).
2. **CamelCase / snake_case expansion** — `IsOffTheRecord` cũng match `is_off_the_record.rs`. Tham khảo: pi-fff implementation (đã có in fff.nvim).
3. **Fuzzy fallback** — nếu zero match: chạy `nucleo-matcher` (hoặc `neo_frizbee` đã có) với threshold-based filter. Tag prefix `(fuzzy)` trong text output để minh bạch.

Add flags:
- `--fuzzy` / `--no-fuzzy` (override auto-detect)
- `--smart-case` (default on) / `--ignore-case` / `--case-sensitive`
- Frecency boost flag `--frecency` để rank theo `lua/ffs/rust/frecency.rs` (đã tồn tại).

### 3.3 Expected outcome

- `ffs find isntall.sh` → match `install.sh` (fuzzy fallback, distance ≤ 2).
- `ffs find IsOffTheRecord` → match `is_off_the_record.{rs,c,py}` (camel/snake expansion).
- Pattern thuần ASCII không metachar không bị slowdown đáng kể (literal pass đầu vẫn O(n)).

---

## 4. P4 — Auto-install MCP cho 6 provider

### 4.1 Hiện trạng

`install-mcp.sh` hiện tại:
1. Download binary `ffs-mcp` ✓
2. Detect tools (Claude Code, OpenCode, Codex) ✓
3. **Chỉ in command để user copy-paste** ✗ — không tự chạy.
4. Không hỗ trợ Cursor, Cline, Continue.
5. Repo URL trong script vẫn trỏ về `dmtrKovalenko/ffs.nvim` (sót lại từ fork) — phải update sang `quangdang46/fast_file_search`.

### 4.2 Phương án — auto-install matrix

Sửa `install-mcp.sh` thành **idempotent installer** thật sự. Mỗi provider có method auto cụ thể:

| Provider | Detect | Auto-install method | Config path | Source |
|---|---|---|---|---|
| **Claude Code** | `command -v claude` | `claude mcp add -s user ffs -- "$BIN"` | `~/.claude/settings.json` | docs.anthropic.com / `claude mcp add --help` |
| **Codex (OpenAI)** | `command -v codex` | `codex mcp add ffs -- "$BIN"` | `~/.codex/config.toml` | developers.openai.com/codex/mcp |
| **Cursor** | `[ -d ~/.cursor ]` hoặc `command -v cursor` | Merge JSON vào `~/.cursor/mcp.json` qua `jq` | `~/.cursor/mcp.json` (global) hoặc `.cursor/mcp.json` (project) | cursor.com/docs/mcp |
| **Cline** | Tìm `~/.config/Code*/User/globalStorage/saoudrizwan.claude-dev*/settings/cline_mcp_settings.json` | Merge JSON qua `jq` | cline_mcp_settings.json (workspace state) | docs.cline.bot/mcp |
| **OpenCode** | `command -v opencode` hoặc `[ -d ~/.config/opencode ]` | Merge JSON vào `~/.config/opencode/opencode.json` qua `jq` | `~/.config/opencode/opencode.json` | opencode.ai/docs/mcp-servers |
| **Continue.dev** | `[ -d ~/.continue ]` | Tạo `~/.continue/mcpServers/ffs.yaml` | `~/.continue/mcpServers/*.yaml` | docs.continue.dev/customize/deep-dives/mcp |
| **Generic stdio MCP** | Fallback | In ra block JSON + path để user paste | — | — |

### 4.3 Implementation sketch

```bash
install_for_claude_code() {
    if ! command -v claude >/dev/null 2>&1; then return 1; fi
    # claude mcp add idempotent — nếu trùng tên sẽ overwrite (xác minh `--help`)
    if claude mcp list 2>/dev/null | grep -q '^ffs\s'; then
        info "[Claude Code] ffs already registered, skipping"
        return 0
    fi
    claude mcp add -s user ffs -- "$BIN" >/dev/null
    success "[Claude Code] registered ffs → ~/.claude/settings.json"
}

install_for_cursor() {
    local cfg="$HOME/.cursor/mcp.json"
    [ -d "$HOME/.cursor" ] || return 1
    mkdir -p "$(dirname "$cfg")"
    [ -f "$cfg" ] || echo '{"mcpServers":{}}' > "$cfg"
    # Merge using jq to preserve existing keys
    local tmp=$(mktemp)
    jq --arg cmd "$BIN" \
       '.mcpServers.ffs = { "type":"stdio", "command":$cmd, "args":[] }' \
       "$cfg" > "$tmp" && mv "$tmp" "$cfg"
    success "[Cursor] registered ffs → $cfg"
}

install_for_codex() {
    if ! command -v codex >/dev/null 2>&1; then return 1; fi
    if codex mcp list 2>/dev/null | grep -q '^ffs\s'; then
        info "[Codex] ffs already registered, skipping"
        return 0
    fi
    codex mcp add ffs -- "$BIN" >/dev/null
    success "[Codex] registered ffs → ~/.codex/config.toml"
}

install_for_continue() {
    [ -d "$HOME/.continue" ] || return 1
    local dst="$HOME/.continue/mcpServers/ffs.yaml"
    mkdir -p "$(dirname "$dst")"
    cat > "$dst" <<EOF
name: ffs
version: 0.1.0
schema: v1
mcpServers:
  - name: ffs
    command: $BIN
EOF
    success "[Continue] registered ffs → $dst"
}

install_for_opencode() {
    local cfg="$HOME/.config/opencode/opencode.json"
    [ -f "$cfg" ] || command -v opencode >/dev/null 2>&1 || return 1
    mkdir -p "$(dirname "$cfg")"
    [ -f "$cfg" ] || echo '{"$schema":"https://opencode.ai/config.json","mcp":{}}' > "$cfg"
    local tmp=$(mktemp)
    jq --arg cmd "$BIN" \
       '.mcp.ffs = { "type":"local", "command":[$cmd], "enabled":true }' \
       "$cfg" > "$tmp" && mv "$tmp" "$cfg"
    success "[OpenCode] registered ffs → $cfg"
}

install_for_cline() {
    # VSCode + Code-OSS + Cursor (Cline runs inside many editors)
    for base in \
        "$HOME/.config/Code/User/globalStorage" \
        "$HOME/.config/Code - Insiders/User/globalStorage" \
        "$HOME/.vscode-server/data/User/globalStorage" \
        "$HOME/Library/Application Support/Code/User/globalStorage"; do
        [ -d "$base" ] || continue
        local cfg
        cfg=$(find "$base" -maxdepth 3 -name 'cline_mcp_settings.json' 2>/dev/null | head -n1)
        [ -n "$cfg" ] || continue
        [ -f "$cfg" ] || echo '{"mcpServers":{}}' > "$cfg"
        local tmp=$(mktemp)
        jq --arg cmd "$BIN" \
           '.mcpServers.ffs = { "command":$cmd, "args":[], "disabled":false }' \
           "$cfg" > "$tmp" && mv "$tmp" "$cfg"
        success "[Cline] registered ffs → $cfg"
        return 0
    done
    return 1
}
```

Flag UX cho installer:

```
install.sh
  --mcp                   sau khi cài ffs, tự cài MCP cho tất cả provider detect được
  --mcp-only              chỉ cài MCP, skip download binary (khi binary đã có)
  --mcp-provider <list>   comma-separated: claude,codex,cursor,cline,opencode,continue,all
  --mcp-name <name>       override tên MCP (default: ffs)
  --mcp-dry-run           in command + diff config sẽ áp dụng nhưng không ghi đè
```

### 4.4 Phụ thuộc

- `jq` — required cho 4/6 provider. Nếu thiếu, dùng Python fallback `python3 -c "import json,sys;..."`.
- Bỏ qua provider nào không detect được, không fail toàn cục.
- Idempotent — chạy lại nhiều lần phải an toàn (check `.mcpServers.ffs` đã tồn tại trước khi merge).

---

## 5. P5 — `ffs find/glob` cạnh tranh với GNU find

### 5.1 Root cause

`walk_files()` trong `crates/ffs-cli/src/commands/mod.rs:59`:

```rust
let walker = ignore::WalkBuilder::new(root)
    .standard_filters(true)
    .follow_links(false)
    .build();         // ← single-threaded
for entry in walker.flatten() { ... }
```

`ignore::WalkBuilder` hỗ trợ `.build_parallel()` (như rg) nhưng đang dùng `.build()` đồng bộ. Trên Linux kernel 93k files, walk này tốn ~80-100 ms, chiếm 70% wall-clock của `ffs find/glob`.

### 5.2 Phương án

```rust
use ignore::{WalkBuilder, WalkState};
use std::sync::Mutex;

pub(crate) fn walk_files(root: &Path) -> Vec<PathBuf> {
    let out = Mutex::new(Vec::with_capacity(8192));
    let walker = WalkBuilder::new(root)
        .standard_filters(true)
        .follow_links(false)
        .threads(num_cpus::get().min(8))     // ← cap để tránh thrash IO
        .build_parallel();
    walker.run(|| {
        let out = &out;
        Box::new(move |entry| {
            if let Ok(e) = entry {
                if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    out.lock().unwrap().push(e.into_path());
                }
            }
            WalkState::Continue
        })
    });
    out.into_inner().unwrap()
}
```

Khi có on-disk index (P2): bỏ qua walk hoàn toàn, đọc `files.db` mmap → chậm hơn nữa cũng < 50 ms.

### 5.3 Expected outcome

- `ffs find scheduler.c` trên Linux kernel: 154 ms → < 80 ms (cạnh tranh với GNU find 63 ms).
- `ffs glob '**/*.h'`: 102 ms → < 60 ms.

---

## 6. P6 — Docs & flags consistency

### 6.1 Fix doc strings

- `crates/ffs-cli/src/commands/grep.rs:11` — `"Substring or regex"` → đúng sau khi fix P1, không cần đổi text.
- `crates/ffs-cli/src/commands/index.rs` — help text README/CLI "on-disk indexes" cần khớp với P2 implementation.
- README "Typo-resistant fuzzy matching" — cần chú thích: "via MCP/library; CLI requires `--fuzzy` or zero-match fallback (post-P3)."

### 6.2 JSON output schema versioning

Hiện tại `--format json` chưa có `schema_version`. Thêm field này để agent có thể detect version mismatch:

```json
{ "schema": "v1", "needle": "...", "hits": [...], ... }
```

---

## 7. Roadmap & milestone

### Milestone M1 — Quick wins (1 tuần)
- P3 fuzzy `ffs find` (1-2 ngày): wire `neo_frizbee` + fallback path. Add `--fuzzy` flag.
- P5 parallel walker (1 ngày): swap `build()` → `build_parallel()`. Cap threads = `min(num_cpus, 8)`.
- P6 doc fixes (< 1 ngày).
- P1 phần literal: chuyển `ffs grep` sang ffs-grep crate + rayon parallel (chưa cần on-disk index). Wire `regex` crate khi detect metachar.

**Output M1:** ffs grep ~2× của rg (vs 7× hiện tại), find/glob ngang find, fuzzy hoạt động.

### Milestone M2 — On-disk index (2 tuần)
- P2 phase 1: LMDB schema + serialization (5 ngày).
- P2 phase 2: incremental refresh + invalidation (3 ngày).
- P2 phase 3: wire `symbol`/`callers`/`flow` vào load path (3 ngày).
- P1 phase 2: ffs grep dùng bigram inverted index từ on-disk store (2 ngày).
- Benchmark + tune.

**Output M2:** `ffs symbol` < 50 ms warm, `ffs grep` rare < 200 ms.

### Milestone M3 — Auto-install MCP (3-5 ngày)
- P4 implement 6 provider auto-install (2 ngày).
- Test trên Linux/macOS/WSL (1 ngày — cần ít nhất Claude Code + Codex + Cursor để verify).
- Add `--mcp-dry-run`, `--mcp-provider`, idempotency tests (1 ngày).
- README section "One-liner setup".

**Output M3:** `curl ... | bash -s -- --mcp` cài binary + tự đăng ký với mọi AI tool có trên máy.

### Milestone M4 — Optional daemon mode (2 tuần, deferred)
- `ffs serve` Unix socket / Windows named pipe.
- File watcher tích hợp với on-disk index.
- CLI subcommand auto-dispatch qua daemon nếu phát hiện.

**Output M4:** Parity với MCP performance trong shell-script use case.

---

## 8. Risks & mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Auto-merge JSON config của user phá settings cũ | M | H | Backup `*.bak` trước khi ghi; `--mcp-dry-run` mặc định; idempotent merge bằng `jq` (không string replace) |
| `claude/codex mcp add` CLI flag thay đổi | M | M | Detect version qua `claude --version`; nếu CLI fail, fallback ghi trực tiếp settings.json |
| LMDB lock conflict khi nhiều `ffs` chạy song song | L | M | LMDB native single-writer multi-reader; serialize index writes qua lock; read luôn không block |
| On-disk index quá lớn (Linux kernel: ước ~200 MB cho 93k files) | M | L | Compression với `lz4_flex` cho outline/body content; tombstone GC định kỳ |
| Threading regression — IO-bound box (2 vCPU) chậm hơn single-thread | L | L | `threads(min(num_cpus, 8))` + benchmark trên ≥3 môi trường (laptop, server, container) |
| Parser regex bị abuse (catastrophic backtracking) | L | M | `regex` crate đảm bảo linear time; nếu thêm fancy-regex sau, set timeout per match |
| Disk full khi index repo lớn | M | L | Phát hiện `ENOSPC` → in cảnh báo, fallback in-memory mode |
| Compat với fff.nvim upstream khi rebase | M | M | Maintain a CHANGELOG `FORK_DIFF.md` ghi rõ điểm khác; sync upstream định kỳ qua merge commit |

---

## 9. Open questions cần Trần xác nhận

1. **Tên fork**: giữ `ffs` (binary name) hay đổi sang tên khác để tránh đụng `fff` upstream? PLAN này giữ `ffs`.
2. **Backward compat**: có cần giữ behavior cũ của `ffs grep` (literal-only) đằng sau flag `--legacy-grep` không, hay break ngay từ M1?
3. **Daemon mode** (M4): có ưu tiên không? Nếu users chủ yếu xài qua MCP thì M4 có thể bỏ — vì MCP đã là long-lived process.
4. **Telemetry opt-in**: thu thập index time / query latency để ưu tiên hot path tiếp theo?
5. **Min supported Rust**: hiện `rust-toolchain.toml` lock 1 phiên bản — cần verify trước khi add `heed`/`postcard`/`roaring` mới.
6. **Schema version**: lần đầu — bắt đầu từ `v1` rồi bump khi nào break.

---

## 10. Tham chiếu chi tiết

- ripgrep design notes: <https://github.com/BurntSushi/ripgrep/blob/master/FAQ.md>, blog "ripgrep is faster than..." <https://blog.burntsushi.net/ripgrep/>
- nucleo matcher (background worker + lock-free streaming): <https://docs.rs/nucleo>, helix PR #7814.
- fff.nvim upstream `crates/fff-mcp/` (cho reference logic dispatch).
- Claude Code MCP API: <https://docs.anthropic.com/claude-code/mcp>, `claude mcp add --help`.
- Codex MCP: <https://developers.openai.com/codex/mcp>, source `openai/codex/codex-rs/cli/src/mcp_cmd.rs`.
- Cursor MCP: <https://cursor.com/docs/mcp>, file `~/.cursor/mcp.json`.
- Cline MCP: <https://docs.cline.bot/mcp/adding-and-configuring-servers>.
- OpenCode MCP: <https://opencode.ai/docs/mcp-servers>.
- Continue MCP: <https://docs.continue.dev/customize/deep-dives/mcp>.
- LMDB / heed crate (đã dùng cho frecency DB).

---

*PLAN này đề xuất; chưa implement. Yêu cầu Trần review M1/M2/M3/M4 priority và xác nhận open questions §9 trước khi bắt đầu code.*
