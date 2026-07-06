//! Payload codec — metadata label 1234, CBOR **raw bytes** (quyết định chốt: KHÔNG
//! JSON-hex; tiết kiệm ~50% byte so hex-text).
//!
//! Layout (metadatum của label 1234):
//! ```text
//! metadatum = [ record* ]                       // mảng — nhiều anchor/nhiều chain gộp 1 tx
//! record    = { "t": uint, "a": [ ...fields ] } // "t" = discriminator kiểu bản ghi
//! t=1 (StrataAnchor): a = [ ref_id b32, head_version_hash b32, mmr_root b32, seq uint ]
//!                     — 4 trường ĐÚNG thứ tự canonical StrataAnchor (_CONTRACT.md)
//! t=2 (key-rotation): a = [ opaque bytes ]      // dành chỗ, chưa dùng ở S1
//! ```
//!
//! Quy tắc chunk 64B (giới hạn bytestring metadata Cardano):
//! - bytes ≤ 64B → MỘT bytestring (KHÔNG được chunk — chống malleability);
//! - bytes > 64B → mảng chunk, mọi chunk trừ chunk cuối PHẢI đúng 64B, chunk cuối
//!   1..=64B. Decode từ chối mọi chunking khác → một dãy bytes chỉ có ĐÚNG MỘT
//!   biểu diễn hợp lệ (bijection, chặn đầu độc bằng biến thể encode).
//!
//! Decode khoan dung có kiểm soát: record `t` lạ → BỎ QUA (forward-compat); record
//! `t=1` NHƯNG sai hình dạng (thiếu trường, bytes ≠ 32B, seq âm/quá u64) → LỖI cứng
//! khi ở chế độ strict, hoặc bỏ qua ở chế độ resolve (kẻ lạ không DoS được resolve
//! bằng record rác — xem [`decode_records_lenient`]).

use ciborium::value::{Integer, Value};
use lampnet_strata::StrataAnchor;

/// Giới hạn bytestring trong tx metadata Cardano.
pub const METADATA_BYTES_MAX: usize = 64;

/// Một bản ghi trong metadatum label 1234.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorRecord {
    /// t=1 — StrataAnchor 4 trường canonical.
    Anchor(StrataAnchor),
    /// t=2 — key-rotation (opaque, dành chỗ S1).
    KeyRotation(Vec<u8>),
}

/// Lỗi codec payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadError {
    /// CBOR hỏng / không decode được.
    BadCbor(String),
    /// Hình dạng record sai (thiếu trường, kiểu sai, bytes sai độ dài, seq âm…).
    BadShape(String),
    /// Chunking không canonical (chunk giữa ≠ 64B, hoặc ≤64B mà lại chunk).
    BadChunking,
}

impl std::fmt::Display for PayloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for PayloadError {}

/// Encode bytes theo quy tắc chunk 64B canonical.
fn encode_bytes_chunked(b: &[u8]) -> Value {
    if b.len() <= METADATA_BYTES_MAX {
        Value::Bytes(b.to_vec())
    } else {
        Value::Array(
            b.chunks(METADATA_BYTES_MAX)
                .map(|c| Value::Bytes(c.to_vec()))
                .collect(),
        )
    }
}

/// Decode bytes; enforce chunking canonical (bijection — chống malleability).
fn decode_bytes_chunked(v: &Value) -> Result<Vec<u8>, PayloadError> {
    match v {
        Value::Bytes(b) => {
            if b.len() > METADATA_BYTES_MAX {
                // Bytestring >64B không tồn tại trong metadata hợp lệ, nhưng nếu
                // nguồn dữ liệu đưa vào (mock/hỏng) → từ chối.
                return Err(PayloadError::BadChunking);
            }
            Ok(b.clone())
        }
        Value::Array(chunks) => {
            if chunks.len() < 2 {
                // 0 hoặc 1 chunk mà lại bọc mảng → không canonical.
                return Err(PayloadError::BadChunking);
            }
            let mut out = Vec::with_capacity(chunks.len() * METADATA_BYTES_MAX);
            for (i, c) in chunks.iter().enumerate() {
                let Value::Bytes(b) = c else {
                    return Err(PayloadError::BadShape("chunk không phải bytes".into()));
                };
                let last = i == chunks.len() - 1;
                if (!last && b.len() != METADATA_BYTES_MAX)
                    || (last && (b.is_empty() || b.len() > METADATA_BYTES_MAX))
                {
                    return Err(PayloadError::BadChunking);
                }
                out.extend_from_slice(b);
            }
            Ok(out)
        }
        _ => Err(PayloadError::BadShape("bytes field sai kiểu".into())),
    }
}

