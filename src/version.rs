//! `StrataVersion` — một nút phiên bản + canonical encoding + chữ ký (CHỐT-1, §1.7).
//!
//! - `version_hash = H_dom("LN/STRATA/ver/v1", canonical_core)` — KHÔNG gồm `sig` (CHỐT-1).
//! - `sig = Ed25519_sign(sk, version_hash)`, verify low-S canonical bằng `verify_strict`.
//! - Canonical: thứ tự trường tất định, u64 big-endian, length-prefix u32_be cho
//!   trường biến độ dài (`content_cid`); trường cố định ghi nguyên byte (§1.7).

use crate::u32_be;
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use lampnet_merkle_anchor::hash::h_dom;

/// BLAKE3 32 byte (`H32`).
pub type Hash32 = [u8; 32];
/// DID đã băm (`Did`) — KHÔNG phải pubkey (CHỐT-5).
pub type Did = [u8; 32];
/// Chữ ký Ed25519.
pub type Sig = [u8; 64];

/// Tag băm version core (CHỐT-2).
pub const TAG_VER: &str = "LN/STRATA/ver/v1";

/// Một phiên bản. Thứ tự trường canonical đúng `_CONTRACT.md`:
/// `{ seq, prev_hash, content_cid, state_root, author_did, policy_hash, ts, sig }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrataVersion {
    /// 0 cho genesis, +1 mỗi lần (INV-E2).
    pub seq: u64,
    /// version_hash(seq-1); 0^32 nếu seq==0 (INV-E1).
    pub prev_hash: Hash32,
    /// CID nội dung off-chain (Mirage) — hash thuần (INV-E5). Biến độ dài.
    pub content_cid: Vec<u8>,
    /// Merkle root field-level (cho FieldProof — INV-E6).
    pub state_root: Hash32,
    /// DID người sửa (đã băm — CHỐT-5).
    pub author_did: Did,
    /// Hash policy quyền sửa (INV-E4).
    pub policy_hash: Hash32,
    /// Unix secs, đơn điệu không-giảm theo seq.
    pub ts: u64,
    /// Ed25519 (canonical low-S) của author trên `version_hash` (CHỐT-1).
    pub sig: Sig,
}

impl StrataVersion {
    /// Dựng version chưa ký (sig = 0^64). Caller gọi [`StrataVersion::sign`] sau.
    #[allow(clippy::too_many_arguments)]
    pub fn unsigned(
        seq: u64,
        prev_hash: Hash32,
        content_cid: Vec<u8>,
        state_root: Hash32,
        author_did: Did,
        policy_hash: Hash32,
        ts: u64,
    ) -> Self {
        Self {
            seq,
            prev_hash,
            content_cid,
            state_root,
            author_did,
            policy_hash,
            ts,
            sig: [0u8; 64],
        }
    }

    /// Encode tất định MỌI trường TRỪ `sig` (§1.7). u64 big-endian; length-prefix
    /// u32_be cho `content_cid`; trường cố định ghi nguyên 32 byte.
    pub fn canonical_core(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(8 + 32 + 4 + self.content_cid.len() + 32 + 32 + 32 + 8);
        b.extend_from_slice(&self.seq.to_be_bytes()); // u64 BE
        b.extend_from_slice(&self.prev_hash); // 32B
        b.extend_from_slice(&u32_be(self.content_cid.len())); // len-prefix u32 BE
        b.extend_from_slice(&self.content_cid);
        b.extend_from_slice(&self.state_root); // 32B
        b.extend_from_slice(&self.author_did); // 32B
        b.extend_from_slice(&self.policy_hash); // 32B
        b.extend_from_slice(&self.ts.to_be_bytes()); // u64 BE
        b // KHÔNG sig (CHỐT-1)
    }

    /// `version_hash = H_dom(TAG_VER, canonical_core)` — KHÔNG gồm sig (CHỐT-1).
    pub fn version_hash(&self) -> Hash32 {
        h_dom(TAG_VER, &self.canonical_core())
    }

    /// Ký version: `sig = Ed25519_sign(sk, version_hash)`. Set field `sig` tại chỗ.
    /// Ký TRỰC TIẾP trên 32 byte `version_hash` như message thuần (PureEdDSA), khớp
    /// CHỐT-1 (sig over version_hash). `author_did` lưu trong version; mapping
    /// `Did → pubkey` thuộc key-registry (CHỐT-5), verify ở [`StrataVersion::verify_sig`].
    pub fn sign(&mut self, sk: &SigningKey) {
        use ed25519_dalek::Signer;
        let vh = self.version_hash();
        let sig: Signature = sk.sign(&vh);
        self.sig = sig.to_bytes();
    }

    /// Verify chữ ký bằng `verify_strict` (chặn malleability — yêu cầu low-S canonical
    /// + reject các điểm thuộc nhóm con nhỏ). `pk` lấy từ key-registry theo `author_did`.
    pub fn verify_sig(&self, pk: &VerifyingKey) -> bool {
        let vh = self.version_hash();
        let sig = Signature::from_bytes(&self.sig);
        pk.verify_strict(&vh, &sig).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn mk_key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    #[test]
    fn canonical_core_excludes_sig() {
        // CHỐT-1: đổi sig KHÔNG đổi version_hash.
        let mut v = StrataVersion::unsigned(0, [0u8; 32], b"cid".to_vec(), [1u8; 32], [2u8; 32], [3u8; 32], 100);
        let vh1 = v.version_hash();
        v.sig = [0xAB; 64];
        let vh2 = v.version_hash();
        assert_eq!(vh1, vh2, "version_hash phải độc lập với sig (CHỐT-1)");
    }

    #[test]
    fn sign_then_verify_strict() {
        let sk = mk_key();
        let pk = sk.verifying_key();
        let mut v = StrataVersion::unsigned(0, [0u8; 32], b"x".to_vec(), [1u8; 32], [2u8; 32], [3u8; 32], 1);
        v.sign(&sk);
        assert!(v.verify_sig(&pk));
    }

    #[test]
    fn verify_strict_rejects_wrong_key() {
        // INV-E4: chữ ký bởi khóa khác → từ chối.
        let sk = mk_key();
        let other = mk_key();
        let mut v = StrataVersion::unsigned(0, [0u8; 32], b"x".to_vec(), [1u8; 32], [2u8; 32], [3u8; 32], 1);
        v.sign(&sk);
        assert!(!v.verify_sig(&other.verifying_key()));
    }

    #[test]
    fn verify_rejects_tampered_field() {
        // Đổi 1 trường core sau khi ký → version_hash đổi → verify fail.
        let sk = mk_key();
        let pk = sk.verifying_key();
        let mut v = StrataVersion::unsigned(0, [0u8; 32], b"x".to_vec(), [1u8; 32], [2u8; 32], [3u8; 32], 1);
        v.sign(&sk);
        v.ts = 999; // tamper
        assert!(!v.verify_sig(&pk));
    }

    #[test]
    fn canonical_length_prefix_avoids_concat_ambiguity() {
        // ("ab", root X) vs ("a", root bắt đầu 'b'…) — length-prefix chống nhập nhằng nối chuỗi.
        let a = StrataVersion::unsigned(0, [0u8; 32], b"ab".to_vec(), [0u8; 32], [0u8; 32], [0u8; 32], 0);
        let b = StrataVersion::unsigned(0, [0u8; 32], b"a".to_vec(), [0u8; 32], [0u8; 32], [0u8; 32], 0);
        assert_ne!(a.version_hash(), b.version_hash());
    }
}
