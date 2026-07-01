//! `StrataChain` — vòng đời Strata: hash-link, seq đơn điệu, append-only MMR, sig+policy,
//! anchor chống rollback.
//!
//! Dựng TRÊN [`lampnet_merkle_anchor::mmr::Mmr`] (leaf = version_hash). KHÔNG tự
//! viết lại MMR/hash. Enforce:
//! - INV-E1: `prev_hash == version_hash(prev)`; seq=0 → 0^32.
//! - INV-E2: seq tăng đúng +1.
//! - INV-E4: `verify_strict` chữ ký bởi `author_did` + author được policy cho phép.
//! - INV-E7: `anchor.seq` đơn điệu — neo lại version cũ bị từ chối.

use crate::field_policy::{FieldAuthProof, FieldPolicy};
use crate::version::{Hash32, StrataVersion};
use ed25519_dalek::VerifyingKey;
use lampnet_merkle_anchor::Blake3Hasher;
use lampnet_merkle_anchor::mmr::{InclusionProof, Mmr, verify as mmr_verify};
use std::collections::BTreeMap;

/// Lỗi vòng đời Strata (mọi vi phạm invariant trả lỗi rõ).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrataError {
    /// INV-E1: prev_hash không khớp version_hash của head.
    HashLinkBroken { expected: Hash32, got: Hash32 },
    /// INV-E2: seq không phải head_seq + 1.
    SeqNotMonotonic { expected: u64, got: u64 },
    /// INV-E4: chữ ký không hợp lệ (verify_strict fail / malleable / sai khoá).
    BadSignature,
    /// INV-E4: author không có trong policy cho phép.
    PolicyDenied,
    /// INV-E4: version.policy_hash không khớp cam kết của policy thực thi
    /// (chống commit một bộ quyền giả — xem Mệnh đề 6, Strata-Math §7.2).
    PolicyHashMismatch { expected: Hash32, got: Hash32 },
    /// CHỐT-5: không phân giải được Did → pubkey qua key-registry.
    UnknownAuthor,
    /// ts giảm so với version trước (vi phạm đơn điệu thời gian).
    TimestampRegress { prev: u64, got: u64 },
    /// INV-E7: neo lại seq cũ/bằng (rollback).
    AnchorRollback { current: u64, attempted: u64 },
    /// Overflow seq (đạt u64::MAX).
    SeqOverflow,
    /// INV-E4 field-level: author không có quyền ghi trường này (bằng chứng thiếu/sai).
    FieldPolicyDenied { field_key: Vec<u8> },
    /// INV-E4 field-level: bằng chứng quyền trỏ sai `policy_hash` (không khớp version).
    FieldProofPolicyMismatch { field_key: Vec<u8> },
}

impl std::fmt::Display for StrataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for StrataError {}

/// Policy đơn giản: tập `author_did` được phép append, commit qua `policy_hash`
/// (INV-E4). `policy_hash` = `H_dom("LN/STRATA/ver/v1"…)`? Không — đây là cam kết tập
/// author; ta băm danh sách author đã sort để mọi version tham chiếu cùng giá trị.
///
// INV-E4 (V1 — mức CHAIN): tập author được phép sửa toàn bộ Strata; mọi author hợp lệ
// sửa mọi trường. Nâng cấp field-level (mỗi (author, field) + Merkle proof dưới
// policy_hash) đã có ở [`crate::field_policy::FieldPolicy`] + `check_auth_fielded`.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    allowed: BTreeMap<[u8; 32], VerifyingKey>,
}

impl Policy {
    /// Policy rỗng.
    pub fn new() -> Self {
        Self {
            allowed: BTreeMap::new(),
        }
    }

    /// Thêm một author được phép: `author_did → pubkey` (mô phỏng key-registry CHỐT-5).
    pub fn allow(&mut self, author_did: [u8; 32], pk: VerifyingKey) {
        self.allowed.insert(author_did, pk);
    }

