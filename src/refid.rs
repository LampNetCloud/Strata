//! `ref_id` — định danh ổn định opaque (INV-E5).
//!
//! `ref_id = H_dom("LN/STRATA/ref/v1", author_did ‖ nonce)` — hash THUẦN, KHÔNG nhúng
//! nhãn loại/độ nhạy (sửa lỗi CID leak Vault/Bulk). Biểu diễn công khai `lnref1…`
//! (bech32m HRP "lnref"). KHÔNG đổi qua các phiên bản.

use lampnet_merkle_anchor::hash::{Hash32, h_dom};

/// Tag domain sinh ref_id (CHỐT-2 — copy nguyên văn bảng domain-tag).
pub const TAG_REF: &str = "LN/STRATA/ref/v1";

/// HRP bech32 cho định danh Strata — tách namespace khỏi content-CID (`ln`).
pub const HRP_REF: &str = "lnref";

/// Sinh ref_id 32 byte thô — THUẦN, không class byte, không nhãn loại (INV-E5).
///
/// `author_did ‖ nonce` → `H_dom(TAG_REF, ·)`. Không phụ thuộc content → bất biến
/// qua mọi version.
pub fn gen_ref_id_raw(author_did: &[u8], nonce: &[u8]) -> Hash32 {
    let mut x = Vec::with_capacity(author_did.len() + nonce.len());
    x.extend_from_slice(author_did);
    x.extend_from_slice(nonce);
    h_dom(TAG_REF, &x)
}

/// Sinh ref_id và encode `lnref1…` (bech32m). INV-E5: payload = 32B ref_id thuần,
/// KHÔNG version/class byte.
pub fn gen_ref_id(author_did: &[u8], nonce: &[u8]) -> String {
    let raw = gen_ref_id_raw(author_did, nonce);
    encode_ref_id(&raw)
}

/// Encode 32B ref_id thô thành `lnref1…` (bech32m).
pub fn encode_ref_id(ref_id: &Hash32) -> String {
    let hrp = bech32::Hrp::parse(HRP_REF).expect("HRP_REF hợp lệ");
    bech32::encode::<bech32::Bech32m>(hrp, ref_id).expect("encode bech32m ref_id")
}

/// Decode `lnref1…` về 32B ref_id thô. Trả `None` nếu sai HRP/độ dài/checksum.
pub fn decode_ref_id(s: &str) -> Option<Hash32> {
    let (hrp, data) = bech32::decode(s).ok()?;
    if hrp.as_str() != HRP_REF {
        return None;
    }
    if data.len() != 32 {
        return None;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&data);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_id_round_trip() {
        let did = [7u8; 32];
        let nonce = [9u8; 32];
        let s = gen_ref_id(&did, &nonce);
        assert!(s.starts_with("lnref1"), "phải có tiền tố lnref1, got {s}");
        let raw = gen_ref_id_raw(&did, &nonce);
        assert_eq!(decode_ref_id(&s), Some(raw));
    }

    #[test]
    fn ref_id_deterministic() {
        let did = [1u8; 32];
        let nonce = [2u8; 32];
        assert_eq!(gen_ref_id(&did, &nonce), gen_ref_id(&did, &nonce));
    }

    #[test]
    fn ref_id_depends_only_on_hash_not_type_label() {
        // INV-E5: hai Strata "khác loại" với CÙNG (author, nonce) cho CÙNG ref_id —
        // chứng tỏ ref_id KHÔNG nhúng nhãn loại. Loại nằm trong state, không trong định danh.
        let did = [5u8; 32];
        let nonce = [6u8; 32];
        let a = gen_ref_id(&did, &nonce); // giả lập Strata loại "Vault"
        let b = gen_ref_id(&did, &nonce); // giả lập Strata loại "Bulk"
        assert_eq!(a, b, "ref_id không được phụ thuộc loại");
    }

    #[test]
    fn ref_id_no_class_byte_in_payload() {
        // Payload decode đúng 32 byte = hash thuần; không có byte loại dẫn đầu.
        let s = gen_ref_id(&[3u8; 32], &[4u8; 16]);
        let raw = decode_ref_id(&s).expect("decode");
        assert_eq!(raw.len(), 32);
        // Khác (author,nonce) → khác ref_id (collision-resistance của hash nền).
        let other = gen_ref_id_raw(&[3u8; 32], &[5u8; 16]);
        assert_ne!(raw, other);
    }

    #[test]
    fn decode_rejects_wrong_hrp() {
        // Một bech32m HRP khác (giả CID "ln") không được nhận là ref_id.
        let hrp = bech32::Hrp::parse("ln").unwrap();
        let bad = bech32::encode::<bech32::Bech32m>(hrp, &[0u8; 32]).unwrap();
        assert_eq!(decode_ref_id(&bad), None);
    }
}
