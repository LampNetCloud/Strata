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
- **PR #8** (`docs(spec): addendum §5.3/§8.3 + chốt Settlement + nghiệm thu T1–T6 Preview`) — ✅ **MERGED 2026-07-09** (nền spec; Addendum S3.1 tại `cfa4970`).
- **PR #5** (S2 DerivedIndex) — ✅ **MERGED 2026-07-09** (vá review vòng 2: comment test build_log).
- **PR #7** (S3 BatchCheckpoint) — ✅ **MERGED 2026-07-09** (vá vòng 2 + Addendum S3.1: DoS parse_batch, seq liên tục, van b′).
- **Thứ tự merge anh Đức chốt:** #8 → #5 → #7 → #6. Đang xử lý #6 (vòng 2) — PR cuối.
- `lampnet-merkle-anchor` tách khỏi repo private `lampnet-hivemind` sang repo riêng **`LampNetCloud/Anchor`**; `Cargo.toml` repoint sang Anchor + pin git-rev. Không còn phụ thuộc quyền đọc `lampnet-hivemind`.
- `cargo build` / `cargo test` chạy được với Anchor git+rev — **65 test pass**, không đổi code Strata.

---

## 2. Milestone

| ID | Tên | Phần | Mức độ | Trạng thái |
|---|---|---|---|---|
| **S1** | Anchor adapter `Strata.anchor → Mosaic` (CIP-68) | Rust + on-chain | P0 | ✅ **CODE-COMPLETE + NEO ON-CHAIN THẬT (L1 Preview) + VÁ 2 VÒNG REVIEW** (`STRATA-S1-REPORT.md`) — `src/anchor_sink.rs`: `MosaicAnchorSink`/`AnchorSink` + datum CIP-68 §8.1 + **thread-token NFT one-shot** chống đầu độc + `publish_with_retry` + `AnchoredTable(ref_id,seq)` + `chain::prove_version_at`/`verify_resolved`. Phase 2: neo datum CIP-68 THẬT + validator Plutus V3 spend-recreate THẬT (INV-E7 on-chain) Preview. **PR #6** (base main, rebase sau #8/#5/#7). Vòng 2 (07-09): `new_unverified_for_tests()` + WARN anchor-thật-hỏng + test biên `prove_version_at` + gộp check version_hash. **111 pass / clippy sạch / fmt clean.** Đợi merge (cuối chuỗi). **PR follow-up:** codec Settlement + hợp nhất `AnchoredTable` (backend mặc định = Settlement theo PR #8). |
| **S2** | DerivedIndex / columnar query engine | Rust thuần | P1 | ✅ **CODE-COMPLETE 2026-07-04** (`STRATA-S2-REPORT.md`) — module `src/derived_index.rs`: `VersionLog`/`Query`/`DerivedIndex`+`ColumnarIndex`/`brute_force` + **proof 2 tầng** `CompositeFieldProof`+`verify_composite` (ghép `state::prove_field` × `chain::prove_version`, bind qua `version.state_root`) + `InMemoryVersionLog` (state_fields off-chain, ràng buộc `build_state_root==version.state_root`). **7 test** (5 tiêu chí §8.2 + 2 negative), benchmark n∈{1e2..1e5} (build O(n), lookup_latest O(1)). Repo **71 pass / 0 warning / clippy sạch**. Không primitive mới, không write-back (§7.5). Còn: ráp daemon thật + HTTP §3 + Backend sign-off. |
| **S3** | BatchPolicy / checkpoint sub-MMR (4 tầng lưu) | Rust | P1 | ✅ **CODE-COMPLETE + VÁ 2 VÒNG REVIEW** (`STRATA-S3-REPORT.md`) — `src/batch.rs`: `Checkpoint`(sub-MMR)+`BatchPolicy`/`should_close`+`EpochAccumulator`+blob canonical(`serialize/parse_batch`)+`TwoTierProof`/`verify_two_tier(&StrataVersion)`. **PR #7** (base main, rebase sau #8). Vòng 1 (07-08): 8 mục `flush_max_age`+watermark+`max_entries`@push+payload-guard. Vòng 2 + **Addendum S3.1** (07-09): DoS `parse_batch` cap + `serialize_batch`→Result + header version byte + `entry_seq` LIÊN TỤC(gap) + arrival-ts + `max_epoch_bytes` van b′. **98 pass / clippy sạch / fmt clean.** Đợi merge (sau #7 → #6). |

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
