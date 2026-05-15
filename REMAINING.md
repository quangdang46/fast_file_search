# REMAINING.md — kế hoạch chi tiết phần còn lại sau PR #28

Phạm vi: tất cả công việc **chưa nằm trong PR #28**. Mục tiêu là đưa `ffs` đạt
trạng thái production cho **cold CLI** trên repo lớn, đặc biệt nhóm
`ffs symbol`/`callers`/`refs`/`flow`/`siblings`/`impact` đang phải **rebuild
tree-sitter index 79 giây/lần invoke** trên Linux kernel.

Tham chiếu: [PLAN.md](./PLAN.md) §P2 + §M2, [REPORT.md (chat attachment)](.).

---

## 1. M2 — On-disk index (highest priority)

### 1.1 Vấn đề cụ thể

```
ffs symbol scheduler  ─►  Engine::default()             [Arc<SymbolIndex>::new]
                          engine.index(root)            [tree-sitter parse 93k files = 79s]
                          engine.handles.symbols.lookup_exact("scheduler")
                          render → 30ms
                          ────────────────────────────────────────
                          Total: 79s mỗi invocation, kể cả nếu repo không đổi.
```

Mọi command code-navigation (8 cái) đều theo pattern này. Không có disk cache.

### 1.2 Mục tiêu

| Command | Hiện tại | Sau M2 (warm) | Sau M2 (cold) |
|---------|---------:|---------------:|---------------:|
| `ffs symbol` | 79 s | **≤ 50 ms** | 79 s |
| `ffs callers` | 80 s | **≤ 100 ms** | 80 s |
| `ffs refs` | 81 s | **≤ 150 ms** | 81 s |
| `ffs flow` | 82 s | **≤ 200 ms** | 82 s |
| `ffs grep <rare>` | 3.2 s* | **≤ 200 ms** | 3.2 s |

(*) Đã giảm xuống ~320 ms trong PR #28 cho query phổ biến nhờ parallel scan,
nhưng query *rare* trên kernel vẫn cần bigram prefilter để < 200 ms.

### 1.3 Layout `.ffs/` trên đĩa

```
<repo-root>/
└── .ffs/
    ├── meta.json                 # schema_version, git_head, ffs_version, file_count, generated_at_ms
    ├── symbol_index.postcard.zst # SymbolIndex serialize → postcard → zstd-19 (compact)
    ├── files.postcard.zst        # PathBuf -> mtime (SystemTime) cho incremental check
    ├── bigram.bin                # raw bigram bit-vector cho grep prefilter (M2 phase 2)
    └── outline.postcard.zst      # OutlineIndex (cho `ffs outline`, optional)
```

Lý do **không dùng LMDB ngay từ đầu**: postcard+zstd đơn giản, atomic, ~3-5×
nhỏ hơn JSON, deserialize nhanh hơn LMDB cho payload < 200 MB. Nếu sau này
need partial loading (e.g. workspace > 1 GB of symbols) sẽ chuyển sang
`heed` (đã có trong workspace).

### 1.4 Crate phụ thuộc

Thêm vào `crates/ffs-cli/Cargo.toml`:

```toml
postcard = { version = "1", features = ["use-std"] }
zstd     = "0.13"
sha2     = "0.10"
```

Hoặc nếu workspace ưu tiên `bincode`, dùng `bincode 1.3` (đã từng có?). Cần
verify trong `Cargo.toml` root.

### 1.5 Plan implementation chi tiết

#### Step 1 — Module `cache` trong `ffs-cli` (~150 LoC, 1 ngày)

File mới: `crates/ffs-cli/src/cache.rs`

