# Study Report: ripgrep — Speed Techniques & Match Highlighting

> Generated from a deep read of `burntsushi/ripgrep` (v15.2.0, cloned into `.tmp/`).
> Goal: learn what ffs can borrow to get faster and to add the highlight feature rg has.

---

## TL;DR

Hai thứ bạn hỏi, trả lời ngắn:

1. **Highlight**: rg *không bao giờ* tìm lại để lấy vị trí match — highlight là "miễn phí" vì searcher vốn đã trao từng match range cho printer, printer chỉ việc đi kèm escape code ANSI. ffs **đã có** dữ liệu đó (`GrepMatch.match_byte_offsets`) nhưng **chưa render màu nào ra terminal**.

2. **Tốc độ**: ý tưởng lớn nhất mà rg dùng mà ffs chưa có là **literal prefilter ở mức buffer/line** — dùng `memmem` (SIMD) bỏ qua toàn bộ vùng không khớp *trước khi* chạy regex thật. ffs mới prefilter ở mức *file* (bigram index → candidate files), chưa ở mức *line*.

---

## ✅ Quyết định cuối cùng (owner review — 2026-08-10)

| Priority | Việc | Risk | Expected value |
|---|---|---|---|
| **P0** | CLI ANSI highlight từ `match_byte_offsets` | Rất thấp | Feature rõ ràng |
| **P1** | Benchmark Regex vs PlainText hiện tại | Thấp | Xác định bottleneck thật |
| **P1** | Regex literal prefilter (conservative, có benchmark gate) | Trung bình | Potential speedup lớn |
| **P2** | Skip match-span extraction khi consumer không cần | Trung bình | Micro/medium optimization |
| **P3** | bstr / line-buffer optimization | Thấp | Ít priority |

**Nguyên tắc ràng buộc:**

1. **Highlight implement ngay** — engine đã có `match_byte_offsets`, presentation-layer change, risk cực thấp. Không bao giờ tìm lại để lấy match range.

   **Quyết định về base (owner chốt):**
   - CLI `grep` / `multi-grep` hiện là command riêng (đọc file bằng `std::fs::read`, `GrepHit{path,line,text}` — KHÔNG qua engine ffs-core, KHÔNG giữ offsets).
   - → **Chuyển CLI sang engine `ffs-core`**: `FilePicker` + `parse_grep_query` + `picker.grep()` / `picker.multi_grep()` (đã trả `GrepMatch.match_byte_offsets`, mmap, frecency, bigram). MCP/C-ABI đã wire theo pattern này → CLI copy theo.
   - **Scope màu: grep + multi-grep + find fuzzy** (`find` dùng `match_indices` đã có).

   ```rust
   // pattern CLI sẽ model theo MCP:
   let mut picker = FilePicker::new(FilePickerOptions { base_path, watch: false, ..Default::default() })?;
   picker.collect_files()?;
   let q = parse_grep_query(&needle);
   let result = picker.grep(&q, &GrepSearchOptions { mode: GrepMode::Regex, .. });
   // result.matches[i].match_byte_offsets → render highlight
   ```

2. **Regex literal prefilter KHÔNG implement hấp tấp.** Không tự động viết `regex-syntax Extract → memmem Finder → regex wrapper` cho mọi regex — đây là chỗ dễ ra correctness bug, không chỉ perf bug:
   - literal có thể không thực sự là *required* literal;
   - case-insensitive / Unicode;
   - alternation, repetition;
   - look-around / anchor semantics;
   - nhiều literal với selectivity khác nhau;
   - prefilter phải đảm bảo **không bao giờ false-negative**.

   Thứ tự theo phase:

   ```text
   Phase 1: benchmark ffs Regex hiện tại (đối chứng PlainText)
   Phase 2: xác định pattern classes có required literal chắc chắn
   Phase 3: implement conservative prefilter
            │
            ▼
         benchmark
   Phase 4: mở rộng extraction nếu benchmark chứng minh đáng
   ```

   > **Không** assume con số 10–50x của rg. Nó phụ thuộc pattern, file size, match density, CPU, mmap, encoding. Đo bằng benchmark thật của ffs.

3. **Không đụng parallelism hiện tại.** Với use case pagination / early-exit theo file của ffs, file-level Rayon có thể tốt hơn line-level parallelism của rg.

4. **File-level + within-file là hai optimization bổ sung, không thay thế nhau:**

   ```text
   ffs hiện tại:           bigram index → candidate files → search whole file
   rg-style bổ sung:       candidate files → candidate regions/lines → regex verification
   ```

---

## 1. Match Highlighting trong ripgrep

### Sơ đồ tổng quan

```
searcher (candidate-line search)
   │  tìm ra từng dòng match + match ranges (byte offsets)
   ▼
printer (crates/printer/src/standard.rs)
   │  nếu cần highlight → write_colored_matches()
   ▼
stdout:  path:line: <match đỏ đậm>
```

### Cơ chế chính (`crates/printer/src/standard.rs`)