fn record_to_value(r: &AnchorRecord) -> Value {
    match r {
        AnchorRecord::Anchor(a) => Value::Map(vec![
            (Value::Text("t".into()), Value::Integer(Integer::from(1u8))),
            (
                Value::Text("a".into()),
                Value::Array(vec![
                    encode_bytes_chunked(&a.ref_id),
                    encode_bytes_chunked(&a.head_version_hash),
                    encode_bytes_chunked(&a.mmr_root),
                    Value::Integer(Integer::from(a.seq)),
                ]),
            ),
        ]),
        AnchorRecord::KeyRotation(payload) => Value::Map(vec![
            (Value::Text("t".into()), Value::Integer(Integer::from(2u8))),
            (
                Value::Text("a".into()),
                Value::Array(vec![encode_bytes_chunked(payload)]),
            ),
        ]),
    }
}

/// Encode danh sách bản ghi → CBOR metadatum (mảng record). Deterministic:
/// thứ tự map cố định `t` rồi `a`, chunking canonical, integer CBOR chuẩn.
pub fn encode_records(records: &[AnchorRecord]) -> Vec<u8> {
    let v = Value::Array(records.iter().map(record_to_value).collect());
    let mut out = Vec::new();
    ciborium::ser::into_writer(&v, &mut out).expect("Vec<u8> writer không fail");
    out
}

fn hash32(v: &Value, name: &str) -> Result<[u8; 32], PayloadError> {
    let b = decode_bytes_chunked(v)?;
    b.try_into()
        .map_err(|_| PayloadError::BadShape(format!("{name} phải đúng 32 byte")))
}

fn record_from_value(v: &Value) -> Result<Option<AnchorRecord>, PayloadError> {
    let Value::Map(entries) = v else {
        return Err(PayloadError::BadShape("record không phải map".into()));
    };
    // Chống malleability kiểu duplicate-key: record PHẢI có đúng 2 entry (t, a) —
    // map có key trùng/khác lạ khiến parser khác nhau thấy giá trị khác nhau.
    if entries.len() != 2 {
        return Err(PayloadError::BadShape(format!(
            "record map phải có đúng 2 entry (t, a), có {}",
            entries.len()
        )));
    }
    let get = |key: &str| -> Option<&Value> {
        entries
            .iter()
            .find(|(k, _)| matches!(k, Value::Text(t) if t == key))
            .map(|(_, val)| val)
    };
    let t = match get("t") {
        Some(Value::Integer(i)) => u64::try_from(*i)
            .map_err(|_| PayloadError::BadShape("t âm/quá lớn".into()))?,
        _ => return Err(PayloadError::BadShape("thiếu discriminator t".into())),
    };
    let Some(Value::Array(a)) = get("a") else {
        return Err(PayloadError::BadShape("thiếu mảng a".into()));
    };
    match t {
        1 => {
            if a.len() != 4 {
                return Err(PayloadError::BadShape(format!(
                    "anchor cần đúng 4 trường, có {}",
                    a.len()
                )));
            }
            let ref_id = hash32(&a[0], "ref_id")?;
            let head_version_hash = hash32(&a[1], "head_version_hash")?;
            let mmr_root = hash32(&a[2], "mmr_root")?;
            let seq = match &a[3] {
                Value::Integer(i) => u64::try_from(*i)
                    .map_err(|_| PayloadError::BadShape("seq âm hoặc > u64::MAX".into()))?,
                _ => return Err(PayloadError::BadShape("seq không phải int".into())),
            };
            Ok(Some(AnchorRecord::Anchor(StrataAnchor {
                ref_id,
                head_version_hash,
                mmr_root,
                seq,
            })))
        }
        2 => {
            if a.len() != 1 {
                return Err(PayloadError::BadShape("key-rotation cần 1 trường".into()));
            }
            Ok(Some(AnchorRecord::KeyRotation(decode_bytes_chunked(&a[0])?)))
        }
        // t lạ → bỏ qua (forward-compat), KHÔNG lỗi.
        _ => Ok(None),
    }
}

