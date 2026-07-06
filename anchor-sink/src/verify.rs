//! Verify ngược §8.1c: `anchor on-chain → chứng minh khớp chain local` dưới
//! **mmr_root ĐÃ NEO** với **mmr_size TẠI THỜI ĐIỂM neo** (từ [`AnchoredLog`]).
//!
//! Vì sao KHÔNG dùng `chain.prove_version()` trực tiếp: proof đó sinh dưới MMR
//! HIỆN TẠI (size mới). INV-E3 bảo đảm proof cũ vẫn đúng dưới root MỚI, nhưng chiều
//! ta cần là verify dưới root CŨ → phải TÁI DỰNG MMR ở đúng `mmr_size` lúc neo
//! (leaf = version_hash 0..size, đã có sẵn trong chain local) rồi prove ở đó.

use crate::AnchoredLog;
use lampnet_strata::{Blake3Hasher, StrataAnchor, StrataChain, mmr::Mmr};

/// Lỗi verify ngược — mỗi nhánh chỉ rõ khâu nào gãy (fail-closed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// `on_chain.ref_id != chain.ref_id` — nhầm chain.
    RefIdMismatch,
    /// `on_chain.seq > head local` — local STALE, phải đồng bộ lại (không phải giả mạo).
    LocalBehind { on_chain_seq: u64, local_head: u64 },
    /// AnchoredLog không có dòng cho seq này — daemon quên record lúc publish.
    NotInAnchoredLog { seq: u64 },
    /// Root trong AnchoredLog != root on-chain — log daemon hỏng hoặc anchor giả.
    AnchoredRootMismatch,
    /// Chain local không có version tại seq đã neo.
    SeqMissing { seq: u64 },
    /// `version_hash` local tại seq neo != `head_version_hash` on-chain — lịch sử lệch.
    HeadHashMismatch,
    /// Inclusion-proof KHÔNG verify dưới root đã neo — lịch sử local KHÔNG phải
    /// prefix của lịch sử đã cam kết.
    ProofInvalid,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for VerifyError {}

/// Verify anchor on-chain khớp chain local (thuật toán §8.1c).
///
/// Điều kiện PASS:
/// 1. `ref_id` khớp;
/// 2. `on_chain.seq ≤ head local` (on-chain không đi trước local);
/// 3. [`AnchoredLog`] có `(seq → mmr_root, mmr_size)` và root khớp on-chain;
/// 4. `version_hash(local[seq]) == on_chain.head_version_hash`;
/// 5. inclusion-proof của leaf `seq`, sinh trên MMR TÁI DỰNG ở `mmr_size` cũ,
///    verify dưới `on_chain.mmr_root`.
pub fn verify_anchored(
    chain: &StrataChain,
    on_chain: &StrataAnchor,
    log: &AnchoredLog,
) -> Result<(), VerifyError> {
    // (1) định danh khớp.
    let local = chain.anchor();
    if on_chain.ref_id != local.ref_id {
        return Err(VerifyError::RefIdMismatch);
    }
    // (2) on-chain KHÔNG đi trước local.
    let local_head = chain.head().seq;
    if on_chain.seq > local_head {
        return Err(VerifyError::LocalBehind {
            on_chain_seq: on_chain.seq,
            local_head,
        });
    }
    // (3) size tại thời điểm neo — từ bảng anchored của daemon.
    let (logged_root, mmr_size) = log
        .get(&on_chain.ref_id, on_chain.seq)
        .ok_or(VerifyError::NotInAnchoredLog { seq: on_chain.seq })?;
    if logged_root != on_chain.mmr_root {
        return Err(VerifyError::AnchoredRootMismatch);
    }
    // (4) head đã neo == version local tại seq đó.
    let v = chain
        .version(on_chain.seq)
        .ok_or(VerifyError::SeqMissing { seq: on_chain.seq })?;
    let vh = v.version_hash();
    if vh != on_chain.head_version_hash {
        return Err(VerifyError::HeadHashMismatch);
    }
    // (5) tái dựng MMR ở size CŨ (leaf 0..mmr_size) rồi prove + verify dưới root ĐÃ NEO.
    if mmr_size == 0 || on_chain.seq >= mmr_size {
        return Err(VerifyError::ProofInvalid); // size vô nghĩa với seq đã neo
    }
    let mut old = Mmr::<Blake3Hasher>::new();
    for i in 0..mmr_size {
        let vi = chain
            .version(i)
            .ok_or(VerifyError::SeqMissing { seq: i })?;
        old.append(&vi.version_hash());
    }
    let proof = old.prove(on_chain.seq as usize);
    if !StrataChain::verify_version(on_chain.mmr_root, &vh, on_chain.seq, mmr_size, &proof) {
        return Err(VerifyError::ProofInvalid);
    }
    Ok(())
}
