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
///
/// # Vì sao tham số là `&[u8; 32]` chứ không phải `&[u8]`
///
/// Phép nối KHÔNG length-prefix. Với độ dài biến thiên thì `(A‖B, C)` và `(A, B‖C)`
/// cho **cùng** `ref_id` — va chạm **cấu trúc**, chi phí bằng không, không cần va chạm
/// BLAKE3 (issue #39 điểm 1).
///
/// Không vá bằng cách thêm length-prefix vào công thức, vì hai lẽ độc lập:
/// `author_did` là trường **cố định** 32 byte, ghi nguyên byte KHÔNG tiền tố độ dài
/// (`_CONTRACT.md` CHỐT-5, `Strata-Tech.md §1.7` quy tắc 4); và đổi công thức là đổi
/// **giá trị** mọi `ref_id` đã sinh — đang lưu, đang neo on-chain — trong khi `ref_id`
/// KHÔNG đổi qua các phiên bản (INV-E5).
///
/// Chốt: đóng ở **KIỂU**. Hai độ dài cố định thì không có chỗ tách lại, nên tập bên
/// dựng được va chạm đúng bằng tập bên truyền lát cắt độ dài tuỳ ý — và tập đó nay đỏ
/// lúc **biên dịch** thay vì sinh ra một `ref_id` trùng lặng lẽ. Giá trị không đổi một
/// bit; `Strata-Tech.md §2.1` vốn đã viết chữ ký kiểu cố định (`&Did`, `&H32`), nên đây
/// là đưa mã về đúng spec chứ không phải đổi spec.
///
/// Đối chứng **dương** — đúng 32 byte thì biên dịch:
/// ```
/// use lampnet_strata::refid::gen_ref_id_raw;
/// let _ = gen_ref_id_raw(&[1u8; 32], &[2u8; 32]);
/// ```
///
/// Đối chứng **âm** — lát cắt độ dài tuỳ ý KHÔNG biên dịch được. Hai dòng dưới là
/// nguyên văn ca va chạm cũ (`tests/property.rs`), nay chính là cả cái hàng rào:
/// ```compile_fail
/// use lampnet_strata::refid::gen_ref_id_raw;
/// let _ = gen_ref_id_raw(b"ab", b"c");
/// ```
pub fn gen_ref_id_raw(author_did: &[u8; 32], nonce: &[u8; 32]) -> Hash32 {
    let mut x = [0u8; 64];
    x[..32].copy_from_slice(author_did);
    x[32..].copy_from_slice(nonce);
    h_dom(TAG_REF, &x)
}

/// Sinh ref_id và encode `lnref1…` (bech32m). INV-E5: payload = 32B ref_id thuần,
/// KHÔNG version/class byte.
pub fn gen_ref_id(author_did: &[u8; 32], nonce: &[u8; 32]) -> String {
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
        let s = gen_ref_id(&[3u8; 32], &[4u8; 32]);
        let raw = decode_ref_id(&s).expect("decode");
        assert_eq!(raw.len(), 32);
        // Khác (author,nonce) → khác ref_id (collision-resistance của hash nền).
        let other = gen_ref_id_raw(&[3u8; 32], &[5u8; 32]);
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
