//! INV-E4 **field-level authorization** — nâng cấp từ V1 (mức-chain) lên mức-trường.
//!
//! V1 (`chain::Policy`) cam kết TẬP author được phép; mọi author sửa MỌI trường.
//! `FieldPolicy` ở đây cam kết TẬP entry `(author_did, field_key)` — mỗi cặp là một
//! quyền ghi trên đúng một trường. `policy_hash` = Merkle-root cây các entry (tag
//! RIÊNG, KHÔNG đụng state/MMR). Khi sửa một trường, author kèm **bằng chứng quyền**
//! ([`FieldAuthProof`]): Merkle-proof rằng entry `(author_did, field_key)` nằm dưới
//! `policy_hash`. Verifier kiểm (Strata-Math §7.2):
//!   (a) `author_did` có entry cho ĐÚNG `field_key` đang sửa;
//!   (b) `version.policy_hash` khớp commit (chống bộ quyền giả — Mệnh đề 6).
//!
//! Vì `policy_hash` là commit hash, không ai thêm/bớt quyền lén mà không đổi
//! `policy_hash` → đổi `core` → đổi `version_hash` → sig fail.
//!
//! Cây dùng CÙNG luật với state (INV-E8): domain-sep, sort tất định, dup-leaf guard
//! (lá lẻ carry, KHÔNG copy). Tag riêng theo bảng domain-tag (CHỐT-2 mở rộng).

use crate::u32_be;
use ed25519_dalek::VerifyingKey;
use lampnet_merkle_anchor::hash::{Hash32, h_dom};
use std::collections::BTreeMap;

/// Tag entry quyền field-level (leaf cây policy).
pub const TAG_POLICY_FIELD_LEAF: &str = "LN/STRATA/policy/field/leaf/v1";
/// Tag internal node cây policy field-level.
pub const TAG_POLICY_FIELD_NODE: &str = "LN/STRATA/policy/field/node/v1";

/// `entry_leaf = H_dom(TAG_POLICY_FIELD_LEAF, author_did ‖ u32_be(len(field_key)) ‖ field_key)`.
///
/// `author_did` cố định 32B đứng trước; `field_key` length-prefix để chống nhập nhằng
/// nối chuỗi (INV-E8). Một entry = "author_did được ghi field_key".
pub fn entry_leaf(author_did: &[u8; 32], field_key: &[u8]) -> Hash32 {
    let mut buf = Vec::with_capacity(32 + 4 + field_key.len());
    buf.extend_from_slice(author_did);
    buf.extend_from_slice(&u32_be(field_key.len()));
    buf.extend_from_slice(field_key);
    h_dom(TAG_POLICY_FIELD_LEAF, &buf)
}

/// `node = H_dom(TAG_POLICY_FIELD_NODE, left ‖ right)`.
fn node_hash(left: &Hash32, right: &Hash32) -> Hash32 {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(left);
    buf.extend_from_slice(right);
    h_dom(TAG_POLICY_FIELD_NODE, &buf)
}

/// Một tầng cây: gộp đôi, lá lẻ carry nguyên lên (dup-leaf guard — CVE-2012-2459).
fn fold_level(level: &[Hash32]) -> Vec<Hash32> {
    let mut next = Vec::with_capacity(level.len().div_ceil(2));
    let mut i = 0;
    while i + 1 < level.len() {
        next.push(node_hash(&level[i], &level[i + 1]));
        i += 2;
    }
    if i < level.len() {
        next.push(level[i]); // carry lá lẻ — KHÔNG nhân đôi
    }
    next
}

/// Policy field-level: cam kết tập entry `(author_did, field_key)` + key-registry
/// `Did → pubkey` (CHỐT-5, như `chain::Policy`). Ai được ghi trường nào.
#[derive(Debug, Clone, Default)]
pub struct FieldPolicy {
    /// entry được phép: `(author_did, field_key)` (dedup + sort tất định qua BTreeSet-lối).
    /// Dùng BTreeMap<(did, field), ()> để sort ổn định và dedup.
    entries: BTreeMap<([u8; 32], Vec<u8>), ()>,
    /// key-registry `author_did → pubkey` (verify sig — CHỐT-5).
    keys: BTreeMap<[u8; 32], VerifyingKey>,
}

