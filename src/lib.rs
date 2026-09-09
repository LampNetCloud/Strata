//! `lampnet-strata` — Strata (Evolving Content Record = "Hồ sơ Tiến hóa").
//!
//! Lõi vòng đời Strata dựng TRÊN sub-primitive [`lampnet_merkle_anchor`] (MMR + hash
//! domain-separated, dup-leaf guard). Crate THUẦN (no I/O): caller ngoài làm I/O,
//! key-registry, index dẫn xuất.
//!
//! Mọi tên/ký hiệu/invariant khớp `Specs/Strata/_CONTRACT.md` (CHỐT-1..5, bảng
//! domain-tag, khối "Mã hóa state leaf") và `Specs/Strata/Strata-Tech.md` §1/§3.
//!
//! Bốn loại dữ liệu phục vụ bởi MỘT primitive:
//! - #1 Tĩnh = Strata 1 version;
//! - #2 Chuỗi-thêm = MMR chính là log;
//! - #3 Thanh-ghi = đọc head;
//! - #4 Hồ sơ cấu trúc = `state_root` field-level + policy.
//!
//! Map module ↔ INV:
//! - [`refid`]  — INV-E5 (ref_id hash thuần, KHÔNG nhúng loại).
//! - [`state`]  — INV-E6 (field-proof không lộ trường khác), CHỐT-4.
//! - [`version`]— CHỐT-1 (version_hash KHÔNG gồm sig), canonical encoding §1.7.
//! - [`chain`]  — INV-E1/E2/E3/E4/E7 (hash-link, seq đơn điệu, append-only, sig+policy, anchor).
//! - [`field_policy`] — INV-E4 field-level (quyền ghi MỨC TRƯỜNG + bằng chứng quyền dưới policy_hash).
//! - [`audit`]  — INV-E3 (audit-log append-only bất biến).
//! - [`batch`]  — S3: checkpoint sub-MMR (gộp lô epoch → neo 1 lần) + inclusion 2 tầng; kế thừa INV-E3/E7.
//! - [`privacy`]— INV-E9 mức code (padding/decoy chống suy luận loại qua kích thước).
//! - [`composite`] — Composite Strata: rừng cha–con qua field-proof (Feat §6, Math §12, API §5.1).
//! - [`tabular`]— Merkle Sum Tree cho cột: tổng/đếm có bằng chứng (Feat §8, Math §14).
//!
//! Gộp lô S3 (sub-MMR epoch + checkpoint) + AnchorSink S1: xem PR #7 / #6 (Thịnh) — hợp nhất 2026-07-07.

pub mod anchor_sink;
pub mod audit;
pub mod batch;
pub mod chain;
pub mod composite;
pub mod derived_index;
pub mod field_policy;
pub mod privacy;
pub mod refid;
pub mod settlement;
pub mod state;
pub mod tabular;
pub mod version;

pub use anchor_sink::{
    AnchorBackend, AnchorError, AnchorPriority, AnchorReceipt, AnchorRecord, AnchorSink,
    AnchoredTable, AssetClass, CborError, DatumError, MosaicAnchorSink, MosaicBackend, PlutusData,
    ResolvedAnchor, TableError, VerifyError, map_anchor_to_datum, parse_datum_to_anchor,
    verify_resolved,
};
pub use audit::{AuditAction, AuditEntry, AuditLog};
pub use batch::{
    BatchError, BatchPolicy, Checkpoint, CloseReason, ClosedEpoch, EpochAccumulator, TwoTierProof,
    batch_root, entry_bytes, entry_leaf, parse_batch, proof_hash_bytes, serialize_batch,
    verify_two_tier,
};
pub use chain::{Policy, StrataAnchor, StrataChain, StrataError};
pub use composite::{
    ChildRef, ParentChildProof, composite_state_fields, composite_state_root, link_two_tier,
    prove_child,
};
pub use derived_index::{
    ColumnarIndex, CompositeFieldProof, DerivedIndex, InMemoryVersionLog, LogError, Query, Seq,
    VersionLog, brute_force, verify_composite,
};
pub use field_policy::{FieldAuthProof, FieldPolicy};
pub use refid::gen_ref_id;
pub use settlement::{
    ChainQuery, PayloadError, SettlementRecord, SettlementSink, SinkConfig, SubmitOutcome,
    Submitter, decode_records, decode_records_lenient, encode_records, publish_with_retry,
};
pub use state::{FieldProof, build_state_root, prove_field, verify_field_proof};
pub use tabular::{ColumnSumTree, RowSumProof, verify_row_sum, verify_row_sum_range};
pub use version::{CanonicalError, Hash32, StrataVersion, parse_canonical_core};

/// Re-export tiện dụng từ sub-primitive nền.
pub use lampnet_merkle_anchor::{Blake3Hasher, DomainHasher, hash, mmr};

/// Encode `u32` big-endian — length/count-prefix canonical (§1.7 quy tắc 3).
///
/// **Trần cứng `< 2³²`** (issue #18, đồng bộ Spectra §1.9): mọi trường/danh sách length-
/// prefix phải `< 2³²` (byte hoặc phần tử). `n ≥ 2³²` → `n as u32` truncate LẶNG LẼ →
/// prefix cụt → **mất song ánh** canonical → hai input khác cho cùng byte → `H_dom` trùng,
/// vỡ tất định băm (lan xuống consumer không phát hiện được). Vì vậy **fail-loud**:
/// `assert!` (chạy CẢ release, không phải `debug_assert!`) — thà panic tại chỗ còn hơn băm
/// sai. Đây là van chung cho mọi caller không có guard graceful (`version::content_cid`,
/// `field_policy::field_key`, `audit`); `batch::entry_bytes` guard trước bằng
/// `PayloadTooLarge` nên không chạm assert này.
#[inline]
pub(crate) fn u32_be(n: usize) -> [u8; 4] {
    assert!(
        n <= u32::MAX as usize,
        "canonical §1.7: trường length/count-prefix = {n} ≥ 2³² — mất song ánh (issue #18)"
    );
    (n as u32).to_be_bytes()
}

#[cfg(test)]
mod u32_be_tests {
    use super::u32_be;

    #[test]
    fn boundary_u32_max_ok() {
        // Đúng trần: tới u32::MAX vẫn encode được, không panic.
        assert_eq!(u32_be(0), [0, 0, 0, 0]);
        assert_eq!(u32_be(258), 258u32.to_be_bytes());
        assert_eq!(u32_be(u32::MAX as usize), [0xff, 0xff, 0xff, 0xff]);
    }

    #[test]
    #[should_panic(expected = "mất song ánh")]
    #[cfg(target_pointer_width = "64")] // usize 32-bit không biểu diễn nổi 2³²
    fn over_2pow32_fails_loud() {
        // Vượt trần: fail-loud thay vì truncate lặng lẽ (chỉ truyền n, KHÔNG cấp phát 4GiB).
        let _ = u32_be((u32::MAX as usize) + 1);
    }
}
