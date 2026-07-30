//! Hex ở rìa ngoài (§3: "Hash/CID truyền hex"). Lõi vẫn là byte thuần — chuyển đổi
//! CHỈ xảy ra ở lớp DTO này, không rò hex vào trong.

use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

/// Giải hex vào `[u8; N]` — độ dài PHẢI đúng `N` byte (không pad, không truncate).
pub fn decode_fixed<const N: usize>(s: &str) -> Result<[u8; N], String> {
    let raw = hex::decode(s).map_err(|e| format!("hex không hợp lệ: {e}"))?;
    if raw.len() != N {
        return Err(format!("cần {N} byte, nhận {}", raw.len()));
    }
    let mut out = [0u8; N];
    out.copy_from_slice(&raw);
    Ok(out)
}

/// Giải hex độ dài tuỳ ý (`content_cid` biến độ dài).
pub fn decode_var(s: &str) -> Result<Vec<u8>, String> {
    hex::decode(s).map_err(|e| format!("hex không hợp lệ: {e}"))
}

/// serde cho `[u8; 32]` ⇄ chuỗi hex 64 ký tự.
pub mod h32 {
    use super::*;

    pub fn serialize<S: Serializer>(v: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let s = String::deserialize(d)?;
        decode_fixed::<32>(&s).map_err(D::Error::custom)
    }
}

/// serde cho `[u8; 64]` (chữ ký Ed25519) ⇄ chuỗi hex 128 ký tự.
pub mod h64 {
    use super::*;

    pub fn serialize<S: Serializer>(v: &[u8; 64], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        let s = String::deserialize(d)?;
        decode_fixed::<64>(&s).map_err(D::Error::custom)
    }
}

/// serde cho `Vec<u8>` độ dài tuỳ ý ⇄ hex.
pub mod hvar {
    use super::*;

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        decode_var(&s).map_err(D::Error::custom)
    }
}