impl FieldPolicy {
    /// Policy rỗng (không quyền nào).
    pub fn new() -> Self {
        Self::default()
    }

    /// Đăng ký khoá của một author (key-registry). Cần cho verify sig.
    pub fn register_key(&mut self, author_did: [u8; 32], pk: VerifyingKey) {
        self.keys.insert(author_did, pk);
    }

    /// Cấp quyền ghi: `author_did` được sửa `field_key`. Tự đăng ký entry (idempotent).
    pub fn grant(&mut self, author_did: [u8; 32], field_key: &[u8]) {
        self.entries.insert((author_did, field_key.to_vec()), ());
    }

    /// Cấp quyền + đăng ký khoá cùng lúc (tiện dụng).
    pub fn grant_with_key(&mut self, author_did: [u8; 32], pk: VerifyingKey, field_key: &[u8]) {
        self.register_key(author_did, pk);
        self.grant(author_did, field_key);
    }

    /// `author_did` có quyền ghi `field_key` không (kiểm cục bộ — không qua proof).
    pub fn can_write(&self, author_did: &[u8; 32], field_key: &[u8]) -> bool {
        self.entries
            .contains_key(&(*author_did, field_key.to_vec()))
    }

    /// Phân giải Did → pubkey (key-registry — CHỐT-5).
    pub fn resolve(&self, author_did: &[u8; 32]) -> Option<&VerifyingKey> {
        self.keys.get(author_did)
    }

    /// Danh sách leaf đã sort tất định (theo (author_did, field_key)).
    fn sorted_leaves(&self) -> Vec<Hash32> {
        self.entries
            .keys()
            .map(|(did, fk)| entry_leaf(did, fk))
            .collect()
    }

    /// `policy_hash` = Merkle-root cây các entry. Rỗng → 0^32. Tất định (entries đã
    /// sort qua BTreeMap). Đây là giá trị mỗi version commit vào `policy_hash`.
    pub fn policy_hash(&self) -> Hash32 {
        let leaves = self.sorted_leaves();
        if leaves.is_empty() {
            return [0u8; 32];
        }
        let mut level = leaves;
        while level.len() > 1 {
            level = fold_level(&level);
        }
        level[0]
    }

    /// Sinh bằng chứng quyền cho entry `(author_did, field_key)`. `None` nếu entry
    /// không có trong policy.
    pub fn prove(&self, author_did: &[u8; 32], field_key: &[u8]) -> Option<FieldAuthProof> {
        let leaves = self.sorted_leaves();
        let target = entry_leaf(author_did, field_key);
        let mut pos = leaves.iter().position(|h| *h == target)?;

        let mut level = leaves;
        let mut siblings = Vec::new();
        while level.len() > 1 {
            let is_left = pos % 2 == 0;
            if is_left {
                if pos + 1 < level.len() {
                    siblings.push((level[pos + 1], true));
                }
                // pos lẻ-cuối được carry → không sibling ở tầng này
            } else {
                siblings.push((level[pos - 1], false));
            }
            level = fold_level(&level);
            pos /= 2;
        }

        Some(FieldAuthProof {
            author_did: *author_did,
            field_key: field_key.to_vec(),
            siblings,
            policy_hash: self.policy_hash(),
        })
    }
}

/// Bằng chứng "author_did được ghi field_key" nằm dưới `policy_hash` (Strata-Math §7.2).
/// Tương tự [`crate::state::FieldProof`] nhưng cho cây POLICY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldAuthProof {
    /// author được cấp quyền.
    pub author_did: [u8; 32],
    /// trường được phép ghi.
    pub field_key: Vec<u8>,
    /// đường anh em từ entry-leaf lên root: `(sibling_hash, sibling_is_right)`.
    pub siblings: Vec<(Hash32, bool)>,
    /// `policy_hash` mục tiêu (gắn với version qua `version.policy_hash`).
    pub policy_hash: Hash32,
}