- **Quyết định màu một lần duy nhất** (`standard.rs:578-583`):

  ```rust
  fn needs_match_granularity(&self) -> bool {
      let match_colored = !self.config.colors.matched().is_none();
      (supports_color && match_colored)
  }
  ```

  Nếu tắt màu → printer **không tính match spans gì cả**, chỉ in dòng trần. Đây là nguyên tắc "đừng trích xuất highlight nếu consumer không cần".

- **Tách fast/slow sink** (`standard.rs:705, 766, 969`): printer có path "fast" (không cần per-match spans) và path "slow" (chỉ khi highlight / only-matching bật). ffs nên áp dụng nguyên tắc tương tự.

- **Phát match** — `write_colored_matches` (`standard.rs:1247-1288`): con trỏ tuyến tính đi qua các match range đã sort. Với mỗi match so với con trỏ dòng, hoặc in text trước match, hoặc in text match có màu:

  ```rust
  if line.start() < m.start() { end_color_match();  write(&bytes[line.with_end(upto)]) }
  else                        { start_color_match(); write(&bytes[line.with_end(upto)]) }
  ```

- **Màu mặc định** (`crates/printer/src/color.rs:14-24`), chọn conservative chạy được trên cả theme sáng/tối:

  | Thành phần | Màu |
  |---|---|
  | `path` | fg magenta (cyan trên Windows) |
  | `line` | fg green |
  | `match` | **fg red + bold** |

- Dùng crate **`termcolor`** (tự xử lý `NO_COLOR` / `CLICOLOR` / Windows VT vs WinAPI).

### Hiện trạng ffs

- Engine đã tính `GrepMatch.match_byte_offsets: SmallVec<[(u32,u32);4]>` đầy đủ (plain, regex, Aho-Corasick, fuzzy — `crates/ffs-core/src/grep/grep.rs`).
- C API + MCP đã expose highlight ranges.
- **Nhưng CLI không render màu gì**: `commands/mod.rs:43 emit()` chỉ `println!` text trần; không có `termcolor`, không `IsTerminal`, không `NO_COLOR` trong toàn bộ codebase.
- Demo web (`demo/src/App.tsx:51`) tự highlight bằng JS thuần (khác cơ chế, chỉ để tham khảo UI).

---

## 2. Bí quyết tốc độ: `find_candidate_line` (candidate-line prefilter)

### Mô hình của ripgrep

rg **không bao giờ** chạy regex trên từng dòng. Flow chuẩn:

```
regex ──► regex-automata
          │
          ├─ 1. trích "inner literals" (phân tích prefix/suffix của pattern)
          ├─ 2. build fast_literal_regex (vd: "foo" từ "\w+foo\w+")
          │
          ▼
   matcher.find_candidate_line(haystack)        (crates/regex/src/matcher.rs:491-506)
          │  = memmem search trên TOÀN buffer (bỏ qua vùng không khớp)
          │
          ├─ LineMatchKind::Candidate(offset)   ──► locate() dòng chứa offset
          │                                          │
          │                                          ▼
          │                              chạy regex THẬT trên ĐÚNG dòng đó
          │
          └─ LineMatchKind::Confirmed(offset)  (khi không trích được literal — fallback)
```

Trong searcher core (`crates/searcher/src/searcher/core.rs:385-519`):

- `match_by_line_fast` → `find_by_line_fast` lặp tìm **candidate lines**, bỏ qua toàn bộ phần còn lại.
- Khi không trích được literal → fallback `find_by_line_slow` — **đúng thứ ffs đang làm mọi lúc**.

### Ánh xạ vào ffs

- ffs **đã làm được nửa** tối ưu này ở `PlainText` mode: `PlainTextMatcher::find_at` dùng `memchr::memmem::find` trên phần buffer còn lại rồi searcher `locate` dòng (`grep.rs:314-332`). Đó chính là path `Confirmed`.
- **Thiếu** cùng thủ thuật đó cho `Regex` mode và multi-pattern mode.

### Ý tưởng cụ thể (gọn, khớp cấu trúc ffs-grep)

> **Regex mode**: dùng `regex-syntax` (pure dep) trích 1 required literal làm anchor. Nếu có, build `memmem::Finder` cho literal đó, bọc matcher regex để `find_at` chạy `finder.find` trước — trả về candidate line — chỉ khi trúng mới chạy regex thật lên dòng đó.
>
> **Multi-pattern**: regex crate của Rust tự làm điều này nội bộ với `build_literals` → alternation.

**Vì sao đây là win lớn nhất**: `PlainText` vốn đã SIMD; khoảng trống nằm ở path `Regex`/`multi-grep` đang trả giá regex automaton trên mọi buffer. Memchr (AVX2 two-way) bỏ qua hàng KB mỗi lần quét. Tài liệu rg ghi candidate-line search nhanh hơn line-by-line **10-50x** trong trường hợp phổ biến.

---

## 3. Kỹ thuật tốc độ khác (ffs đã có sẵn đầu tàu)

