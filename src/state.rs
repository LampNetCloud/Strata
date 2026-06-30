//! `state_root` field-level + field-proof (INV-E6, CHỐT-4).
//!
//! Khối "Mã hóa state leaf" của `_CONTRACT.md`:
//! ```text
//! fvh  = H_dom("LN/STRATA/state/fval/v1", field_value_bytes)
//! leaf = H_dom("LN/STRATA/state/leaf/v1", u32_be(len(key)) ‖ key ‖ fvh)
//! node = H_dom("LN/STRATA/state/node/v1", left ‖ right)
//! state_root = node-root trên các leaf đã sort theo key tăng dần
//! ```
//! `field_value_bytes` có thể là giá trị inline HOẶC content_cid THUẦN (CHỐT-4 —
//! KHÔNG class byte, để field-proof không leak loại).
//!
//! Cây dùng tag RIÊNG (TAG_STATE_*), KHÔNG đụng tag MMR. Dup-leaf guard: số lá lẻ
//! KHÔNG copy lá cuối — carry lên tầng trên (đối xứng với MMR carry, INV-E8).
//! Proof một trường chỉ lộ `value_cid + fvh + sibling-hash`; KHÔNG lộ key/value
//! trường khác (sibling đã băm).

use crate::u32_be;
use lampnet_merkle_anchor::hash::{h_dom, Hash32};

/// Tag băm giá trị trường (CHỐT-2).
pub const TAG_STATE_FVAL: &str = "LN/STRATA/state/fval/v1";
/// Tag state leaf (key + fval).
pub const TAG_STATE_LEAF: &str = "LN/STRATA/state/leaf/v1";
/// Tag state internal node.
pub const TAG_STATE_NODE: &str = "LN/STRATA/state/node/v1";

/// `fvh = H_dom(TAG_STATE_FVAL, field_value_bytes)`.
pub fn fval_hash(field_value_bytes: &[u8]) -> Hash32 {
    h_dom(TAG_STATE_FVAL, field_value_bytes)
}

/// `leaf = H_dom(TAG_STATE_LEAF, u32_be(len(key)) ‖ key ‖ fvh)`.
pub fn leaf_hash(key: &[u8], fvh: &Hash32) -> Hash32 {
    let mut buf = Vec::with_capacity(4 + key.len() + 32);
    buf.extend_from_slice(&u32_be(key.len()));
    buf.extend_from_slice(key);
    buf.extend_from_slice(fvh);
    h_dom(TAG_STATE_LEAF, &buf)
}

/// `node = H_dom(TAG_STATE_NODE, left ‖ right)`.
fn node_hash(left: &Hash32, right: &Hash32) -> Hash32 {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(left);
    buf.extend_from_slice(right);
    h_dom(TAG_STATE_NODE, &buf)
}

/// Sắp các field theo key tăng dần và trả về leaf-hash tương ứng (tất định).
fn sorted_leaves(fields: &[(Vec<u8>, Vec<u8>)]) -> Vec<(Vec<u8>, Hash32)> {
    let mut v: Vec<(Vec<u8>, Hash32)> = fields
        .iter()
        .map(|(k, val)| (k.clone(), leaf_hash(k, &fval_hash(val))))
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// Một tầng cây: gộp đôi, lá lẻ carry nguyên lên (dup-leaf guard — KHÔNG copy).
fn fold_level(level: &[Hash32]) -> Vec<Hash32> {
    let mut next = Vec::with_capacity(level.len().div_ceil(2));
    let mut i = 0;
    while i + 1 < level.len() {
        next.push(node_hash(&level[i], &level[i + 1]));
        i += 2;
    }
    if i < level.len() {
        next.push(level[i]); // carry lá lẻ — không nhân đôi (CVE-2012-2459)
    }
    next
}

/// `build_state_root(fields)` — state_root field-level (§3.6). `fields` rỗng → 0^32.
pub fn build_state_root(fields: &[(Vec<u8>, Vec<u8>)]) -> Hash32 {
    let leaves: Vec<Hash32> = sorted_leaves(fields).into_iter().map(|(_, h)| h).collect();
    if leaves.is_empty() {
        return [0u8; 32];
    }
    let mut level = leaves;
    while level.len() > 1 {
        level = fold_level(&level);
    }
    level[0]
}

/// Field-proof từ `state_root` (INV-E6). KHÔNG lộ giá trị/khoá trường khác — sibling
/// chỉ là hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldProof {
    /// Khoá trường được chứng minh.
    pub key: Vec<u8>,
    /// `field_value_bytes` công khai (content_cid THUẦN hoặc giá trị inline — CHỐT-4).
    /// Verifier băm lại để so `fvh`.
    pub value: Vec<u8>,
    /// `fvh` của trường (tiện cho verifier; cũng tính lại được từ `value`).
    pub fvh: Hash32,
    /// Đường anh em từ lá lên root: `(sibling_hash, sibling_is_right)`.
    /// `sibling_is_right == true` ⇒ nút hiện tại là con TRÁI.
    pub siblings: Vec<(Hash32, bool)>,
    /// state_root mục tiêu (gắn với một version).
    pub state_root: Hash32,
}

