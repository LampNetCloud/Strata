# Strata — Roadmap thực thi

> **Repo:** `LampNetCloud/Strata` (Rust) · **Cập nhật:** 2026-07-04
> **Strata** = tầng lưu trữ tiến hóa của MagicLamp: chuỗi version hash-link + MMR + `state_root` field-Merkle + anchor on-chain + audit-log.
> **Spec nguồn:** `spec/_CONTRACT.md` (khế ước giao diện) + `spec/Strata-Feat/Math/Tech/API.md`.
> **Theo dõi công việc:** issue `#1` (S1) · `#2` (S2) · `#3` (S3).

---

## 0. Bối cảnh

- Module hạ tầng dùng chung xuyên nền tảng (OriLife, ProofChat, AladinWork, VeData…) qua **Strata API**.
- Nối VeData: `Strata.anchor → Mosaic` (CIP-68) — Strata neo on-chain qua Mosaic của VeData (issue S1).
- Bất biến: INV-E1..E9 (append-only; INV-E5 CID không lộ loại nội dung; INV-E6 field-proof ghép ZK; INV-E7 anchor chống rollback).

---

## 1. Baseline / dependency

- **PR #4** (`chore(deps): repoint lampnet-merkle-anchor → repo tách Anchor + pin rev`) — ✅ **MERGED 2026-07-04.**
- `lampnet-merkle-anchor` tách khỏi repo private `lampnet-hivemind` sang repo riêng **`LampNetCloud/Anchor`**; `Cargo.toml` repoint sang Anchor + pin git-rev. Không còn phụ thuộc quyền đọc `lampnet-hivemind`.
- `cargo build` / `cargo test` chạy được với Anchor git+rev — **65 test pass**, không đổi code Strata.

---

## 2. Milestone

| ID | Tên | Phần | Mức độ | Trạng thái |
|---|---|---|---|---|
| **S1** | Anchor adapter `Strata.anchor → Mosaic` (CIP-68) | Rust + on-chain | P0 | **Chốt kỹ thuật (issue #1, 07-03):** `AnchorSink` = trait cắm được, mặc định theo platform (VeData→`MosaicAnchorSink`, CIP-68 datum §8.1); không khoá backend toàn cục. OriLife label 1454/1455 KHÔNG hội tụ (thêm sink riêng sau nếu cần). `resolve()` dùng bảng `anchored: Vec<(seq, mmr_root, mmr_size)>` ở **daemon**; `StrataChain` core giữ thuần (chỉ `mmr_size` hiện tại). ⚠️ Còn **2 điểm đối chiếu byte-layout** (§3) trước khi cố định datum. |
| **S2** | DerivedIndex / columnar query engine | Rust thuần | P1 | ✅ **CODE-COMPLETE 2026-07-04** (`STRATA-S2-REPORT.md`) — module `src/derived_index.rs`: `VersionLog`/`Query`/`DerivedIndex`+`ColumnarIndex`/`brute_force` + **proof 2 tầng** `CompositeFieldProof`+`verify_composite` (ghép `state::prove_field` × `chain::prove_version`, bind qua `version.state_root`) + `InMemoryVersionLog` (state_fields off-chain, ràng buộc `build_state_root==version.state_root`). **7 test** (5 tiêu chí §8.2 + 2 negative), benchmark n∈{1e2..1e5} (build O(n), lookup_latest O(1)). Repo **71 pass / 0 warning / clippy sạch**. Không primitive mới, không write-back (§7.5). Còn: ráp daemon thật + HTTP §3 + Backend sign-off. |
| **S3** | BatchPolicy / checkpoint sub-MMR (4 tầng lưu) | Rust | P1 | **Chốt kỹ thuật (issue #3, 07-03):** (a) domain-tag entry CÓ, namespace `LN/STRATA/...`, tách tag version-leaf (chống type-confusion) — chuỗi tag chính xác đóng vào `_CONTRACT.md` trước khi neo lần đầu; (b) `max_entries=10_000` / `flush_on_idle=300s` là **config runtime** (không đụng byte-layout), chỉnh sau từ telemetry ProofChat. Dùng lại `Mmr<Blake3Hasher>` sẵn có, phụ thuộc **S1 sink** để neo checkpoint. |

**Thứ tự phụ thuộc:** S1 (cố định byte-layout) → S3 (phụ thuộc S1 sink + tag) · S2 độc lập (build đã thông).

---

## 3. Điểm cần đối chiếu trước khi cố định byte-layout S1

Mô tả trong comment issue lệch với `spec/_CONTRACT.md` hiện tại — cần thống nhất bản chuẩn trước khi cố định datum neo.

1. **Byte-layout anchor.**
   - Comment issue #1: `{version_hash, mmr_root, state_root, seq}`.
   - `_CONTRACT.md`: `anchor = (ref_id, head_version_hash, mmr_root, seq)` = 32+32+32+8 = **104 byte**.
   - Khác biệt: `state_root` (comment) so với `ref_id` (`_CONTRACT.md`).
2. **Chuỗi domain-tag checkpoint sub-MMR.**
   - Comment issue #3: `LN/STRATA/checkpoint/entry/v1`.
   - `_CONTRACT.md` bảng CHỐT-2 hiện có `LN/STRATA/entry/v1` (Batch entry sub-MMR gộp lô).
   - Chuỗi cuối cần đóng vào `_CONTRACT.md` (CHỐT-2 là bảng domain-tag DUY NHẤT).

---

## 4. Ràng buộc kỹ thuật cố định (`_CONTRACT.md`)

- **Hash nền:** BLAKE3 32 byte. Domain-separated `H_dom(tag, x) = BLAKE3(tag ‖ 0x00 ‖ x)`. RFC 6962 prefix (leaf `0x00`, internal `0x01`). Dup-leaf guard (CVE-2012-2459): số leaf lẻ xử lý theo MMR carry, không copy leaf cuối.
- **CHỐT-1:** `version_hash = H_dom("LN/STRATA/ver/v1", canonical(core))` KHÔNG gồm `sig`. Tamper-evidence từ chữ ký canonical Ed25519 low-S trên `version_hash`.
- **CHỐT-3:** `mmr_root = H_dom("LN/STRATA/mmr/root/v1", u64_be(n) ‖ bag_of_peaks)` — commit số lá `n` trước khi bag peaks.
- **CHỐT-4:** `value_cid` của một trường state là `content_cid` thuần (không class byte / doc_type) — nếu không sẽ leak loại qua field-proof (INV-E5/E6).
- **CHỐT-5:** `Did` lưu `[u8;32]` (băm DID PhoenixKey); verify chữ ký cần ánh xạ `Did → pubkey` qua key-registry lampnet-join/PhoenixKey, không giả định `Did == pubkey`.