fn parse_top_level(cbor: &[u8]) -> Result<Vec<Value>, PayloadError> {
    let v: Value = ciborium::de::from_reader(cbor)
        .map_err(|e| PayloadError::BadCbor(e.to_string()))?;
    match v {
        Value::Array(items) => Ok(items),
        // Một số nguồn (Blockfrost cbor endpoint) có thể bọc {label: metadatum}.
        Value::Map(entries) if entries.len() == 1 => match entries.into_iter().next() {
            Some((Value::Integer(label), Value::Array(items)))
                if u64::try_from(label) == Ok(crate::METADATA_LABEL) =>
            {
                Ok(items)
            }
            _ => Err(PayloadError::BadShape(
                "metadatum không phải mảng record (map lạ)".into(),
            )),
        },
        _ => Err(PayloadError::BadShape("metadatum không phải mảng record".into())),
    }
}

/// Decode STRICT: mọi record phải hợp lệ (t lạ vẫn được bỏ qua, nhưng record hỏng
/// → lỗi). Dùng cho round-trip test + payload TỰ MÌNH tạo.
pub fn decode_records(cbor: &[u8]) -> Result<Vec<AnchorRecord>, PayloadError> {
    let items = parse_top_level(cbor)?;
    let mut out = Vec::with_capacity(items.len());
    for item in &items {
        if let Some(r) = record_from_value(item)? {
            out.push(r);
        }
    }
    Ok(out)
}

