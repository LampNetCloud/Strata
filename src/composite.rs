//! Composite Strata — đối tượng thật là RỪNG các Strata nguyên thủy ghép cha–con
//! (Feat §6, Math §12, API §5.1).
//!
//! KHÔNG có primitive mới: một Strata ghép `C` là một Strata loại #4 mà `state`
//! chứa các tham chiếu con. Mỗi con xuất hiện như một trường:
//! ```text
//! field_key   = role            (VD b"profile", b"posts", b"counters")
//! field_value = child_ref_id    (32B hash THUẦN — CHỐT-4, KHÔNG class byte)
//! ```
//! Vậy `state_root(C)` (state.rs §6) cam kết toàn bộ danh sách con; thêm/bớt con =
//! một `append_version` mới của cha (INV-E1/E2 nguyên). Quan hệ cha–con đệ quy: con
//! của `C` lại có thể là composite.
//!
//! **Proof hai tầng** (`composite_two_tier_proof`): chứng minh "phần tử x thuộc đối
//! tượng ghép C" = field-proof cha (`role → child_ref_id`) CỘNG proof bên trong
//! Strata con (inclusion-proof #1/#2 hoặc field-proof #4). Verifier nối: gốc tầng
//! con phải khớp `child_ref_id` mà field-proof cha trả về. Mỗi tầng `O(log)`.

use crate::state::{FieldProof, build_state_root, prove_field, verify_field_proof};
use crate::version::Hash32;

/// Một tham chiếu con trong Composite: `(role, child_ref_id)`.
pub type ChildRef = (Vec<u8>, Hash32);

/// Dựng danh sách state-fields của một Strata ghép từ các con `(role, child_ref_id)`.
///
/// Mỗi con → một trường `(role, child_ref_id.to_vec())`. `child_ref_id` PHẢI là
/// `ref_id` THUẦN 32B (CHỐT-4) — không nhúng loại. Kết quả đưa thẳng vào
/// [`build_state_root`] để có `state_root` của Strata cha.
pub fn composite_state_fields(children: &[ChildRef]) -> Vec<(Vec<u8>, Vec<u8>)> {
    children
        .iter()
        .map(|(role, ref_id)| (role.clone(), ref_id.to_vec()))
        .collect()
}

/// `state_root` của một Strata ghép cam kết đúng tập con `children`.
pub fn composite_state_root(children: &[ChildRef]) -> Hash32 {
    build_state_root(&composite_state_fields(children))
}

/// Bằng chứng "con vai trò `role` là `child_ref_id`" thuộc Strata ghép (tầng CHA).
/// Chính là một [`FieldProof`] từ `state_root(C)` với `value == child_ref_id`.
///
/// Verify tầng cha: [`verify_field_proof`] hợp lệ (value khớp cam kết) VÀ
/// `value == child_ref_id` mong đợi ([`ParentChildProof::verify`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentChildProof {
    /// Vai trò con (khoá trường trong state cha).
    pub role: Vec<u8>,
    /// `ref_id` con mà proof khẳng định (32B thuần).
    pub child_ref_id: Hash32,
    /// Field-proof cha (`role → child_ref_id`) từ `state_root(C)`.
    pub field_proof: FieldProof,
}

impl ParentChildProof {
    /// Verify tầng cha: field-proof hợp lệ, khoá đúng `role`, và giá trị = đúng
    /// `child_ref_id` (32B). KHÔNG lộ con khác (INV-E6 ở tầng cha, Math §12).
    pub fn verify(&self) -> bool {
        verify_field_proof(&self.field_proof)
            && self.field_proof.key == self.role
            && self.field_proof.value.as_slice() == self.child_ref_id.as_slice()
    }
}

/// Sinh proof tầng cha cho con `role`. `None` nếu `role` không có trong `children`.
pub fn prove_child(children: &[ChildRef], role: &[u8]) -> Option<ParentChildProof> {
    let child_ref_id = children.iter().find(|(r, _)| r == role)?.1;
    let fields = composite_state_fields(children);
    let field_proof = prove_field(&fields, role)?;
    Some(ParentChildProof {
        role: role.to_vec(),
        child_ref_id,
        field_proof,
    })
}

