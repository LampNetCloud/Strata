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
//!
//! Gộp lô S3 (sub-MMR epoch + checkpoint) + AnchorSink S1: xem PR #7 / #6 (Thịnh) — hợp nhất 2026-07-07.

pub mod audit;
pub mod batch;
pub mod chain;
pub mod derived_index;
pub mod field_policy;
pub mod privacy;
pub mod refid;
pub mod state;
pub mod version;

pub use audit::{AuditAction, AuditEntry, AuditLog};
pub use batch::{
    BatchError, BatchPolicy, Checkpoint, CloseReason, ClosedEpoch, EpochAccumulator, TwoTierProof,
    batch_root, entry_bytes, entry_leaf, parse_batch, proof_hash_bytes, serialize_batch,
    verify_two_tier,
};
pub use chain::{Policy, StrataAnchor, StrataChain, StrataError};
pub use derived_index::{
    ColumnarIndex, CompositeFieldProof, DerivedIndex, InMemoryVersionLog, LogError, Query, Seq,
    VersionLog, brute_force, verify_composite,
};
pub use field_policy::{FieldAuthProof, FieldPolicy};
pub use refid::gen_ref_id;
pub use state::{FieldProof, build_state_root, prove_field, verify_field_proof};
pub use version::{Hash32, StrataVersion};

/// Re-export tiện dụng từ sub-primitive nền.
pub use lampnet_merkle_anchor::{Blake3Hasher, DomainHasher, hash, mmr};

/// Encode `u32` big-endian (length-prefix cho trường biến độ dài — §1.7 quy tắc 3).
#[inline]
pub(crate) fn u32_be(n: usize) -> [u8; 4] {
    (n as u32).to_be_bytes()
}