impl FieldAuthProof {
    /// Verify bằng chứng: tính lại root từ entry-leaf `(author_did, field_key)` + đường
    /// anh em, so `policy_hash`. Đúng ⇒ author THẬT SỰ có quyền ghi field này dưới
    /// commit `policy_hash`.
    pub fn verify(&self) -> bool {
        let mut acc = entry_leaf(&self.author_did, &self.field_key);
        for (sib, sib_is_right) in &self.siblings {
            acc = if *sib_is_right {
                node_hash(&acc, sib)
            } else {
                node_hash(sib, &acc)
            };
        }
        acc == self.policy_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn did(tag: u8) -> [u8; 32] {
        [tag; 32]
    }
    fn key() -> VerifyingKey {
        SigningKey::generate(&mut OsRng).verifying_key()
    }

    fn sample_policy() -> FieldPolicy {
        let mut p = FieldPolicy::new();
        // bác sĩ ghi diagnosis; y tá ghi phone; hành chính ghi address.
        p.grant_with_key(did(1), key(), b"diagnosis");
        p.grant_with_key(did(2), key(), b"phone");
        p.grant_with_key(did(3), key(), b"address");
        p.grant(did(1), b"note"); // author 1 có 2 quyền
        p
    }

    #[test]
    fn policy_hash_deterministic_regardless_insert_order() {
        let mut a = FieldPolicy::new();
        a.grant(did(1), b"diagnosis");
        a.grant(did(2), b"phone");
        a.grant(did(3), b"address");

        let mut b = FieldPolicy::new();
        b.grant(did(3), b"address");
        b.grant(did(1), b"diagnosis");
        b.grant(did(2), b"phone");

        assert_eq!(a.policy_hash(), b.policy_hash());
    }

    #[test]
    fn empty_policy_hash_is_zero() {
        assert_eq!(FieldPolicy::new().policy_hash(), [0u8; 32]);
    }

    #[test]
    fn proof_round_trip_all_entries() {
        let p = sample_policy();
        for (did_fk, _) in p.entries.iter() {
            let (d, fk) = did_fk;
            let proof = p.prove(d, fk).expect("entry tồn tại");
            assert!(proof.verify(), "verify quyền ({:?},{:?})", d[0], fk);
            assert_eq!(proof.policy_hash, p.policy_hash());
        }
    }

    #[test]
    fn can_write_matches_grants() {
        let p = sample_policy();
        assert!(p.can_write(&did(1), b"diagnosis"));
        assert!(p.can_write(&did(1), b"note"));
        assert!(!p.can_write(&did(1), b"phone")); // author 1 KHÔNG có quyền phone
        assert!(!p.can_write(&did(9), b"diagnosis")); // outsider
    }

    #[test]
    fn redteam_unauthorized_field_no_proof() {
        // author 2 (chỉ có quyền phone) cố xin proof cho diagnosis → None.
        let p = sample_policy();
        assert!(p.prove(&did(2), b"diagnosis").is_none());
    }

    #[test]
    fn redteam_forged_proof_wrong_field_fails() {
        // Lấy proof hợp lệ của (author1, diagnosis), đổi field_key sang trường author1
        // KHÔNG có quyền → verify fail (root không khớp).
        let p = sample_policy();
        let mut proof = p.prove(&did(1), b"diagnosis").unwrap();
        proof.field_key = b"phone".to_vec();
        assert!(!proof.verify());
    }

    #[test]
    fn redteam_forged_proof_wrong_author_fails() {
        // Đổi author_did trong proof (giả mạo người khác được quyền) → verify fail.
        let p = sample_policy();
        let mut proof = p.prove(&did(1), b"diagnosis").unwrap();
        proof.author_did = did(9);
        assert!(!proof.verify());
    }

    #[test]
    fn redteam_proof_against_wrong_policy_hash_fails() {
        // Proof sinh dưới policy A không verify dưới policy_hash của policy B khác tập quyền.
        let a = sample_policy();
        let mut b = FieldPolicy::new();
        b.grant(did(1), b"diagnosis"); // tập quyền HẸP hơn → policy_hash khác
        let proof = a.prove(&did(1), b"diagnosis").unwrap();
        assert_ne!(a.policy_hash(), b.policy_hash());
        // proof.policy_hash vẫn là của A; nếu ai đó ép nó khớp B thì root tính lại != B.
        let mut forged = proof.clone();
        forged.policy_hash = b.policy_hash();
        assert!(!forged.verify());
    }

    #[test]
    fn single_entry_tree() {
        let mut p = FieldPolicy::new();
        p.grant(did(1), b"only");
        let proof = p.prove(&did(1), b"only").unwrap();
        assert!(proof.verify());
        assert!(proof.siblings.is_empty());
    }
}