/// Nối proof hai tầng: sau khi verify [`ParentChildProof`] (con `role → child_ref_id`
/// thuộc cha), verifier còn phải chắc **gốc tầng con** (`mmr_root`/`state_root`/
/// `version_hash` của Strata con) thuộc đúng `child_ref_id` đó qua kênh riêng (anchor
/// con đã neo). Hàm này kiểm bước NỐI: proof cha hợp lệ và `child_ref_id` khớp cái mà
/// tầng con dùng.
///
/// `child_anchored_ref_id` = `ref_id` của Strata con mà proof tầng con (inclusion/
/// field) đã được verify dưới anchor riêng của nó. Trả `true` nếu hai tầng nối khớp.
pub fn link_two_tier(parent: &ParentChildProof, child_anchored_ref_id: &Hash32) -> bool {
    parent.verify() && parent.child_ref_id.as_slice() == child_anchored_ref_id.as_slice()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::{Policy, StrataChain};
    use crate::refid::gen_ref_id_raw;
    use crate::state::{prove_field as state_prove_field, verify_field_proof};
    use crate::version::StrataVersion;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    fn mk_child_ref(tag: u8) -> Hash32 {
        gen_ref_id_raw(&[tag; 32], &[tag.wrapping_add(1); 32])
    }

    #[test]
    fn composite_root_commits_all_children() {
        let children = vec![
            (b"profile".to_vec(), mk_child_ref(1)),
            (b"posts".to_vec(), mk_child_ref(2)),
            (b"counters".to_vec(), mk_child_ref(3)),
        ];
        let r1 = composite_state_root(&children);
        // Đổi một con → root đổi.
        let mut c2 = children.clone();
        c2[1].1 = mk_child_ref(9);
        assert_ne!(r1, composite_state_root(&c2));
        // Đảo thứ tự nhập → cùng root (sort theo role trong state tree).
        let mut c3 = children.clone();
        c3.reverse();
        assert_eq!(r1, composite_state_root(&c3));
    }

    #[test]
    fn parent_child_proof_round_trip() {
        let children = vec![
            (b"profile".to_vec(), mk_child_ref(1)),
            (b"posts".to_vec(), mk_child_ref(2)),
            (b"counters".to_vec(), mk_child_ref(3)),
        ];
        for (role, ref_id) in &children {
            let p = prove_child(&children, role).expect("con tồn tại");
            assert!(p.verify(), "verify con {role:?}");
            assert_eq!(&p.child_ref_id, ref_id);
        }
        // Con không tồn tại → None.
        assert!(prove_child(&children, b"unknown").is_none());
    }

    #[test]
    fn parent_proof_forged_child_ref_fails() {
        // Đổi child_ref_id trong proof (giả mạo con khác) → verify fail (value != cam kết).
        let children = vec![
            (b"profile".to_vec(), mk_child_ref(1)),
            (b"posts".to_vec(), mk_child_ref(2)),
        ];
        let mut p = prove_child(&children, b"posts").unwrap();
        p.child_ref_id = mk_child_ref(9);
        assert!(!p.verify(), "child_ref_id giả phải bị chặn");
    }

    /// Test #16 (Tech §9): proof "phần tử x thuộc đối tượng ghép C" = field-proof cha
    /// (`role → ref_id`) + proof CON; gốc con khớp giá trị cha trả về.
    ///
    /// Kịch bản profile MXH: cha = composite {profile, posts, counters}; con `posts`
    /// là một Strata #4 thật. Chứng minh "bài đăng title=X thuộc posts thuộc profile".
    #[test]
    fn composite_two_tier_proof() {
        // --- Tầng CON: dựng Strata `posts` (một chain thật) với field "title". ---
        let owner_sk = SigningKey::generate(&mut OsRng);
        let owner_did = [7u8; 32];
        let mut pol = Policy::new();
        pol.allow(owner_did, owner_sk.verifying_key());
        let ph = pol.policy_hash();

        let posts_ref_id = gen_ref_id_raw(&owner_did, &[42u8; 32]);
        let child_fields = vec![
            (b"title".to_vec(), b"cid_post_X".to_vec()),
            (b"body".to_vec(), b"cid_body".to_vec()),
        ];
        let child_state_root = build_state_root(&child_fields);
        let mut v0 = StrataVersion::unsigned(
            0,
            [0u8; 32],
            b"cid_posts".to_vec(),
            child_state_root,
            owner_did,
            ph,
            100,
        );
        v0.sign(&owner_sk);
        let child_chain = StrataChain::genesis(posts_ref_id, v0, &pol).unwrap();

        // Proof tầng con: "title == cid_post_X" thuộc state_root của posts head.
        let child_fp = state_prove_field(&child_fields, b"title").unwrap();
        assert!(verify_field_proof(&child_fp));
        assert_eq!(child_fp.state_root, child_chain.head().state_root);

        // --- Tầng CHA: composite {profile, posts, counters}. ---
        let children = vec![
            (b"profile".to_vec(), mk_child_ref(1)),
            (b"posts".to_vec(), posts_ref_id),
            (b"counters".to_vec(), mk_child_ref(3)),
        ];
        let parent = prove_child(&children, b"posts").expect("con posts tồn tại");
        assert!(parent.verify(), "proof cha hợp lệ");

        // --- NỐI hai tầng: child_ref_id cha trả về == ref_id của chain con đã neo. ---
        assert!(
            link_two_tier(&parent, &posts_ref_id),
            "gốc tầng con phải khớp child_ref_id mà cha cam kết"
        );
        // Và proof tầng con verify độc lập dưới state_root con.
        assert!(verify_field_proof(&child_fp));

        // Red-team: nối với một ref_id con KHÁC → fail.
        assert!(!link_two_tier(&parent, &mk_child_ref(99)));
    }
}