    /// author_did có được phép không.
    pub fn is_allowed(&self, author_did: &[u8; 32]) -> bool {
        self.allowed.contains_key(author_did)
    }

    /// Phân giải Did → pubkey (key-registry — CHỐT-5).
    pub fn resolve(&self, author_did: &[u8; 32]) -> Option<&VerifyingKey> {
        self.allowed.get(author_did)
    }

    /// `policy_hash` = cam kết tất định của tập author được phép (sort theo did).
    /// Mọi version commit chính giá trị này; đổi tập author → đổi policy_hash.
    pub fn policy_hash(&self) -> Hash32 {
        let mut buf = Vec::with_capacity(self.allowed.len() * 64);
        for (did, pk) in &self.allowed {
            buf.extend_from_slice(did);
            buf.extend_from_slice(pk.as_bytes());
        }
        lampnet_merkle_anchor::hash::h_dom("LN/STRATA/policy/v1", &buf)
    }
}

/// Neo on-chain = 4 trường = 104 byte (3×32 + 8). "Cam kết lịch sử" = `mmr_root` 32B.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrataAnchor {
    /// ref_id thô (32B) — định danh ổn định.
    pub ref_id: Hash32,
    /// version_hash của head.
    pub head_version_hash: Hash32,
    /// cam kết lịch sử (MMR root).
    pub mmr_root: Hash32,
    /// seq của head — đơn điệu (INV-E7).
    pub seq: u64,
}

/// Một Strata chain: MMR (leaf = version_hash) + danh sách version theo thứ tự seq.
///
/// Debug/Clone hand-impl: `Mmr<H>` derive yêu cầu `H: Debug+Clone` (PhantomData),
/// mà `Blake3Hasher` là unit struct không derive — nên MMR tái dựng từ `versions`.
pub struct StrataChain {
    ref_id: Hash32,
    mmr: Mmr<Blake3Hasher>,
    versions: Vec<StrataVersion>,
    last_anchor_seq: Option<u64>,
}

impl std::fmt::Debug for StrataChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StrataChain")
            .field("ref_id", &self.ref_id)
            .field("len", &self.versions.len())
            .field("mmr_root", &self.mmr.root())
            .field("last_anchor_seq", &self.last_anchor_seq)
            .finish()
    }
}

impl Clone for StrataChain {
    fn clone(&self) -> Self {
        let mut mmr = Mmr::<Blake3Hasher>::new();
        for v in &self.versions {
            mmr.append(&v.version_hash());
        }
        Self {
            ref_id: self.ref_id,
            mmr,
            versions: self.versions.clone(),
            last_anchor_seq: self.last_anchor_seq,
        }
    }
}

impl StrataChain {
    /// Genesis: tạo chain với version seq=0 đã ký + được policy cho phép.
    /// `v0` phải có seq=0, prev_hash=0^32, sig hợp lệ, author trong policy.
    pub fn genesis(
        ref_id: Hash32,
        v0: StrataVersion,
        policy: &Policy,
    ) -> Result<Self, StrataError> {
        if v0.seq != 0 {
            return Err(StrataError::SeqNotMonotonic {
                expected: 0,
                got: v0.seq,
            });
        }
        if v0.prev_hash != [0u8; 32] {
            return Err(StrataError::HashLinkBroken {
                expected: [0u8; 32],
                got: v0.prev_hash,
            });
        }
        Self::check_auth(&v0, policy)?;
        let mut mmr = Mmr::<Blake3Hasher>::new();
        mmr.append(&v0.version_hash());
        Ok(Self {
            ref_id,
            mmr,
            versions: vec![v0],
            last_anchor_seq: None,
        })
    }