```rust
use anyhow::{Context, Result};
use ffs_symbol::symbol_index::SymbolIndex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SCHEMA_VERSION: &str = "v1";
const FFS_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Serialize, Deserialize)]
struct CacheMeta {
    schema_version: String,
    ffs_version: String,
    git_head: Option<String>,   // None nếu repo không phải git
    file_count: usize,
    generated_at_ms: u128,
}

pub struct CacheDir(PathBuf);

impl CacheDir {
    pub fn at(root: &Path) -> Self {
        Self(root.join(".ffs"))
    }

    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.0).context("create .ffs dir")?;
        Ok(())
    }

    pub fn meta_path(&self) -> PathBuf { self.0.join("meta.json") }
    pub fn symbol_path(&self) -> PathBuf { self.0.join("symbol_index.postcard.zst") }
    pub fn files_path(&self) -> PathBuf { self.0.join("files.postcard.zst") }

    /// Trả về Some(idx) nếu cache hợp lệ (git head + file_count khớp); None nếu invalidate.
    pub fn load_symbol_index(&self, root: &Path) -> Option<SymbolIndex> {
        let meta = self.read_meta().ok()?;
        if meta.schema_version != SCHEMA_VERSION { return None; }
        if !self.head_matches(root, meta.git_head.as_deref()) { return None; }
        if !self.fast_file_count_matches(root, meta.file_count) { return None; }
        let bytes = fs::read(self.symbol_path()).ok()?;
        let decompressed = zstd::stream::decode_all(&bytes[..]).ok()?;
        postcard::from_bytes::<SymbolIndex>(&decompressed).ok()
    }

    pub fn write_symbol_index(&self, idx: &SymbolIndex, root: &Path) -> Result<()> {
        self.ensure()?;
        let payload = postcard::to_allocvec(idx).context("postcard serialize")?;
        let mut compressed = Vec::new();
        zstd::stream::copy_encode(&payload[..], &mut compressed, 19)
            .context("zstd compress")?;
        atomic_write(&self.symbol_path(), &compressed)?;
        let meta = CacheMeta {
            schema_version: SCHEMA_VERSION.to_string(),
            ffs_version: FFS_VERSION.to_string(),
            git_head: read_git_head(root),
            file_count: idx.files_indexed(),
            generated_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_millis(),
        };
        atomic_write(&self.meta_path(), serde_json::to_vec_pretty(&meta)?.as_slice())?;
        Ok(())
    }

    fn read_meta(&self) -> Result<CacheMeta> {
        Ok(serde_json::from_slice(&fs::read(self.meta_path())?)?)
    }

    fn head_matches(&self, root: &Path, expected: Option<&str>) -> bool {
        let now = read_git_head(root);
        now.as_deref() == expected
    }

    fn fast_file_count_matches(&self, root: &Path, expected: usize) -> bool {
        // Đếm theo ignore::WalkBuilder — fast path. Nếu chênh > 5% coi như stale.
        let now = crate::commands::walk_files(root).len();
        (now as i64 - expected as i64).abs() <= (expected as i64 / 20).max(10)
    }
}

fn read_git_head(root: &Path) -> Option<String> {
    let head = fs::read_to_string(root.join(".git/HEAD")).ok()?;
    if let Some(rest) = head.trim().strip_prefix("ref: ") {
        let ref_path = root.join(".git").join(rest);
        fs::read_to_string(ref_path).ok().map(|s| s.trim().to_string())
    } else {
        Some(head.trim().to_string())
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::rename(tmp, path)?;
    Ok(())
}
```

Test: `cargo test -p ffs-cli cache::tests`.

#### Step 2 — Hook vào `ffs index` (~30 LoC, 0.5 ngày)

Trong `commands/index.rs`, sau khi `Engine::default().index(root)`:

```rust
let cache = crate::cache::CacheDir::at(root);
cache.write_symbol_index(&engine.handles.symbols, root)?;
println!("Wrote cache: {}", cache.symbol_path().display());
```

Verify trên Linux kernel: chạy `ffs --root /home/ubuntu/test_linux index`,
xem `.ffs/symbol_index.postcard.zst` được tạo, size **expected ~10-30 MB**
(symbol index ~500k symbols × ~50 bytes/symbol nén zstd).

#### Step 3 — Wire vào `ffs symbol` (~40 LoC, 0.5 ngày)

Refactor `commands/symbol.rs::run`:

```rust
pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let cache = crate::cache::CacheDir::at(root);
    let symbols: Arc<SymbolIndex> = if let Some(idx) = cache.load_symbol_index(root) {
        Arc::new(idx)
    } else {
        // Cold path: build via engine then snapshot
        let engine = Engine::default();
        engine.index(root);
        let _ = cache.write_symbol_index(&engine.handles.symbols, root);
        engine.handles.symbols.clone()
    };
    // ... existing logic using `symbols` directly instead of `engine.handles.symbols`
}
```

**Vấn đề mở:** `SymbolIndex` hiện tại không `Serialize` được trực tiếp vì
chứa `AtomicUsize`. Cần check `crates/ffs-symbol/src/symbol_index.rs` — đã
derive `Serialize` nhưng có thể fail trên các field atomic. Hai cách fix:

a) Thêm wrapper `SerializableSymbolIndex { map: HashMap<String, Vec<SymbolLocation>>, files: HashMap<PathBuf, SystemTime> }` và convert qua lại.

b) `#[serde(skip)]` trên các atomic counter (counters có thể tính lại từ `map.len()`).

Cách (b) đơn giản hơn — sửa 2 dòng `#[serde(skip)]` trong `crates/ffs-symbol/src/symbol_index.rs`.

#### Step 4 — Wire vào `callers`/`refs`/`flow`/`siblings`/`impact` (~120 LoC, 1 ngày)

Mỗi command có pattern giống nhau:

