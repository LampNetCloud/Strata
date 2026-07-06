# Strata S3 — `BatchPolicy` / checkpoint sub-MMR (gộp lô 4-tier) — Báo cáo

> **Người:** Thịnh (`lrybi`) · **Ngày:** 2026-07-06 · **Issue:** `LampNetCloud/Strata#3` [P1]
> **Spec:** `spec/Strata-API.md §8.3` + `spec/Strata-Feat.md §4-tier` · **Contract:** `spec/_CONTRACT.md` CHỐT-2
> **Nhánh:** `thinh/strata-s3-batch-checkpoint` (base `main`) · **Code:** `src/batch.rs`

## 1. Phạm vi + 2 blocker đã được anh Đức chốt (issue #3)

Gộp N version-entry tần suất cao (ProofChat / IoT / register) thành **một checkpoint** (sub-MMR) rồi neo **một lần** thay vì N lần — tránh đẻ version vô hạn (INV / `_CONTRACT.md §Gộp lô`). Giữ khả năng chứng minh một entry lẻ bằng **inclusion hai tầng**.

Anh Đức chốt 2 điểm byte-on-chain (2026-07-03) trước khi code:
- **(a) Domain-tag entry: CÓ**, tách miền khỏi version-leaf (chống type-confusion/second-preimage). Chuỗi chính xác đóng vào `_CONTRACT.md`. → **Chốt dùng `LN/STRATA/entry/v1`** (đúng bảng CHỐT-2 dòng "Batch entry (sub-MMR gộp lô)", tuyên bố "DUY NHẤT"; spec §8.3 phương án (1) cũng bảo toàn chuỗi này). **KHÔNG sửa `_CONTRACT.md`** (giá trị đã có sẵn).
- **(b) `max_entries=10_000` / `flush_on_idle=300s`**: **config runtime**, không đóng byte-layout, không chặn — code với default.

Ranh giới: crate THUẦN (no I/O); vòng gộp epoch theo đồng hồ thật là **lớp daemon** (§5.3 [SPEC-TODO]). Core cung cấp primitive + **hàm quyết định thuần** `should_close` (không timer).

## 2. Hiện thực (`src/batch.rs`, dựng trên `Mmr<Blake3Hasher>` — KHÔNG primitive mới)

| Thành phần | Mô tả |
|---|---|
| `entry_bytes(seq,payload)` | canonical §1.7: `u64_be(seq) ‖ u32_be(len) ‖ payload` (length-prefix chống nhập nhằng nối) |
| `entry_leaf(seq,payload)` | `H_dom("LN/STRATA/entry/v1", entry_bytes)` — 32B, tách miền khỏi `ver/v1` |
| `Checkpoint` | sub-MMR + giữ `leaves`; `append_entry` → `sub.append(entry_leaf)`; `state_root()=sub.root()` = `checkpoint_state_root` |
| `prove_entry`/`verify_entry` | inclusion **tầng dưới** (entry ∈ checkpoint) |
| `BatchPolicy` + `should_close` → `CloseReason` | quyết định thuần đóng epoch: `MaxEntries` (chặn RAM) → `EpochElapsed` → `Idle` |
| `TwoTierProof` + `verify_two_tier` | ghép: (1) entry∈checkpoint + (2) checkpoint-version∈lịch-sử (tái dùng `StrataChain::verify_version`) + (3) **LINK** `version.state_root == checkpoint_state_root` (fail-closed) |

**§8.3b:** checkpoint = một `StrataVersion` BÌNH THƯỜNG với `state_root = checkpoint_state_root` → `chain.append_version` → `publish_anchor` một lần. Không API mới ở core.

## 3. Test — 7 tiêu chí §8.3 (+1 tất định) — **8/8 pass**

| # | Test | Kết quả |
|---|---|---|
| 1 | `checkpoint_1000_versions_one_anchor` | 1000 entry → 1 checkpoint → **1 anchor** (không 1000) ✅ |
| 2 | `prove_entry_in_checkpoint_size` | prove entry #637/1000 verify OK; **sub_proof = 448B** (8 siblings + 6 peaks) — O(log N), khớp khoảng ~320–640B spec; sai leaf → reject ✅ |
| 3 | `two_tier_inclusion_verifies` | ghép sub-proof + version-proof → verify về `mmr_root` đã neo PASS; LINK sai / idx sai → reject ✅ |
| 4 | `close_on_max_entries` | `should_close(10_000,…)=MaxEntries`, `9_999→None` ✅ |
| 5 | `close_on_idle` | im lặng 300s → `Idle`; epoch hết → `EpochElapsed` ✅ |
| 6 | `entry_bytes_canonical` | khác payload/seq → leaf khác; khung `u64‖u32‖payload` đúng ✅ |
| 7 | `crdt_deterministic_state_root` | cùng tập op, thứ tự nhận khác → sort tất định → cùng `checkpoint_state_root` ✅ |
| + | `deterministic_same_entries_same_root` | cùng chuỗi entry → cùng root ✅ |

**Toàn crate:** `cargo test` **61 lib (+8 batch) + 12 integration pass**, `cargo clippy --all-targets` **sạch**, `cargo fmt --check` **clean**.

**Số thật báo cáo (tiêu chí #2):** với sub-MMR N=1000, proof entry lẻ = **448 byte** (14 hash × 32B). Ở quy mô 1 triệu entry, proof ≈ log2(1e6)×32 ≈ 640B — khớp "~640B/1tr version" spec.

## 4. Còn lại (ngoài phạm vi crate thuần / follow-up)

- **Lớp daemon** vòng gộp epoch theo đồng hồ thật (`epoch_secs`/idle-timer) + persist sub-MMR leaves của mỗi epoch (hoặc batch blob qua Mirage) để sinh `sub_proof` về sau — nếu vứt leaves sau checkpoint thì mất khả năng prove entry lẻ (chỉ prove được cả checkpoint). Quyết định lưu-trữ tầng (c)/(d) theo giá trị.
- **CRDT §7.4** (nếu chọn cho register hội tụ): serialize op tất định phía caller (crate chỉ đảm bảo cùng chuỗi entry → cùng root).
- **Neo checkpoint thật** qua S1 sink (`MosaicAnchorSink`) — S3 gộp, S1 neo; đã sẵn ở `thinh/strata-s1-anchor-sink`.