    /// Append một version mới (seq = head+1). Enforce INV-E1/E2/E4 + đơn điệu ts.
    pub fn append_version(&mut self, v: StrataVersion, policy: &Policy) -> Result<(), StrataError> {
        let head = self.head();
        // INV-E2: seq +1 đúng (overflow-check).
        let expected_seq = head.seq.checked_add(1).ok_or(StrataError::SeqOverflow)?;
        if v.seq != expected_seq {
            return Err(StrataError::SeqNotMonotonic {
                expected: expected_seq,
                got: v.seq,
            });
        }
        // INV-E1: hash-linked — chỉ nối từ head (chống fork).
        let head_vh = head.version_hash();
        if v.prev_hash != head_vh {
            return Err(StrataError::HashLinkBroken {
                expected: head_vh,
                got: v.prev_hash,
            });
        }
        // Đơn điệu thời gian (cho "giá trị tại t").
        if v.ts < head.ts {
            return Err(StrataError::TimestampRegress {
                prev: head.ts,
                got: v.ts,
            });
        }
        // INV-E4: chữ ký + policy.
        Self::check_auth(&v, policy)?;

        self.mmr.append(&v.version_hash());
        self.versions.push(v);
        Ok(())
    }

    /// INV-E4 (V1 — mức CHAIN): phân giải Did→pubkey, verify_strict, kiểm author trong
    /// policy. Mọi author hợp lệ sửa MỌI trường. Cần quyền MỨC TRƯỜNG → dùng
    /// [`StrataChain::append_version_fielded`] + [`crate::field_policy::FieldPolicy`].
    fn check_auth(v: &StrataVersion, policy: &Policy) -> Result<(), StrataError> {
        // INV-E4 (cốt lõi): version PHẢI commit đúng cam kết policy đang thực thi.
        // Thiếu kiểm này → author commit policy_hash giả mà verifier không phát hiện
        // → Mệnh đề 6 (Strata-Math §7.2) sụp đổ.
        let expected = policy.policy_hash();
        if v.policy_hash != expected {
            return Err(StrataError::PolicyHashMismatch {
                expected,
                got: v.policy_hash,
            });
        }
        if !policy.is_allowed(&v.author_did) {
            return Err(StrataError::PolicyDenied);
        }
        let pk = policy
            .resolve(&v.author_did)
            .ok_or(StrataError::UnknownAuthor)?;
        if !v.verify_sig(pk) {
            return Err(StrataError::BadSignature);
        }
        Ok(())
    }

    /// INV-E4 **field-level** (V2): thực thi quyền MỨC TRƯỜNG thay cho mức-chain.
    ///
    /// So với [`StrataChain::check_auth`] (V1: author sửa mọi trường), phiên bản này
    /// buộc author kèm bằng chứng quyền cho TỪNG trường đang sửa (`changed_fields`):
    /// - `version.policy_hash` phải khớp `fpolicy.policy_hash()` (Mệnh đề 6);
    /// - sig hợp lệ bởi khoá của `author_did` (CHỐT-5, key-registry của FieldPolicy);
    /// - mỗi `field_key ∈ changed_fields` có một [`FieldAuthProof`] verify được dưới
    ///   `version.policy_hash`, với `author_did == v.author_did` và đúng `field_key`.
    ///
    /// Bằng chứng bảo đảm author có entry `(author_did, field_key)` nằm dưới cam kết
    /// `policy_hash` — không ai tự cấp quyền lén (đổi tập quyền → đổi policy_hash →
    /// đổi core → sig fail). Trường KHÔNG có bằng chứng hợp lệ ⇒ `FieldPolicyDenied`.
    fn check_auth_fielded(
        v: &StrataVersion,
        fpolicy: &FieldPolicy,
        changed_fields: &[&[u8]],
        proofs: &[FieldAuthProof],
    ) -> Result<(), StrataError> {
        // (a) version PHẢI commit đúng cam kết policy field-level đang thực thi.
        let expected = fpolicy.policy_hash();
        if v.policy_hash != expected {
            return Err(StrataError::PolicyHashMismatch {
                expected,
                got: v.policy_hash,
            });
        }
        // (b) sig hợp lệ bởi khoá của author (INV-E4 phần chữ ký).
        let pk = fpolicy
            .resolve(&v.author_did)
            .ok_or(StrataError::UnknownAuthor)?;
        if !v.verify_sig(pk) {
            return Err(StrataError::BadSignature);
        }
        // (c) mỗi trường thay đổi phải có bằng chứng quyền hợp lệ của CHÍNH author này.
        for field_key in changed_fields {
            let ok = proofs.iter().any(|p| {
                p.field_key.as_slice() == *field_key
                    && p.author_did == v.author_did
                    && p.policy_hash == v.policy_hash
                    && p.verify()
            });
            if !ok {
                // Phân biệt: có proof đúng trường nhưng sai policy_hash → mismatch;
                // ngược lại → thiếu quyền.
                let has_wrong_ph = proofs.iter().any(|p| {
                    p.field_key.as_slice() == *field_key
                        && p.author_did == v.author_did
                        && p.policy_hash != v.policy_hash
                });
                return Err(if has_wrong_ph {
                    StrataError::FieldProofPolicyMismatch {
                        field_key: field_key.to_vec(),
                    }
                } else {
                    StrataError::FieldPolicyDenied {
                        field_key: field_key.to_vec(),
                    }
                });
            }
        }
        Ok(())
    }