```rust
let engine = Engine::default();
engine.index(root);
// dùng engine.handles.symbols + engine.scanner để extract bodies
```

Vấn đề: `callers`/`refs`/`flow` cần đọc **content file** quanh symbol để hiển
thị caller/callee context. Cache chỉ giữ symbol_index, không cache body. Body
đọc lại từ FS theo nhu cầu → vẫn nhanh vì chỉ vài file.

Approach:

1. Tách `callers::run` thành `run_with_engine(engine)` và `run(args)` mới chỉ
   build engine khi cache miss.
2. Thêm helper `crate::cache::load_or_build_engine(root) -> Engine` trong
   cache module — returns Engine với handles.symbols được swap bằng cached.

Tricky part: `Engine` không có public API swap `handles.symbols`. Cần:

- PR phụ vào `ffs-engine`: thêm `Engine::with_symbols(cfg, Arc<SymbolIndex>)
  -> Self` constructor.
- Hoặc dùng `unsafe` cast — không khuyến khích.

Quyết định: tạo PR phụ cho `ffs-engine` (vài chục dòng) trước khi merge M2.

#### Step 5 — Incremental refresh (~80 LoC, 1 ngày)

Khi `head_matches() == false` nhưng cache vẫn tồn tại, thay vì rebuild full,
walker chỉ parse các file thay đổi:

```rust
pub fn refresh_symbol_index(&self, root: &Path, base: SymbolIndex) -> SymbolIndex {
    let old_mtimes = base.files.clone();
    let mut to_repark = Vec::new();
    let mut to_delete = Vec::new();
    for p in walk_files(root) {
        let cur_mtime = fs::metadata(&p).and_then(|m| m.modified()).ok();
        match old_mtimes.get(&p) {
            None => to_repark.push(p),
            Some(old) if Some(old.value()) != cur_mtime.as_ref() => to_repark.push(p),
            _ => {}
        }
    }
    for entry in old_mtimes.iter() {
        if !entry.key().exists() { to_delete.push(entry.key().clone()); }
    }
    // 1) Drop deleted files' symbols
    base.drop_files(&to_delete);
    // 2) Parse + insert tủi đã đổi
    parallel_parse(&to_repark, &base);
    base
}
```

Cần thêm method `SymbolIndex::drop_files(&self, paths: &[PathBuf])` vào
`crates/ffs-symbol/src/symbol_index.rs` (~20 LoC).

#### Step 6 — Bigram prefilter cho `ffs grep` rare patterns (~150 LoC, 1.5 ngày)

Hiện tại `ffs grep` parallel scan ~93k file ~106 ms cho query có metachar.
Cho query *rare* (như `kallsyms_strict_str`) full scan vẫn 100+ ms vì còn
phải đọc nội dung file. Bigram prefilter có thể eliminate ~99% file trước:

1. `ffs-search` đã có `Bigram` trong `crates/ffs-core/src/grep.rs` (xem
   PLAN.md đoạn analyze). Cần expose API mới: `Bigram::serialize(&self)`,
   `Bigram::from_bytes(&[u8])`.
2. `ffs index` snapshot bigram → `.ffs/bigram.bin`.
3. `ffs grep` cold path: load bigram, prefilter danh sách file, sau đó
   parallel scan **chỉ trên file qua filter**.

Performance expected (kernel rare query): 3.2 s → < 200 ms.

#### Step 7 — Tests + benchmark (~0.5 ngày)

Unit tests trong `cache.rs`:

- `roundtrip_symbol_index`: build empty → write → load → assert equal.
- `invalidate_on_schema_mismatch`: write v0 → cố load v1 → expect None.
- `invalidate_on_head_change`: write → mock different HEAD → expect None.

Integration test (`crates/ffs-cli/tests/cli_cache_symbol.rs`):

- Setup tmp repo với 10 file → run `ffs index` → assert `.ffs/` exists.
- Run `ffs symbol foo` 2 lần; lần 2 đo thời gian < 100 ms với criterion.

Benchmark final trên Linux kernel:

```
hyperfine --warmup 2 --runs 5 \
  '/tmp/ffs-new --root /home/ubuntu/test_linux symbol scheduler_init' \
  '/tmp/ffs-new --root /home/ubuntu/test_linux callers vfs_read' \
  '/tmp/ffs-new --root /home/ubuntu/test_linux flow do_sys_open'
```

Expected: tất cả ≤ 200 ms warm.

### 1.6 Risks & mitigation