/// Sinh field-proof cho `key`. Trả `None` nếu key không có trong `fields`.
pub fn prove_field(fields: &[(Vec<u8>, Vec<u8>)], key: &[u8]) -> Option<FieldProof> {
    let value = fields.iter().find(|(k, _)| k == key)?.1.clone();
    let fvh = fval_hash(&value);

    let leaves: Vec<Hash32> = sorted_leaves(fields).into_iter().map(|(_, h)| h).collect();
    // Vị trí của leaf đang chứng minh (key duy nhất sau khi caller bảo đảm; nếu trùng
    // key, lấy lần xuất hiện đầu sau sort).
    let target_leaf = leaf_hash(key, &fvh);
    let mut pos = leaves.iter().position(|h| *h == target_leaf)?;

    let mut level = leaves;
    let mut siblings = Vec::new();
    while level.len() > 1 {
        let is_left = pos % 2 == 0;
        if is_left {
            if pos + 1 < level.len() {
                // có anh em bên phải
                siblings.push((level[pos + 1], true));
            }
            // pos lẻ-cuối được carry → không có sibling ở tầng này
        } else {
            siblings.push((level[pos - 1], false));
        }
        level = fold_level(&level);
        pos /= 2;
    }

    Some(FieldProof {
        key: key.to_vec(),
        value,
        fvh,
        siblings,
        state_root: build_state_root(fields),
    })
}

/// Verify field-proof: tính lại root từ `value` + đường anh em, so `state_root`.
/// INV-E6: không cần biết trường khác — chỉ dùng sibling-hash.
pub fn verify_field_proof(proof: &FieldProof) -> bool {
    // 1. fvh phải khớp băm của value công khai (chống khai man fvh).
    if fval_hash(&proof.value) != proof.fvh {
        return false;
    }
    // 2. Tính lại từ leaf lên root.
    let mut acc = leaf_hash(&proof.key, &proof.fvh);
    for (sib, sib_is_right) in &proof.siblings {
        acc = if *sib_is_right {
            node_hash(&acc, sib) // nút hiện tại con trái, anh em bên phải
        } else {
            node_hash(sib, &acc)
        };
    }
    acc == proof.state_root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> Vec<(Vec<u8>, Vec<u8>)> {
        vec![
            (b"diagnosis".to_vec(), b"cid_diag".to_vec()),
            (b"birthdate".to_vec(), b"cid_bd".to_vec()),
            (b"name".to_vec(), b"cid_name".to_vec()),
            (b"address".to_vec(), b"cid_addr".to_vec()),
            (b"phone".to_vec(), b"cid_phone".to_vec()),
        ]
    }

    #[test]
    fn root_deterministic_regardless_of_input_order() {
        let mut f1 = fields();
        let mut f2 = fields();
        f2.reverse();
        assert_eq!(build_state_root(&f1), build_state_root(&f2));
        f1.swap(0, 2);
        assert_eq!(build_state_root(&f1), build_state_root(&f2));
    }

    #[test]
    fn field_proof_round_trip_all_fields() {
        let f = fields();
        for (k, _) in &f {
            let p = prove_field(&f, k).expect("proof tồn tại");
            assert!(verify_field_proof(&p), "verify trường {:?}", k);
            assert_eq!(p.state_root, build_state_root(&f));
        }
    }

    #[test]
    fn field_proof_fails_on_value_tamper() {
        // INV-E6: đổi giá trị trường được chứng minh → verify fail.
        let f = fields();
        let mut p = prove_field(&f, b"diagnosis").unwrap();
        p.value = b"cid_FAKE".to_vec();
        p.fvh = fval_hash(&p.value); // khai báo fvh khớp value giả
        assert!(!verify_field_proof(&p), "value giả phải fail (root không khớp)");
    }

    #[test]
    fn field_proof_no_leak_other_fields() {
        // INV-E6: proof của 1 trường KHÔNG chứa key/value trường khác — chỉ sibling-hash.
        let f = fields();
        let p = prove_field(&f, b"diagnosis").unwrap();
        // Không sibling nào trùng một giá trị thô của trường khác.
        for (k, val) in &f {
            if k == b"diagnosis" {
                continue;
            }
            for (sib, _) in &p.siblings {
                assert_ne!(sib.as_slice(), val.as_slice(), "sibling không được là value thô");
                assert_ne!(sib.as_slice(), k.as_slice(), "sibling không được là key thô");
            }
        }
    }

    #[test]
    fn field_proof_cross_field_substitution_fails() {
        // Lấy proof của trường A, đổi key sang trường B (cùng cây) → fail.
        let f = fields();
        let mut p = prove_field(&f, b"name").unwrap();
        p.key = b"phone".to_vec();
        assert!(!verify_field_proof(&p));
    }

    #[test]
    fn single_field_tree() {
        let f = vec![(b"only".to_vec(), b"v".to_vec())];
        let p = prove_field(&f, b"only").unwrap();
        assert!(verify_field_proof(&p));
        assert!(p.siblings.is_empty());
    }

    #[test]
    fn cid_value_is_pure_no_class_byte() {
        // CHỐT-4: value_cid công khai trong proof = content_cid thuần (ở đây 32B hash thuần),
        // KHÔNG có class byte dẫn đầu → không leak loại.
        let pure_cid = [0x11u8; 32].to_vec();
        let f = vec![(b"file".to_vec(), pure_cid.clone())];
        let p = prove_field(&f, b"file").unwrap();
        assert_eq!(p.value, pure_cid);
        assert!(verify_field_proof(&p));
    }
}