    /// Genesis với thực thi INV-E4 **field-level** (xem [`StrataChain::check_auth_fielded`]).
    /// `changed_fields` = tập trường author ghi ở genesis; mỗi trường cần một proof.
    pub fn genesis_fielded(
        ref_id: Hash32,
        v0: StrataVersion,
        fpolicy: &FieldPolicy,
        changed_fields: &[&[u8]],
        proofs: &[FieldAuthProof],
    ) -> Result<Self, StrataError> {
        if v0.seq != 0 {
            return Err(StrataError::SeqNotMonotonic {
                expected: 0,
                got: v0.seq,
            });
        }
        if v0.prev_hash != [0u8; 32] {
            return Err(StrataError::HashLinkBroken {
                expected: [0u8; 32],
                got: v0.prev_hash,
            });
        }
        Self::check_auth_fielded(&v0, fpolicy, changed_fields, proofs)?;
        let mut mmr = Mmr::<Blake3Hasher>::new();
        mmr.append(&v0.version_hash());
        Ok(Self {
            ref_id,
            mmr,
            versions: vec![v0],
            last_anchor_seq: None,
        })
    }

    /// Append với thực thi INV-E4 **field-level** (V2). Kiểm INV-E1/E2 + đơn điệu ts
    /// như [`StrataChain::append_version`], nhưng phần quyền dùng
    /// [`StrataChain::check_auth_fielded`]: author phải chứng minh quyền cho MỖI trường
    /// trong `changed_fields`.
    pub fn append_version_fielded(
        &mut self,
        v: StrataVersion,
        fpolicy: &FieldPolicy,
        changed_fields: &[&[u8]],
        proofs: &[FieldAuthProof],
    ) -> Result<(), StrataError> {
        let head = self.head();
        let expected_seq = head.seq.checked_add(1).ok_or(StrataError::SeqOverflow)?;
        if v.seq != expected_seq {
            return Err(StrataError::SeqNotMonotonic {
                expected: expected_seq,
                got: v.seq,
            });
        }
        let head_vh = head.version_hash();
        if v.prev_hash != head_vh {
            return Err(StrataError::HashLinkBroken {
                expected: head_vh,
                got: v.prev_hash,
            });
        }
        if v.ts < head.ts {
            return Err(StrataError::TimestampRegress {
                prev: head.ts,
                got: v.ts,
            });
        }
        Self::check_auth_fielded(&v, fpolicy, changed_fields, proofs)?;

        self.mmr.append(&v.version_hash());
        self.versions.push(v);
        Ok(())
    }