/// Decode KHOAN DUNG: record hỏng/lạ bị BỎ QUA thay vì lỗi. Dùng cho `resolve()`
/// đọc dữ liệu on-chain KHÔNG TIN CẬY — kẻ lạ (hoặc tx label-1234 của hệ khác, VD
/// LampNet settlement JSON) không DoS được resolve bằng payload rác.
pub fn decode_records_lenient(cbor: &[u8]) -> Vec<AnchorRecord> {
    let Ok(items) = parse_top_level(cbor) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| record_from_value(item).ok().flatten())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(seq: u64) -> StrataAnchor {
        StrataAnchor {
            ref_id: [0x11; 32],
            head_version_hash: [0x22; 32],
            mmr_root: [0x33; 32],
            seq,
        }
    }

    #[test]
    fn round_trip_single_anchor_bit_exact() {
        let a = anchor(7);
        let cbor = encode_records(&[AnchorRecord::Anchor(a.clone())]);
        let out = decode_records(&cbor).unwrap();
        assert_eq!(out, vec![AnchorRecord::Anchor(a)]);
        // encode lại → byte khớp (deterministic).
        let cbor2 = encode_records(&out);
        assert_eq!(cbor, cbor2);
    }

    #[test]
    fn round_trip_batch_multiple_chains() {
        let mut a2 = anchor(3);
        a2.ref_id = [0x99; 32];
        let records = vec![
            AnchorRecord::Anchor(anchor(7)),
            AnchorRecord::Anchor(a2),
            AnchorRecord::KeyRotation(vec![0xAB; 100]), // >64B → chunk
        ];
        let cbor = encode_records(&records);
        assert_eq!(decode_records(&cbor).unwrap(), records);
    }

    #[test]
    fn seq_boundary_u64_max() {
        let a = anchor(u64::MAX);
        let cbor = encode_records(&[AnchorRecord::Anchor(a.clone())]);
        assert_eq!(decode_records(&cbor).unwrap(), vec![AnchorRecord::Anchor(a)]);
    }

    #[test]
    fn chunk_edges_64_65_128_129() {
        for n in [1usize, 63, 64, 65, 127, 128, 129, 256] {
            let payload = vec![0xCD; n];
            let r = AnchorRecord::KeyRotation(payload.clone());
            let cbor = encode_records(std::slice::from_ref(&r));
            assert_eq!(decode_records(&cbor).unwrap(), vec![r], "n={n}");
        }
    }

    #[test]
    fn non_canonical_chunking_rejected() {
        // 32B mà bọc mảng 2 chunk 16B → decode phải từ chối (malleability).
        let bad = Value::Array(vec![Value::Map(vec![
            (Value::Text("t".into()), Value::Integer(Integer::from(2u8))),
            (
                Value::Text("a".into()),
                Value::Array(vec![Value::Array(vec![
                    Value::Bytes(vec![0u8; 16]),
                    Value::Bytes(vec![0u8; 16]),
                ])]),
            ),
        ])]);
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&bad, &mut cbor).unwrap();
        assert_eq!(decode_records(&cbor), Err(PayloadError::BadChunking));
        // Lenient: bỏ qua, không panic.
        assert!(decode_records_lenient(&cbor).is_empty());
    }

    #[test]
    fn negative_seq_rejected() {
        let bad = Value::Array(vec![Value::Map(vec![
            (Value::Text("t".into()), Value::Integer(Integer::from(1u8))),
            (
                Value::Text("a".into()),
                Value::Array(vec![
                    Value::Bytes(vec![0x11; 32]),
                    Value::Bytes(vec![0x22; 32]),
                    Value::Bytes(vec![0x33; 32]),
                    Value::Integer(Integer::from(-5i64)),
                ]),
            ),
        ])]);
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&bad, &mut cbor).unwrap();
        assert!(matches!(decode_records(&cbor), Err(PayloadError::BadShape(_))));
    }

    #[test]
    fn wrong_hash_len_rejected_strict_skipped_lenient() {
        let bad = Value::Array(vec![Value::Map(vec![
            (Value::Text("t".into()), Value::Integer(Integer::from(1u8))),
            (
                Value::Text("a".into()),
                Value::Array(vec![
                    Value::Bytes(vec![0x11; 31]), // 31B ≠ 32B
                    Value::Bytes(vec![0x22; 32]),
                    Value::Bytes(vec![0x33; 32]),
                    Value::Integer(Integer::from(1u8)),
                ]),
            ),
        ])]);
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&bad, &mut cbor).unwrap();
        assert!(matches!(decode_records(&cbor), Err(PayloadError::BadShape(_))));
        assert!(decode_records_lenient(&cbor).is_empty());
    }

    #[test]
    fn unknown_discriminator_skipped() {
        let v = Value::Array(vec![
            Value::Map(vec![
                (Value::Text("t".into()), Value::Integer(Integer::from(77u8))),
                (Value::Text("a".into()), Value::Array(vec![])),
            ]),
            record_to_value(&AnchorRecord::Anchor(anchor(4))),
        ]);
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&v, &mut cbor).unwrap();
        let out = decode_records(&cbor).unwrap();
        assert_eq!(out, vec![AnchorRecord::Anchor(anchor(4))]);
    }

    #[test]
    fn duplicate_key_map_rejected() {
        // Map 3 entry: t=1, a hợp lệ, rồi "t"=2 TRÙNG KEY → strict phải từ chối
        // (parser ngây thơ có thể thấy t=2), lenient bỏ qua.
        let a_ok = Value::Array(vec![
            Value::Bytes(vec![0x11; 32]),
            Value::Bytes(vec![0x22; 32]),
            Value::Bytes(vec![0x33; 32]),
            Value::Integer(Integer::from(1u8)),
        ]);
        let bad = Value::Array(vec![Value::Map(vec![
            (Value::Text("t".into()), Value::Integer(Integer::from(1u8))),
            (Value::Text("a".into()), a_ok),
            (Value::Text("t".into()), Value::Integer(Integer::from(2u8))),
        ])]);
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&bad, &mut cbor).unwrap();
        assert!(matches!(decode_records(&cbor), Err(PayloadError::BadShape(_))));
        assert!(decode_records_lenient(&cbor).is_empty());
    }

    #[test]
    fn foreign_label_1234_payload_ignored_lenient() {
        // Payload label-1234 của LampNet settlement (map JSON-style) → lenient trả rỗng.
        let foreign = Value::Map(vec![
            (
                Value::Text("merkle_root".into()),
                Value::Text("abcd".into()),
            ),
            (Value::Text("epoch".into()), Value::Integer(Integer::from(9u8))),
        ]);
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&foreign, &mut cbor).unwrap();
        assert!(decode_records_lenient(&cbor).is_empty());
        assert!(decode_records(&cbor).is_err());
    }

    #[test]
    fn label_wrapped_map_unwrapped() {
        // {1234: [record]} — dạng Blockfrost có thể trả.
        let rec = record_to_value(&AnchorRecord::Anchor(anchor(2)));
        let wrapped = Value::Map(vec![(
            Value::Integer(Integer::from(1234u16)),
            Value::Array(vec![rec]),
        )]);
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&wrapped, &mut cbor).unwrap();
        assert_eq!(
            decode_records(&cbor).unwrap(),
            vec![AnchorRecord::Anchor(anchor(2))]
        );
    }
}