| Risk | Mitigation |
|------|-----------|
| Postcard format ổn định ngược chiều? | Đã pin schema_version="v1". Khi đổi struct, bump v2 và invalidate cũ. |
| Postcard không serialize được `DashMap`? | Đã xác nhận derived Serialize qua serde — dashmap có feature `serde`. Verify trong `crates/ffs-symbol/Cargo.toml`. |
| `.ffs/` được commit nhầm vào git? | Add vào `.gitignore` mặc định (PR-time edit). |
| Cache stale gây result sai? | head_matches + file_count check; người dùng có thể `ffs index --force` rebuild. |
| Cold path còn 79 s với UX kém? | Print progress line `Indexing… 23k/93k files` mỗi 5 s (đã có hook `ScanProgress` trong `ffs-engine`). |

---

## 2. Polish nhỏ song song

### 2.1 README + docs

- [ ] Cập nhật `README.md` thêm section "MCP auto-install" liệt kê 6
      provider, ví dụ command.
- [ ] Update `README.md` benchmark table với kết quả mới (320 ms vs 1.57 s).
- [ ] Thêm `--help` example trong `ffs grep --help`: `ffs grep -F '.is_file()'`
      và `ffs grep --regex 'fn.*\('` để user thấy auto-detect.

### 2.2 Test trên macOS/Windows (nếu có máy)

- [ ] Smoke test `install-mcp.sh` trên macOS Sonoma (cline path resolution).
- [ ] Smoke test trên Windows PowerShell (`install-mcp.ps1` chưa có MCP
      registrar — port logic từ bash sang ps1).

### 2.3 `.gitignore`

- [ ] Add `.ffs/` vào `.gitignore` root khi M2 merge.

### 2.4 CI

- [ ] Thêm step `cargo test -p ffs-cli` vào GitHub Actions matrix
      (kiểm tra lại nếu chưa có).
- [ ] Thêm benchmark CI gate: fail PR nếu `ffs grep` chậm > 2× rg trên
      fixture chuẩn (tránh regression).

---

## 3. M4 — Daemon mode (optional, deprioritized)

Per PLAN.md §M4: `ffs serve` + file watcher + Unix socket dispatcher.
Trị giá: cold path 0 vì index luôn warm trong RAM của daemon. Phức tạp:
process lifecycle, IPC protocol, auto-start.

**Khuyến nghị: hoãn cho đến khi M2 cache làm xong và đo lại UX.** Vì M2 đã
đưa warm path xuống <50 ms — đủ cho phần lớn use case. Daemon chỉ cần thiết
nếu user complain về `ffs symbol` cold startup mỗi lần repo cập nhật lớn.

---

## 4. Timeline gợi ý

| Tuần | Output |
|------|--------|
| Tuần 1 (3 ngày) | M2 step 1 (cache module) + step 2 (`ffs index` snapshot) + step 3 (`ffs symbol` wire). PR riêng, đo benchmark. |
| Tuần 1 (2 ngày) | M2 step 4 (callers/refs/flow/siblings/impact) + step 5 (incremental). |
| Tuần 2 (1.5 ngày) | M2 step 6 (bigram prefilter cho grep). |
| Tuần 2 (1 ngày) | M2 step 7 tests + benchmark Linux kernel + viết PR description. Polish (README, .gitignore). |
| Tuần 3 (optional) | M4 daemon nếu cần. |

Tổng: **6-8 ngày làm việc** để hoàn thành M2 + polish, không tính review.

---

## 5. Câu hỏi mở (cần Trần confirm trước khi code M2)

1. **Đặt `.ffs/` ở repo root hay home dir?** PLAN.md đề xuất repo root (giống
   `.git/`). Ưu: cache per-repo, dễ invalidate. Nhược: noise trong file
   listing. → **Đề xuất: repo root, add vào `.gitignore` mặc định.**

2. **Auto-invalidate khi user `git checkout`?** Nếu có file watcher tích hợp
   → daemon mode. Không daemon thì check HEAD mỗi invocation (~5 ms overhead).
   → **Đề xuất: check HEAD mỗi invocation, không daemon.**

3. **Multi-root workspace (e.g. monorepo với nhiều cargo project)?** Cache
   1 root duy nhất hay nested? → **Đề xuất: 1 cache per `--root`, user
   responsibility chọn root đúng.**

4. **Cache size limit?** Linux kernel symbol_index ~30 MB nén. Nếu repo
   khổng lồ (Chromium ~5 GB)? → **Đề xuất: chưa giới hạn, nếu phình to
   thì user thấy `.ffs/` size lớn và biết.**

5. **Có persist outline + body chunk hay chỉ symbol?** → **Đề xuất: phase 1
   chỉ symbol_index; outline + bigram phase 2 nếu cần.**

Reply lại 5 câu hỏi này hoặc OK với defaults là em bắt đầu M2 ngay.

---

*Last updated: 2026-05-15 — sau khi PR #28 (M1+M3) được tạo. Sẽ update khi
M2 tiến hành.*
