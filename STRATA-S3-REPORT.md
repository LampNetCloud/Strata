# Strata S3 — `BatchPolicy` / checkpoint sub-MMR (gộp lô 4-tier) — Báo cáo

> **Người:** Thịnh (`lrybi`) · **Ngày:** 2026-07-06 (vá review vòng 1: 2026-07-08 · vòng 2 + Addendum S3.1: 2026-07-09) · **Issue:** `LampNetCloud/Strata#3` [P1]
> **Spec:** `spec/Strata-API.md §5.3 + §8.3` (addendum 04/07 chốt tại **PR #8**) + `spec/Strata-Feat.md §4-tier` · **Contract:** `spec/_CONTRACT.md` CHỐT-2
> **Nhánh:** `thinh/strata-s3-batch-checkpoint` (base `main`) · **Code:** `src/batch.rs`

## 1. Phạm vi + 2 blocker đã được anh Đức chốt (issue #3)

Gộp N version-entry tần suất cao (ProofChat / IoT / register) thành **một checkpoint** (sub-MMR) rồi neo **một lần** thay vì N lần — tránh đẻ version vô hạn (INV / `_CONTRACT.md §Gộp lô`). Giữ khả năng chứng minh một entry lẻ bằng **inclusion hai tầng**.

Anh Đức chốt 2 điểm byte-on-chain (2026-07-03) trước khi code:
- **(a) Domain-tag entry: CÓ**, tách miền khỏi version-leaf (chống type-confusion/second-preimage). Chuỗi chính xác đóng vào `_CONTRACT.md`. → **Chốt dùng `LN/STRATA/entry/v1`** (đúng bảng CHỐT-2 dòng "Batch entry (sub-MMR gộp lô)", tuyên bố "DUY NHẤT"; spec §8.3 phương án (1) cũng bảo toàn chuỗi này). **KHÔNG sửa `_CONTRACT.md`** (giá trị đã có sẵn).
- **(b) `max_entries=10_000` / `flush_max_age=300s`**: **config runtime**, không đóng byte-layout, không chặn — code với default. (Addendum §5.3 PR #8 đổi `flush_on_idle`→`flush_max_age` — xem §5 vá review.)

Ranh giới: crate THUẦN (no I/O); vòng gộp epoch theo đồng hồ thật là **lớp daemon** (§5.3 [SPEC-TODO]). Core cung cấp primitive + **hàm quyết định thuần** `should_close` (không timer).

## 2. Hiện thực (`src/batch.rs`, dựng trên `Mmr<Blake3Hasher>` — KHÔNG primitive mới)

| Thành phần | Mô tả |
|---|---|
| `entry_bytes(seq,payload)` | canonical §1.7: `u64_be(seq) ‖ u32_be(len) ‖ payload` (length-prefix chống nhập nhằng nối) |
| `entry_leaf(seq,payload)` | `H_dom("LN/STRATA/entry/v1", entry_bytes)` — 32B, tách miền khỏi `ver/v1` |
| `Checkpoint` | sub-MMR + giữ `leaves`; `append_entry` → `sub.append(entry_leaf)`; `state_root()=sub.root()` = `checkpoint_state_root` |
| `prove_entry`/`verify_entry` | inclusion **tầng dưới** (entry ∈ checkpoint) |
| `BatchPolicy` + `should_close` → `CloseReason` | quyết định thuần đóng epoch: `MaxEntries` (chặn RAM) → `EpochElapsed` → `FlushMaxAge` (tuổi entry cũ nhất ≥ ngưỡng) |
| `EpochAccumulator` (`new/push/should_close/close`) | driver §5.3 có state: watermark chống replay sống-xuyên-close, enforce `max_entries` tại push, `oldest_ts`=min; `close()`→`ClosedEpoch{sub_mmr_root, sub_size, entries, entries_serialized}` |
| `serialize_batch`/`parse_batch`/`batch_root` | blob lô canonical + parse **strict** (cụt/thừa/count sai → `MalformedBatch`) dựng lại đúng root — nguồn `content_cid` (§8.3c) |
| `TwoTierProof` + `verify_two_tier(&StrataVersion)` | ghép: (1) entry∈checkpoint dưới `version.state_root` + (2) checkpoint-version∈lịch-sử (`StrataChain::verify_version`). **LINK là hệ quả cấu trúc** — root/vh/seq đọc thẳng từ version đã neo, không phải input rời prover khai |

**§8.3b:** checkpoint = một `StrataVersion` BÌNH THƯỜNG với `state_root = checkpoint_state_root` → `chain.append_version` → `publish_anchor` một lần. Không API mới ở core.

## 3. Test — 7 tiêu chí §8.3 + tất định + 13 test bổ sung review — **PASS**

| # | Test | Kết quả |
|---|---|---|
| 1 | `checkpoint_1000_versions_one_anchor` | 1000 entry → 1 checkpoint → **1 anchor** (không 1000) ✅ |
| 2 | `prove_entry_in_checkpoint_size` | prove entry #637/1000 verify OK; **sub_proof = 448B** (8 siblings + 6 peaks) — O(log N), khớp khoảng ~320–640B spec; sai leaf → reject ✅ |
| 3 | `two_tier_inclusion_verifies` | ghép sub-proof + version-proof → verify về `mmr_root` đã neo PASS; `&StrataVersion` state_root khác / idx sai → reject ✅ |
| 4 | `close_on_max_entries` | `should_close(10_000,…)=MaxEntries`, `9_999→None` ✅ |
| 5 | `close_on_flush_max_age` | oldest già 300s → `FlushMaxAge` **dù tin mới rả rích** (oldest-age ≠ idle) ✅ |
| 6 | `entry_bytes_canonical` | khác payload/seq → leaf khác; khung `u64‖u32‖payload` đúng ✅ |
| 7 | `crdt_deterministic_state_root` | cùng tập op, thứ tự nhận khác → sort tất định → cùng `checkpoint_state_root` ✅ |
| + | `deterministic_same_entries_same_root` | cùng chuỗi entry → cùng root ✅ |

**Toàn crate (sau vá vòng 2 + Addendum S3.1 — xem §6):** `cargo test` **86 unit (+batch) + 12 integration = 98 pass**, `cargo clippy --all-targets` **0 warning**, `batch.rs` fmt clean. (Vòng 1: 85 pass.)

**Số thật báo cáo (tiêu chí #2):** với sub-MMR N=1000, proof entry lẻ = **448 byte** (14 hash × 32B). Ở quy mô 1 triệu entry, proof ≈ log2(1e6)×32 ≈ 640B — khớp "~640B/1tr version" spec.

## 4. Còn lại (ngoài phạm vi crate thuần / follow-up)

- **Lớp daemon** vòng gộp epoch theo đồng hồ thật (`epoch_secs`/idle-timer) + persist sub-MMR leaves của mỗi epoch (hoặc batch blob qua Mirage) để sinh `sub_proof` về sau — nếu vứt leaves sau checkpoint thì mất khả năng prove entry lẻ (chỉ prove được cả checkpoint). Quyết định lưu-trữ tầng (c)/(d) theo giá trị.
- **CRDT §7.4** (nếu chọn cho register hội tụ): serialize op tất định phía caller (crate chỉ đảm bảo cùng chuỗi entry → cùng root).
- **Neo checkpoint thật** qua S1 sink (`MosaicAnchorSink`) — S3 gộp, S1 neo; đã sẵn ở `thinh/strata-s1-anchor-sink`.

---

## 5. Vá theo review anh Đức (PR #7 — 2026-07-08)

Anh Đức kéo nhánh chạy thật (61 lib + 12 integration pass, proof 448B khớp), khen phần lõi chuẩn (domain-tag `entry/v1` giữ CHỐT-2, `entry_bytes` §1.7, checkpoint qua `append_version` không đẻ API mới, `TwoTierProof` đóng struct tốt). Sau đối chiếu spec mới (PR #8) nêu 8 việc — **mục 1–4 chặn merge, 5–8 gộp cho trọn S3**. Đã xử lý cả 8:

| # | Việc | Cách vá |
|---|---|---|
| 1 ⛔ | **§5.3 `flush_on_idle`→`flush_max_age`** | `should_close` đổi tham số `last_entry_ts`→`oldest_ts` (min, không đòi đơn điệu); đóng van (c) khi `now - oldest_ts ≥ flush_max_age` — tin rả rích vẫn gom 1 checkpoint nhưng entry cũ nhất không chờ quá hạn. `CloseReason::Idle`→`FlushMaxAge`. Thêm **`EpochAccumulator`** (driver có state) theo đúng hợp đồng API §5.3. Test #5 → `close_on_flush_max_age`. |
| 2 ⛔ | **Chống replay `entry_seq`** | Watermark `last_entry_seq` tăng nghiêm ngặt, **sống xuyên `close()`** (reset epoch nhưng giữ watermark) → daemon retry sau crash không băm đôi entry vào 2 checkpoint. `push` trả `ReplaySeq{last,got}` khi seq cũ/bằng. Test `replay_seq_across_epoch_close`. |
| 3 ⛔ | **Enforce `max_entries` tại push** | `push` trả `EpochFull` NGAY khi epoch đầy (entry thuộc epoch SAU; watermark chưa cập nhật nên push lại sau `close` không bị coi replay). 2 góc chết: **epoch rỗng không đóng** (`should_close(0,…)=None`, trước trả `EpochElapsed`); **`max_entries=0` fail-closed** (mọi push `EpochFull`, không đóng lặp vô hạn). Test `max_entries_enforced_at_push`, `max_entries_zero_fail_closed`, `empty_epoch_never_closes`. |
| 4 ⛔ | **Guard payload > u32::MAX** | `entry_bytes` trả `PayloadTooLarge{len}` **trước khi** gọi `u32_be` (chặn truncate lặng lẽ gãy canonical). Lan lên `entry_leaf`/`append_entry`/`push` (Result). |
| 5 | **Blob lô canonical + parse strict** | `serialize_batch` = `u32_be(count) ‖ entry_bytes*`; `parse_batch` strict (byte cụt / thừa đuôi / count sai → `MalformedBatch`); `batch_root` dựng lại root từ blob (§8.3c — nguồn `content_cid`). `ClosedEpoch.entries_serialized` mang blob này. Test `accumulator_close_matches_direct_checkpoint`, `tamper_batch_blob_detected`. |
| 6 | **`proofchat()` + `u32`** | `BatchPolicy::proofchat() = {600, 4096, 180}`; `max_entries: usize`→**`u32`** khớp chữ ký §5.3. Test `proofchat_profile`. |
| 7 | **Siết `verify_two_tier`** | Bỏ `checkpoint_vh`/`checkpoint_state_root`/`checkpoint_seq` prover tự khai khỏi `TwoTierProof`; hàm **nhận thẳng `&StrataVersion`**, tự `version_hash()` + đọc `state_root` từ chính version đã neo → LINK là **hệ quả cấu trúc**, không còn phép so 2 input rời. |
| 8 | **Test bổ sung** | replay xuyên epoch, tamper blob sau đóng (kèm đối chứng pass), ts lùi trong epoch (`non_monotonic_ts_uses_min`), clock-skew saturating không panic, push từ chối không ghi gì, payload rỗng hợp lệ, ưu tiên close. |

**Ghi chú spec:** code khớp §5.3/§8.3 bản mới do anh Đức chốt ở **PR #8** (chưa merge main). Không nhân bản thay đổi spec vào PR này (thuộc PR #8); khi #8 lên main sẽ rebase. Sau vá: **85 pass / 0 fail, clippy 0 warning, fmt clean**.

---

## 6. Vá vòng 2 + Addendum S3.1 (PR #7 — 2026-07-09, sau khi PR #8 lên main)

PR #8 đã **merged main** (2026-07-09) — rebase nhánh S3 lên main mới. Anh Đức rà sâu từng dòng `batch.rs @ 583bc0b` (15/15 hợp đồng spec ĐẠT), bắt thêm 3 lỗi + gộp 4 mục **Addendum S3.1** (spec §8.3, PR #8 `cfa4970`). Đã xử lý cả 7:

| # | Việc | Cách vá |
|---|---|---|
| 1 ⛔ CAO | **DoS alloc `parse_batch`** | Blob khai `count = u32::MAX` từng ép `Vec::with_capacity(count)` xin ~137 GB (blob qua Mirage = untrusted). Vá: cap `count.min((bytes.len()-5)/12)` (mỗi entry ≥ 12B) **trước** `with_capacity`; count dư để vòng parse tự bắt `MalformedBatch`. Test `parse_batch_dos_count_capped` (blob 5-byte `FF FF FF FF` + blob khai 1 triệu entry chỉ có 1). |
| 2 | **`serialize_batch` nuốt lỗi im lặng** | Trước: entry quá cỡ bị `if let Ok` bỏ qua nhưng `count` đã đếm → blob tự-mâu-thuẫn. Vá: `serialize_batch` **trả `Result`**, `entry_bytes(...)?` propagate `PayloadTooLarge`. `close()` cũng trả `Result<ClosedEpoch, _>`. Test `serialize_batch_returns_result`. |
| 3 | **Doc overclaim** | Sửa câu "retry sau crash không băm đôi" — watermark chỉ sống **TRONG-TIẾN-TRÌNH (RAM)**; crash thật mất watermark, chống-băm-đôi qua-crash là việc daemon persist (ngoài phạm vi core thuần). Doc `EpochAccumulator` + bảng §5 #2 nêu rõ phạm vi. |
| A1 | **Blob header version byte** | `serialize_batch = u8(format_version=1) ‖ u32_be(count) ‖ entries…`; `parse_batch` đọc + kiểm version byte, **version lạ → `MalformedBatch`** (không parse mù format tương lai). Const `BATCH_FORMAT_VERSION = 1`. Test `serialize_batch_has_version_header`, `parse_batch_rejects_unknown_version`. |
| A2 | **`entry_seq` LIÊN TỤC (chống gap)** | Không chỉ "tăng nghiêm ngặt" mà bắt `= prev+1`; seq đầu khi watermark `None` **PHẢI là 0** (daemon cấp từ 0). Gap tiến → `NonContiguousSeq{expected,got}` (mất entry LỘ ngay điểm ghi). Enforce **xuyên close** (watermark sống tiếp). Test `entry_seq_must_be_contiguous`, `contiguous_seq_across_close`. |
| A3 | **Van (c) arrival-time** | `push(entry_seq, ts, …)` → **`arrival_ts`** (thời điểm daemon NHẬN entry, KHÔNG dùng ts client khai — ts giả quá khứ sẽ ép đóng epoch liên tục). Đổi tên tham số + doc `oldest_ts`/`epoch_start_ts` = arrival-time. |
| A4 | **`max_epoch_bytes` van b′** | Thêm trường `max_epoch_bytes: u32` (default **64 MiB**, `proofchat` **16 MiB**); `EpochAccumulator` cộng dồn `epoch_bytes`; `should_close` thêm nhánh `MaxEpochBytes` — `max_entries` một mình không chặn 10k×1 MiB = 10 GiB. Ưu tiên van: **MaxEntries → MaxEpochBytes → EpochElapsed → FlushMaxAge**. Test `close_on_max_epoch_bytes` + `close_priority_order` (thêm assert MaxEpochBytes). |

**Ghi chú thứ tự van b′:** `max_epoch_bytes` đặt ngay sau `max_entries` (cả hai là trần RAM cứng, check trước) — chọn fail-closed. Nếu anh Đức muốn vị trí khác trong chuỗi ưu tiên thì đổi 1 dòng.

**Ghi chú `close()` trả `Result`:** vì `serialize_batch` giờ trả `Result` (mục 2), `close()` propagate `Result<ClosedEpoch, BatchError>`; call site đổi `let _ = acc.close();` → `acc.close().unwrap();`. Thực tế `Err` bất khả (push đã guard) nhưng không nuốt để blob không bao giờ tự-mâu-thuẫn.

**Kết quả sau vá vòng 2:** `cargo test` **86 unit (+batch) + 12 integration = 98 pass / 0 fail**, `cargo clippy --all-targets` **0 warning**, `batch.rs` fmt clean. (`derived_index.rs` báo fmt-diff là do rustfmt-version local lệch với file đã-merge của main, KHÔNG thuộc thay đổi PR này.)