| Kỹ thuật | ripgrep | ffs |
|---|---|---|
| Mmap read | mmap mặc định cho file lớn | ✅ đã có `MmapSlot` + `get_content_for_search` |
| Aho-Corasick cho literal | có | ✅ đã có trong `multi_grep` |
| Parallelism | chia *dòng* trong từng file | chia *file* qua rayon — **tốt hơn cho early-exit pagination**, không cần đổi |
| `bstr` (tránh UTF-8 validate) | dùng khắp nơi | ⚠️ dùng `String::from_utf8_lossy` mỗi match line — chi phí nhỏ mỗi hit |
| Line buffer / context | `line_buffer.rs` (35KB, phức tạp: CRLF, rolling buffer) | đơn giản hơn — đúng mức cần cho ffs |

---

## 4. Khuyến nghị hành động (theo thứ tự ưu tiên)

### #1. (P0) CLI ANSI highlight — **trên engine ffs-core**

> **Refactor đổi scope** (owner chốt): CLI grep/multi-grep chuyển từ command tự scan → engine `ffs-core`. Đây không còn "chỉ là presentation" — gồm 2 phần:

**1a. Wire CLI `grep` / `multi-grep` sang engine `ffs-core`**
- Model theo MCP: `FilePicker::new(base_path, watch:false)` + `collect_files()` + `parse_grep_query(&needle)` + `picker.grep(&q, &opts)` / `picker.multi_grep(&patterns, &[], &opts)`.
- Giữ lại các flag hiện có (`--limit`, `--max-count`, `-l`, `--regex`, `-F`, `-w`, `--group`): map vào `GrepMode` / `GrepSearchOptions` / constraint.
- Output `GrepHit` → mở rộng thành dùng `GrepMatch.match_byte_offsets`.
- JSON schema giữ tương thích (`path`, `line`, `text` giữ nguyên; thêm `match_ranges` optional).

**1b. Renderer highlight + màu `find` fuzzy**
- `termcolor` + `std::io::IsTerminal`; match red/bold; path/line theo màu mặc định của rg (magenta/green).
- Tôn trọng `NO_COLOR`; `--format json` và pipe ra text trần.
- `find`: highlight phần fuzzy match qua `match_indices` đã có.
- Truyền tín hiệu `is_tty` xuống `commands/mod.rs:emit` (hiện chỉ `OutputFormat::Text/Json`).

### #2. (P1) Benchmark Regex vs PlainText — TRƯỚC tiên

- Đo pattern classes khác nhau (literal-heavy, anchor, alternation, unicode…), file size, match density.
- Đối chứng PlainText (đã SIMD) để xác định bottleneck thật, **không** dùng số của rg.
- Quyết định có nên prefilter cho Regex không dựa trên kết quả này.
- **Ghi chú sau refactor P0**: vì CLI giờ chạy chung engine ffs-core, benchmark có thể chạy thẳng trên `FilePicker` — không cần giữ CLI cũ để đối chứng.

### #3. (P1) Regex literal prefilter — có benchmark gate

- Trích required literal bằng `regex-syntax`'s `Extract` (conservative, đúng class đã xác định ở #2).
- Memoize `memmem::Finder`; short-circuit `find_at` → candidate line trước khi chạy regex.
- Prefilter phải **không bao giờ false-negative**. Sai ở đây là correctness bug.

### #4. (P2) Skip match-span extraction theo consumer capability

> **Correction (owner)**: decision KHÔNG chỉ theo TTY — theo **consumer capability/requirement**:

**Decision theo consumer requirement — không chỉ TTY:**

```rust
needs_match_ranges =
    output.requires_match_ranges()        // JSON / MCP / API / --only-matching
    || (output.is_tty() && output.color_enabled());
```

| Tình huống | Tính spans? |
|---|---|
| TTY + color | ✅ YES |
| TTY + `--color=never` | ❌ NO |
| pipe | ❌ NO |
| JSON | ✅ YES |
| MCP | ✅ YES |

Kiến trúc sạch hơn và sau này `--only-matching`, structured output, API consumer không bị phụ thuộc vào TTY.

### #5. (P3) bstr / line-buffer optimization

- Ít ưu tiên, chỉ khi benchmark chứng minh có vấn đề.

---

## Reference files

| File (trong `.tmp/`) | Nội dung |
|---|---|
| `crates/regex/src/matcher.rs` | `find_candidate_line`, literal extraction |
| `crates/searcher/src/searcher/core.rs` | `match_by_line_fast/slow`, candidate-line loop |
| `crates/printer/src/standard.rs` | `write_colored_matches`, `needs_match_granularity` |
| `crates/printer/src/color.rs` | color spec defaults (`path:magenta`, `line:green`, `match:red+bold`) |
| `crates/matcher/src/lib.rs` | `Matcher` trait (find / find_candidate_line / line_terminator) |

`ripgrep` clone giữ nguyên tại `.tmp/` để tham khảo. Có thể xoá bất cứ lúc nào.