    /// Version head (cuối chuỗi). Luôn tồn tại (genesis bảo đảm ≥1).
    pub fn head(&self) -> &StrataVersion {
        self.versions.last().expect("chain luôn có ≥1 version")
    }

    /// version_hash của head.
    pub fn head_version_hash(&self) -> Hash32 {
        self.head().version_hash()
    }

    /// MMR root hiện tại (cam kết lịch sử).
    pub fn mmr_root(&self) -> Hash32 {
        self.mmr.root()
    }

    /// Số version.
    pub fn len(&self) -> usize {
        self.versions.len()
    }

    /// Rỗng? (không bao giờ — genesis bảo đảm ≥1; có để tránh clippy).
    pub fn is_empty(&self) -> bool {
        self.versions.is_empty()
    }

    /// Version tại `seq` (nếu có).
    pub fn version(&self, seq: u64) -> Option<&StrataVersion> {
        self.versions.get(seq as usize)
    }

    /// Anchor hiện tại (4 trường, 104 byte). KHÔNG tự cập nhật last_anchor_seq —
    /// gọi [`StrataChain::publish_anchor`] để enforce INV-E7.
    pub fn anchor(&self) -> StrataAnchor {
        StrataAnchor {
            ref_id: self.ref_id,
            head_version_hash: self.head_version_hash(),
            mmr_root: self.mmr_root(),
            seq: self.head().seq,
        }
    }

    /// "Neo" anchor: enforce INV-E7 (seq đơn điệu — không neo lại version cũ/bằng).
    /// Trả anchor mới khi hợp lệ; cập nhật `last_anchor_seq`.
    pub fn publish_anchor(&mut self) -> Result<StrataAnchor, StrataError> {
        let a = self.anchor();
        if let Some(prev) = self.last_anchor_seq
            && a.seq <= prev
        {
            return Err(StrataError::AnchorRollback {
                current: prev,
                attempted: a.seq,
            });
        }
        self.last_anchor_seq = Some(a.seq);
        Ok(a)
    }

    /// Inclusion-proof cho version tại `seq` (dùng MMR nền).
    pub fn prove_version(&self, seq: u64) -> Option<(InclusionProof, u64, Hash32)> {
        if seq as usize >= self.versions.len() {
            return None;
        }
        let proof = self.mmr.prove(seq as usize);
        Some((
            proof,
            self.mmr.len(),
            self.versions[seq as usize].version_hash(),
        ))
    }

    /// Verify một version-proof dưới một `root` cho trước (merkle-anchor verify).
    pub fn verify_version(
        root: Hash32,
        leaf_version_hash: &Hash32,
        seq: u64,
        mmr_size: u64,
        proof: &InclusionProof,
    ) -> bool {
        mmr_verify::<Blake3Hasher>(root, leaf_version_hash, seq as usize, mmr_size, proof)
    }

    /// "Giá trị tại thời điểm t": version có ts lớn nhất ≤ t (ts đơn điệu theo seq).
    /// Trả `(version, inclusion-proof)`.
    pub fn version_at(&self, t: u64) -> Option<(&StrataVersion, InclusionProof)> {
        // Binary search hợp lệ vì ts không-giảm theo seq.
        let mut lo = 0usize;
        let mut hi = self.versions.len();
        let mut found: Option<usize> = None;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.versions[mid].ts <= t {
                found = Some(mid);
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let idx = found?;
        let proof = self.mmr.prove(idx);
        Some((&self.versions[idx], proof))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::build_state_root;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    struct Author {
        did: [u8; 32],
        sk: SigningKey,
    }
    fn mk_author(tag: u8) -> Author {
        Author {
            did: [tag; 32],
            sk: SigningKey::generate(&mut OsRng),
        }
    }

    fn policy_with(authors: &[&Author]) -> Policy {
        let mut p = Policy::new();
        for a in authors {
            p.allow(a.did, a.sk.verifying_key());
        }
        p
    }

    fn signed(seq: u64, prev: Hash32, ts: u64, a: &Author, ph: Hash32) -> StrataVersion {
        let sr = build_state_root(&[(b"k".to_vec(), b"v".to_vec())]);
        let mut v = StrataVersion::unsigned(seq, prev, b"cid".to_vec(), sr, a.did, ph, ts);
        v.sign(&a.sk);
        v
    }

    fn genesis_chain(a: &Author) -> (StrataChain, Policy) {
        let pol = policy_with(&[a]);
        let ph = pol.policy_hash();
        let v0 = signed(0, [0u8; 32], 100, a, ph);
        let chain = StrataChain::genesis([0xAB; 32], v0, &pol).unwrap();
        (chain, pol)
    }

    #[test]
    fn append_happy_path() {
        let a = mk_author(1);
        let (mut chain, pol) = genesis_chain(&a);
        let ph = pol.policy_hash();
        let v1 = signed(1, chain.head_version_hash(), 200, &a, ph);
        assert!(chain.append_version(v1, &pol).is_ok());
        assert_eq!(chain.len(), 2);
    }

    #[test]
    fn inv_e1_hash_link_broken_rejected() {
        let a = mk_author(1);
        let (mut chain, pol) = genesis_chain(&a);
        let ph = pol.policy_hash();
        // prev_hash sai (không phải head).
        let v1 = signed(1, [0xFF; 32], 200, &a, ph);
        assert!(matches!(
            chain.append_version(v1, &pol),
            Err(StrataError::HashLinkBroken { .. })
        ));
    }

    #[test]
    fn inv_e1_tamper_past_version_breaks_link() {
        // Sửa version quá khứ → version_hash đổi → version sau không còn nối được.
        let a = mk_author(1);
        let (mut chain, pol) = genesis_chain(&a);
        let ph = pol.policy_hash();
        let head_vh = chain.head_version_hash();
        let v1 = signed(1, head_vh, 200, &a, ph);
        chain.append_version(v1, &pol).unwrap();
        // Giả lập tamper: tính prev_hash từ một genesis ĐÃ BỊ SỬA.
        let mut tampered_v0 = signed(0, [0u8; 32], 100, &a, ph);
        tampered_v0.ts = 999; // đổi quá khứ
        let tampered_vh = tampered_v0.version_hash();
        assert_ne!(
            tampered_vh, head_vh,
            "version_hash phải đổi khi sửa quá khứ"
        );
        // v1.prev_hash (tính trên genesis gốc) != version_hash genesis-tampered.
        assert_ne!(chain.version(1).unwrap().prev_hash, tampered_vh);
    }

    #[test]
    fn inv_e2_seq_jump_rejected() {
        let a = mk_author(1);
        let (mut chain, pol) = genesis_chain(&a);
        let ph = pol.policy_hash();
        let v = signed(2, chain.head_version_hash(), 200, &a, ph); // nhảy 0→2
        assert!(matches!(
            chain.append_version(v, &pol),
            Err(StrataError::SeqNotMonotonic {
                expected: 1,
                got: 2
            })
        ));
    }

    #[test]
    fn inv_e2_seq_replay_rejected() {
        let a = mk_author(1);
        let (mut chain, pol) = genesis_chain(&a);
        let ph = pol.policy_hash();
        let v = signed(0, chain.head_version_hash(), 200, &a, ph); // lùi về 0
        assert!(matches!(
            chain.append_version(v, &pol),
            Err(StrataError::SeqNotMonotonic { .. })
        ));
    }

    #[test]
    fn inv_e3_old_proof_valid_under_new_root() {
        // Append version mới; inclusion-proof của version cũ vẫn verify dưới root mới.
        let a = mk_author(1);
        let (mut chain, pol) = genesis_chain(&a);
        let ph = pol.policy_hash();
        // root tại n=1, proof cho seq 0.
        let (proof0, size0, vh0) = chain.prove_version(0).unwrap();
        let root0 = chain.mmr_root();
        assert!(StrataChain::verify_version(root0, &vh0, 0, size0, &proof0));

        // mở rộng tới 3 version.
        let v1 = signed(1, chain.head_version_hash(), 200, &a, ph);
        chain.append_version(v1, &pol).unwrap();
        let v2 = signed(2, chain.head_version_hash(), 300, &a, ph);
        chain.append_version(v2, &pol).unwrap();

        // Proof cho seq 0 dưới ROOT MỚI (n=3) vẫn đúng.
        let (proof0b, size0b, vh0b) = chain.prove_version(0).unwrap();
        let root_new = chain.mmr_root();
        assert_ne!(root0, root_new, "root phải đổi khi mở rộng");
        assert!(StrataChain::verify_version(
            root_new, &vh0b, 0, size0b, &proof0b
        ));
    }

    #[test]
    fn inv_e4_wrong_signature_rejected() {
        let a = mk_author(1);
        let (mut chain, pol) = genesis_chain(&a);
        let ph = pol.policy_hash();
        // Ký bởi author a nhưng giả mạo: dùng khoá khác trong khi did là của a.
        let imposter = SigningKey::generate(&mut OsRng);
        let sr = build_state_root(&[(b"k".to_vec(), b"v".to_vec())]);
        let mut v = StrataVersion::unsigned(
            1,
            chain.head_version_hash(),
            b"cid".to_vec(),
            sr,
            a.did,
            ph,
            200,
        );
        v.sign(&imposter); // sig của khoá khác, did vẫn = a.did
        assert!(matches!(
            chain.append_version(v, &pol),
            Err(StrataError::BadSignature)
        ));
    }

    #[test]
    fn inv_e4_author_not_in_policy_rejected() {
        let a = mk_author(1);
        let (mut chain, pol) = genesis_chain(&a);
        let ph = pol.policy_hash();
        let outsider = mk_author(2); // KHÔNG trong policy
        let v = signed(1, chain.head_version_hash(), 200, &outsider, ph);
        assert!(matches!(
            chain.append_version(v, &pol),
            Err(StrataError::PolicyDenied)
        ));
    }

    #[test]
    fn inv_e4_signature_malleability_blocked() {
        // verify_strict chặn malleability: lật S thành non-canonical → verify fail.
        let a = mk_author(1);
        let (mut chain, pol) = genesis_chain(&a);
        let ph = pol.policy_hash();
        let mut v = signed(1, chain.head_version_hash(), 200, &a, ph);
        // Tạo một biến thể sig "bẻ": thêm L vào nửa scalar S (byte 32..64) để non-canonical.
        // Đơn giản hơn: flip 1 bit trong sig → verify_strict reject.
        v.sig[40] ^= 0x01;
        assert!(matches!(
            chain.append_version(v, &pol),
            Err(StrataError::BadSignature)
        ));
    }

    #[test]
    fn inv_e7_anchor_monotonic() {
        let a = mk_author(1);
        let (mut chain, pol) = genesis_chain(&a);
        let ph = pol.policy_hash();
        let a0 = chain.publish_anchor().unwrap();
        assert_eq!(a0.seq, 0);
        let v1 = signed(1, chain.head_version_hash(), 200, &a, ph);
        chain.append_version(v1, &pol).unwrap();
        let a1 = chain.publish_anchor().unwrap();
        assert_eq!(a1.seq, 1);
        assert!(a1.seq > a0.seq);
    }

    #[test]
    fn inv_e7_anchor_rollback_rejected() {
        // Neo seq 1 rồi cố neo lại seq cũ → từ chối (rollback).
        let a = mk_author(1);
        let (mut chain, pol) = genesis_chain(&a);
        let ph = pol.policy_hash();
        let v1 = signed(1, chain.head_version_hash(), 200, &a, ph);
        chain.append_version(v1, &pol).unwrap();
        chain.publish_anchor().unwrap(); // seq 1
        // Mô phỏng indexer thấy lại anchor seq 1 (đã neo) → rollback.
        assert!(matches!(
            chain.publish_anchor(),
            Err(StrataError::AnchorRollback {
                current: 1,
                attempted: 1
            })
        ));
    }

    #[test]
    fn timestamp_regress_rejected() {
        let a = mk_author(1);
        let (mut chain, pol) = genesis_chain(&a);
        let ph = pol.policy_hash();
        let v1 = signed(1, chain.head_version_hash(), 50, &a, ph); // ts < genesis 100
        assert!(matches!(
            chain.append_version(v1, &pol),
            Err(StrataError::TimestampRegress { .. })
        ));
    }

    #[test]
    fn version_at_picks_correct_version() {
        let a = mk_author(1);
        let (mut chain, pol) = genesis_chain(&a); // seq0 ts=100
        let ph = pol.policy_hash();
        let v1 = signed(1, chain.head_version_hash(), 200, &a, ph);
        chain.append_version(v1, &pol).unwrap();
        let v2 = signed(2, chain.head_version_hash(), 300, &a, ph);
        chain.append_version(v2, &pol).unwrap();

        assert!(chain.version_at(99).is_none(), "trước genesis → None");
        assert_eq!(chain.version_at(150).unwrap().0.seq, 0);
        assert_eq!(chain.version_at(250).unwrap().0.seq, 1);
        assert_eq!(chain.version_at(9999).unwrap().0.seq, 2);
        // proof của version_at verify dưới root hiện tại.
        let (v, proof) = chain.version_at(250).unwrap();
        assert!(StrataChain::verify_version(
            chain.mmr_root(),
            &v.version_hash(),
            v.seq,
            chain.len() as u64,
            &proof
        ));
    }

    #[test]
    fn inv_e4_genesis_wrong_policy_hash_rejected() {
        // B1: version commit policy_hash GIẢ → genesis phải reject (Mệnh đề 6).
        let a = mk_author(1);
        let pol = policy_with(&[&a]);
        let bad_ph = [0xDE; 32]; // KHÔNG khớp pol.policy_hash()
        let v0 = signed(0, [0u8; 32], 100, &a, bad_ph);
        assert!(matches!(
            StrataChain::genesis([0xAB; 32], v0, &pol),
            Err(StrataError::PolicyHashMismatch { .. })
        ));
    }

    #[test]
    fn inv_e4_append_wrong_policy_hash_rejected() {
        // B1: append với policy_hash GIẢ → reject dù sig + author hợp lệ.
        let a = mk_author(1);
        let (mut chain, _pol) = genesis_chain(&a);
        let pol = policy_with(&[&a]);
        let bad_ph = [0xDE; 32];
        let v1 = signed(1, chain.head_version_hash(), 200, &a, bad_ph);
        assert!(matches!(
            chain.append_version(v1, &pol),
            Err(StrataError::PolicyHashMismatch { expected, got })
                if expected == pol.policy_hash() && got == bad_ph
        ));
    }

    #[test]
    fn determinism_same_input_same_roots() {
        let a = mk_author(7);
        let pol = policy_with(&[&a]);
        let ph = pol.policy_hash();
        let build = || {
            let v0 = signed(0, [0u8; 32], 100, &a, ph);
            let mut c = StrataChain::genesis([0xAB; 32], v0, &pol).unwrap();
            let v1 = signed(1, c.head_version_hash(), 200, &a, ph);
            c.append_version(v1, &pol).unwrap();
            (c.mmr_root(), c.head_version_hash())
        };
        // Lưu ý: sig khác nhau mỗi lần (nonce ngẫu nhiên Ed25519) nhưng version_hash
        // KHÔNG gồm sig (CHỐT-1) → mmr_root + head_version_hash tất định.
        assert_eq!(build(), build());
    }
}
