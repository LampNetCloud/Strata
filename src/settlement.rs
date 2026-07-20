//! S1 backend **Settlement** — neo `StrataAnchor` qua **tx metadata label 1234**
//! (đối chiếu `settle.ts` LampNet). Backend MẶC ĐỊNH của S1 (anh Đức chốt PR #8):
//! payload CBOR **raw bytes** (KHÔNG JSON-hex, tiết kiệm ~50% byte). Mosaic CIP-68
//! (`anchor_sink.rs`) giữ cho hồ sơ giá trị cao.
//!
//! Module này là **lớp THUẦN** (no I/O): codec + logic sink generic theo hai seam
//! [`ChainQuery`] (đọc on-chain) và [`Submitter`] (build+submit tx). Cài đặt I/O thật
//! (Blockfrost + submitter TS Lucid) sống ở crate riêng `lampnet-anchor-io` — giữ crate
//! lõi không kéo `reqwest`/process.
//!
//! **Hợp nhất `AnchoredTable` (anh Đức chốt PR #6 vòng 2 mục 1):** đường Settlement
//! KHÔNG có bảng anchored song song. `resolve()` trả `StrataAnchor` chuẩn; verify ngược
//! dùng CHUNG [`AnchoredTable`](crate::anchor_sink::AnchoredTable) +
//! [`verify_resolved`](crate::anchor_sink::verify_resolved) như backend Mosaic.
//!
//! Nguyên tắc trust (§4.3): anchor CHỈ hợp lệ khi tx phát (INPUT) từ ví publisher đã
//! pin trong [`SinkConfig`] — tx lạ mang label 1234 gửi *tới* publisher không được tính
//! (chống đầu độc indexer). Idempotency §8.1b: đọc on-chain seq TRƯỚC khi build;
//! `on_chain_seq == seq` → `Ok(None)`; `>` → [`AnchorError::RollbackAttempt`].

use crate::anchor_sink::{AnchorBackend, AnchorError, AnchorPriority, AnchorReceipt, AnchorSink};
use crate::chain::StrataAnchor;
use crate::version::Hash32;
use ciborium::value::{Integer, Value};

/// Nhãn metadata Cardano cho anchor Strata (đối chiếu `settle.ts` LampNet dùng 1234).
pub const METADATA_LABEL: u64 = 1234;

/// Giới hạn bytestring trong tx metadata Cardano.
pub const METADATA_BYTES_MAX: usize = 64;

// ────────────────────────────────────────────────────────────────────────────
// Codec — metadatum label 1234, CBOR raw bytes
// ────────────────────────────────────────────────────────────────────────────
//
// Layout (metadatum của label 1234):
//   metadatum = [ record* ]                       // mảng — nhiều anchor/nhiều chain gộp 1 tx
//   record    = { "t": uint, "a": [ ...fields ] } // "t" = discriminator kiểu bản ghi
//   t=1 (StrataAnchor): a = [ ref_id b32, head_version_hash b32, mmr_root b32, seq uint ]
//                       — 4 trường ĐÚNG thứ tự canonical StrataAnchor (_CONTRACT.md)
//   t=2 (key-rotation): a = [ opaque bytes ]      // dành chỗ, chưa dùng ở S1
//
// Quy tắc chunk 64B (giới hạn bytestring metadata Cardano):
// - bytes ≤ 64B → MỘT bytestring (KHÔNG được chunk — chống malleability);
// - bytes > 64B → mảng chunk, mọi chunk trừ chunk cuối PHẢI đúng 64B, chunk cuối
//   1..=64B. Decode từ chối mọi chunking khác → một dãy bytes chỉ có ĐÚNG MỘT
//   biểu diễn hợp lệ (bijection, chặn đầu độc bằng biến thể encode).
//
// Decode khoan dung có kiểm soát: record `t` lạ → BỎ QUA (forward-compat); record `t=1`
// NHƯNG sai hình dạng → LỖI ở chế độ strict, hoặc bỏ qua ở chế độ resolve (kẻ lạ không
// DoS được resolve bằng record rác — xem [`decode_records_lenient`]).

/// Một bản ghi trong metadatum label 1234. (Đổi tên từ `AnchorRecord` bản tham chiếu
/// để không đụng [`AnchorRecord`](crate::anchor_sink::AnchorRecord) = dòng `AnchoredTable`.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettlementRecord {
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
                // Bytestring >64B không tồn tại trong metadata hợp lệ; nguồn mock/hỏng → từ chối.
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

fn record_to_value(r: &SettlementRecord) -> Value {
    match r {
        SettlementRecord::Anchor(a) => Value::Map(vec![
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
        SettlementRecord::KeyRotation(payload) => Value::Map(vec![
            (Value::Text("t".into()), Value::Integer(Integer::from(2u8))),
            (
                Value::Text("a".into()),
                Value::Array(vec![encode_bytes_chunked(payload)]),
            ),
        ]),
    }
}

/// Encode danh sách bản ghi → CBOR metadatum (mảng record). Deterministic: thứ tự map
/// cố định `t` rồi `a`, chunking canonical, integer CBOR chuẩn.
pub fn encode_records(records: &[SettlementRecord]) -> Vec<u8> {
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

fn record_from_value(v: &Value) -> Result<Option<SettlementRecord>, PayloadError> {
    let Value::Map(entries) = v else {
        return Err(PayloadError::BadShape("record không phải map".into()));
    };
    // Chống malleability kiểu duplicate-key: record PHẢI có đúng 2 entry (t, a) — map có
    // key trùng/khác lạ khiến parser khác nhau thấy giá trị khác nhau.
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
        Some(Value::Integer(i)) => {
            u64::try_from(*i).map_err(|_| PayloadError::BadShape("t âm/quá lớn".into()))?
        }
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
            Ok(Some(SettlementRecord::Anchor(StrataAnchor {
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
            Ok(Some(SettlementRecord::KeyRotation(decode_bytes_chunked(
                &a[0],
            )?)))
        }
        // t lạ → bỏ qua (forward-compat), KHÔNG lỗi.
        _ => Ok(None),
    }
}

fn parse_top_level(cbor: &[u8]) -> Result<Vec<Value>, PayloadError> {
    let v: Value =
        ciborium::de::from_reader(cbor).map_err(|e| PayloadError::BadCbor(e.to_string()))?;
    match v {
        Value::Array(items) => Ok(items),
        // Một số nguồn (Blockfrost cbor endpoint) có thể bọc {label: metadatum}.
        Value::Map(entries) if entries.len() == 1 => match entries.into_iter().next() {
            Some((Value::Integer(label), Value::Array(items)))
                if u64::try_from(label) == Ok(METADATA_LABEL) =>
            {
                Ok(items)
            }
            _ => Err(PayloadError::BadShape(
                "metadatum không phải mảng record (map lạ)".into(),
            )),
        },
        _ => Err(PayloadError::BadShape(
            "metadatum không phải mảng record".into(),
        )),
    }
}

/// Decode STRICT: mọi record phải hợp lệ (t lạ vẫn được bỏ qua, nhưng record hỏng →
/// lỗi). Dùng cho round-trip test + payload TỰ MÌNH tạo.
pub fn decode_records(cbor: &[u8]) -> Result<Vec<SettlementRecord>, PayloadError> {
    let items = parse_top_level(cbor)?;
    let mut out = Vec::with_capacity(items.len());
    for item in &items {
        if let Some(r) = record_from_value(item)? {
            out.push(r);
        }
    }
    Ok(out)
}

/// Decode KHOAN DUNG: record hỏng/lạ bị BỎ QUA thay vì lỗi. Dùng cho `resolve()` đọc dữ
/// liệu on-chain KHÔNG TIN CẬY — kẻ lạ (hoặc tx label-1234 của hệ khác, VD LampNet
/// settlement JSON) không DoS được resolve bằng payload rác.
pub fn decode_records_lenient(cbor: &[u8]) -> Vec<SettlementRecord> {
    let Ok(items) = parse_top_level(cbor) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| record_from_value(item).ok().flatten())
        .collect()
}

// ────────────────────────────────────────────────────────────────────────────
// SettlementSink — generic theo hai seam I/O (test-injectable)
// ────────────────────────────────────────────────────────────────────────────

/// Đọc on-chain — trừu tượng hoá Blockfrost để mock được trong unit test. Cài đặt thật
/// (`BlockfrostQuery`) ở crate `lampnet-anchor-io`.
pub trait ChainQuery {
    /// Tx hash liên quan `addr`, MỚI → CŨ, tối đa `limit` tx.
    fn address_txs(&self, addr: &str, limit: usize) -> Result<Vec<String>, AnchorError>;
    /// Địa chỉ các INPUT (không tính collateral) của tx.
    fn tx_input_addresses(&self, txid: &str) -> Result<Vec<String>, AnchorError>;
    /// CBOR metadatum (raw bytes) của `label` trong tx; `None` nếu tx không có label.
    fn tx_metadata_cbor(&self, txid: &str, label: u64) -> Result<Option<Vec<u8>>, AnchorError>;
}

/// Kết quả submit từ backend build tx.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitOutcome {
    /// Tx hash.
    pub txid: String,
    /// Địa chỉ ví đã ký (đối chiếu publisher pin trong config).
    pub address: String,
}

/// Build + sign + submit tx metadata — trừu tượng hoá submitter để mock được. Cài đặt
/// thật (`TsSubmitter`, child-process Lucid Evolution) ở crate `lampnet-anchor-io`.
pub trait Submitter {
    /// Submit tx với metadatum label 1234 = các record đã cho. Trả txid + địa chỉ ví ký.
    fn submit(&self, records: &[SettlementRecord]) -> Result<SubmitOutcome, AnchorError>;
}

/// Cấu hình sink Settlement.
#[derive(Debug, Clone)]
pub struct SinkConfig {
    /// Ví dịch vụ công bố (publisher) — TRUST PIN v1: anchor chỉ hợp lệ nếu tx phát
    /// (input) từ ví này.
    pub publisher_address: String,
    /// Nhãn metadata (mặc định [`METADATA_LABEL`] = 1234).
    pub label: u64,
    /// Trần kích thước metadatum (byte) — vượt → [`AnchorError::DatumTooLarge`]. Cardano
    /// maxTxSize ~16384; để dư địa cho phần tx còn lại.
    pub max_metadatum_bytes: usize,
    /// Trần số tx quét khi `resolve` (ví publisher dùng chung có nhiều tx).
    pub resolve_scan_limit: usize,
}

impl Default for SinkConfig {
    fn default() -> Self {
        Self {
            publisher_address: String::new(),
            label: METADATA_LABEL,
            max_metadatum_bytes: 8 * 1024,
            resolve_scan_limit: 500,
        }
    }
}

/// Sink Settlement: metadata label 1234, generic theo query + submitter.
pub struct SettlementSink<Q: ChainQuery, S: Submitter> {
    cfg: SinkConfig,
    query: Q,
    submitter: S,
}

impl<Q: ChainQuery, S: Submitter> SettlementSink<Q, S> {
    /// Tạo sink; `publisher_address` rỗng → coi như chưa cấu hình (mọi call trả
    /// [`AnchorError::NotConfigured`]).
    pub fn new(cfg: SinkConfig, query: Q, submitter: S) -> Self {
        Self {
            cfg,
            query,
            submitter,
        }
    }

    /// Truy cập config (đọc).
    pub fn config(&self) -> &SinkConfig {
        &self.cfg
    }

    fn ensure_configured(&self) -> Result<(), AnchorError> {
        if self.cfg.publisher_address.is_empty() {
            return Err(AnchorError::NotConfigured);
        }
        Ok(())
    }

    /// Gộp NHIỀU anchor (nhiều chain) vào MỘT tx. Idempotency kiểm từng `ref_id`; anchor
    /// đã neo rồi bị loại khỏi lô; nếu lô rỗng sau lọc → `Ok(None)`. Bất kỳ anchor nào bị
    /// rollback → fail cả lô TRƯỚC khi build tx.
    pub fn publish_batch(
        &self,
        anchors: &[StrataAnchor],
    ) -> Result<Option<AnchorReceipt>, AnchorError> {
        self.ensure_configured()?;
        let mut fresh: Vec<SettlementRecord> = Vec::new();
        for a in anchors {
            match self.resolve(&a.ref_id)? {
                Some(on_chain) if on_chain.seq > a.seq => {
                    return Err(AnchorError::RollbackAttempt {
                        on_chain_seq: on_chain.seq,
                        attempted: a.seq,
                    });
                }
                Some(on_chain) if on_chain.seq == a.seq => {
                    // idempotent no-op cho anchor này.
                }
                _ => fresh.push(SettlementRecord::Anchor(a.clone())),
            }
        }
        if fresh.is_empty() {
            return Ok(None);
        }
        let cbor = encode_records(&fresh);
        if cbor.len() > self.cfg.max_metadatum_bytes {
            return Err(AnchorError::DatumTooLarge { bytes: cbor.len() });
        }
        let outcome = self.submitter.submit(&fresh)?;
        if outcome.address != self.cfg.publisher_address {
            // Ví submitter KHÔNG phải publisher đã pin → anchor vừa đẩy sẽ bị chính
            // resolve() bỏ qua. Fail to hơn im lặng.
            return Err(AnchorError::Rejected(format!(
                "ví submitter ({}) != publisher pin trong config ({})",
                outcome.address, self.cfg.publisher_address
            )));
        }
        Ok(Some(AnchorReceipt {
            txid: outcome.txid,
            backend: AnchorBackend::Settlement,
            slot: None,
        }))
    }
}

impl<Q: ChainQuery, S: Submitter> AnchorSink for SettlementSink<Q, S> {
    fn publish(
        &self,
        anchor: &StrataAnchor,
        priority: AnchorPriority,
    ) -> Result<Option<AnchorReceipt>, AnchorError> {
        if priority == AnchorPriority::NoAnchor {
            return Ok(None);
        }
        self.publish_batch(std::slice::from_ref(anchor))
    }

    fn resolve(&self, ref_id: &Hash32) -> Result<Option<StrataAnchor>, AnchorError> {
        self.ensure_configured()?;
        let txs = self
            .query
            .address_txs(&self.cfg.publisher_address, self.cfg.resolve_scan_limit)?;
        let mut best: Option<StrataAnchor> = None;
        for txid in txs {
            let Some(cbor) = self.query.tx_metadata_cbor(&txid, self.cfg.label)? else {
                continue;
            };
            // TRUST: chỉ tin tx do publisher CHI (địa chỉ publisher trong input).
            // `address_txs` trả cả tx GỬI TỚI publisher (VD faucet) → phải lọc input.
            let inputs = self.query.tx_input_addresses(&txid)?;
            if !inputs.iter().any(|a| a == &self.cfg.publisher_address) {
                continue; // tx từ ví lạ mang label 1234 → bỏ qua
            }
            for rec in decode_records_lenient(&cbor) {
                if let SettlementRecord::Anchor(a) = rec
                    && a.ref_id == *ref_id
                    && best.as_ref().is_none_or(|b| a.seq > b.seq)
                {
                    best = Some(a);
                }
            }
        }
        Ok(best)
    }
}

/// Retry CHỈ với [`AnchorError::Network`] (backoff mũ). Mọi lỗi khác fail-hard ngay.
/// `sleep(ms)` do caller cấp (giữ lớp THUẦN, test injectable — không `thread::sleep`
/// trong crate lõi, đồng bộ style [`MosaicAnchorSink::publish_with_retry`]).
/// `max_attempts=0` coi như 1.
///
/// [`MosaicAnchorSink::publish_with_retry`]: crate::anchor_sink::MosaicAnchorSink::publish_with_retry
pub fn publish_with_retry<K: AnchorSink>(
    sink: &K,
    anchor: &StrataAnchor,
    priority: AnchorPriority,
    max_attempts: u32,
    base_backoff_ms: u64,
    mut sleep: impl FnMut(u64),
) -> Result<Option<AnchorReceipt>, AnchorError> {
    let cap = max_attempts.max(1);
    let mut attempt: u32 = 0;
    loop {
        match sink.publish(anchor, priority) {
            Err(e) if e.is_retryable() && attempt + 1 < cap => {
                sleep(base_backoff_ms.saturating_mul(1u64 << attempt.min(63)));
                attempt += 1;
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    fn anchor(seq: u64) -> StrataAnchor {
        StrataAnchor {
            ref_id: [0x11; 32],
            head_version_hash: [0x22; 32],
            mmr_root: [0x33; 32],
            seq,
        }
    }

    // ---- codec ----

    #[test]
    fn round_trip_single_anchor_bit_exact() {
        let a = anchor(7);
        let cbor = encode_records(&[SettlementRecord::Anchor(a.clone())]);
        let out = decode_records(&cbor).unwrap();
        assert_eq!(out, vec![SettlementRecord::Anchor(a)]);
        // encode lại → byte khớp (deterministic).
        assert_eq!(cbor, encode_records(&out));
    }

    #[test]
    fn round_trip_batch_multiple_chains() {
        let mut a2 = anchor(3);
        a2.ref_id = [0x99; 32];
        let records = vec![
            SettlementRecord::Anchor(anchor(7)),
            SettlementRecord::Anchor(a2),
            SettlementRecord::KeyRotation(vec![0xAB; 100]), // >64B → chunk
        ];
        let cbor = encode_records(&records);
        assert_eq!(decode_records(&cbor).unwrap(), records);
    }

    #[test]
    fn seq_boundary_u64_max() {
        let a = anchor(u64::MAX);
        let cbor = encode_records(&[SettlementRecord::Anchor(a.clone())]);
        assert_eq!(
            decode_records(&cbor).unwrap(),
            vec![SettlementRecord::Anchor(a)]
        );
    }

    #[test]
    fn chunk_edges_63_64_65_128_129() {
        for n in [1usize, 63, 64, 65, 127, 128, 129, 256] {
            let payload = vec![0xCD; n];
            let r = SettlementRecord::KeyRotation(payload.clone());
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
        assert!(matches!(
            decode_records(&cbor),
            Err(PayloadError::BadShape(_))
        ));
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
        assert!(matches!(
            decode_records(&cbor),
            Err(PayloadError::BadShape(_))
        ));
        assert!(decode_records_lenient(&cbor).is_empty());
    }

    #[test]
    fn unknown_discriminator_skipped() {
        let v = Value::Array(vec![
            Value::Map(vec![
                (Value::Text("t".into()), Value::Integer(Integer::from(77u8))),
                (Value::Text("a".into()), Value::Array(vec![])),
            ]),
            record_to_value(&SettlementRecord::Anchor(anchor(4))),
        ]);
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&v, &mut cbor).unwrap();
        assert_eq!(
            decode_records(&cbor).unwrap(),
            vec![SettlementRecord::Anchor(anchor(4))]
        );
    }

    #[test]
    fn duplicate_key_map_rejected() {
        // Map 3 entry: t=1, a hợp lệ, rồi "t"=2 TRÙNG KEY → strict từ chối, lenient bỏ qua.
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
        assert!(matches!(
            decode_records(&cbor),
            Err(PayloadError::BadShape(_))
        ));
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
            (
                Value::Text("epoch".into()),
                Value::Integer(Integer::from(9u8)),
            ),
        ]);
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&foreign, &mut cbor).unwrap();
        assert!(decode_records_lenient(&cbor).is_empty());
        assert!(decode_records(&cbor).is_err());
    }

    #[test]
    fn label_wrapped_map_unwrapped() {
        // {1234: [record]} — dạng Blockfrost có thể trả.
        let rec = record_to_value(&SettlementRecord::Anchor(anchor(2)));
        let wrapped = Value::Map(vec![(
            Value::Integer(Integer::from(1234u16)),
            Value::Array(vec![rec]),
        )]);
        let mut cbor = Vec::new();
        ciborium::ser::into_writer(&wrapped, &mut cbor).unwrap();
        assert_eq!(
            decode_records(&cbor).unwrap(),
            vec![SettlementRecord::Anchor(anchor(2))]
        );
    }

    // ---- sink (mock query + submitter) ----

    /// ChainQuery mock: một ví publisher, map txid → (inputs, metadatum cbor).
    #[derive(Default)]
    struct MockQuery {
        publisher: String,
        /// MỚI → CŨ.
        txs: Vec<String>,
        inputs: HashMap<String, Vec<String>>,
        meta: HashMap<String, Vec<u8>>,
    }
    impl ChainQuery for MockQuery {
        fn address_txs(&self, addr: &str, limit: usize) -> Result<Vec<String>, AnchorError> {
            if addr != self.publisher {
                return Ok(Vec::new());
            }
            Ok(self.txs.iter().take(limit).cloned().collect())
        }
        fn tx_input_addresses(&self, txid: &str) -> Result<Vec<String>, AnchorError> {
            Ok(self.inputs.get(txid).cloned().unwrap_or_default())
        }
        fn tx_metadata_cbor(
            &self,
            txid: &str,
            _label: u64,
        ) -> Result<Option<Vec<u8>>, AnchorError> {
            Ok(self.meta.get(txid).cloned())
        }
    }

    /// Submitter mock: ghi lô đã submit vào tx-store dùng chung + trả txid tăng dần.
    struct MockSubmitter {
        publisher: String,
        store: std::rc::Rc<RefCell<MockQuery>>,
        fail_times: RefCell<u32>, // số lần đầu trả Network trước khi thành công
    }
    impl Submitter for MockSubmitter {
        fn submit(&self, records: &[SettlementRecord]) -> Result<SubmitOutcome, AnchorError> {
            {
                let mut ft = self.fail_times.borrow_mut();
                if *ft > 0 {
                    *ft -= 1;
                    return Err(AnchorError::Network("mock tạm lỗi".into()));
                }
            }
            let cbor = encode_records(records);
            let mut q = self.store.borrow_mut();
            let n = q.txs.len();
            let txid = format!("tx{n}");
            q.txs.insert(0, txid.clone()); // MỚI nhất lên đầu
            q.inputs.insert(txid.clone(), vec![self.publisher.clone()]);
            q.meta.insert(txid.clone(), cbor);
            Ok(SubmitOutcome {
                txid,
                address: self.publisher.clone(),
            })
        }
    }

    fn sink_with(
        publisher: &str,
        fail_times: u32,
    ) -> SettlementSink<std::rc::Rc<RefCell<MockQuery>>, MockSubmitter> {
        let store = std::rc::Rc::new(RefCell::new(MockQuery {
            publisher: publisher.to_string(),
            ..Default::default()
        }));
        let submitter = MockSubmitter {
            publisher: publisher.to_string(),
            store: store.clone(),
            fail_times: RefCell::new(fail_times),
        };
        let cfg = SinkConfig {
            publisher_address: publisher.to_string(),
            ..Default::default()
        };
        SettlementSink::new(cfg, store, submitter)
    }

    impl ChainQuery for std::rc::Rc<RefCell<MockQuery>> {
        fn address_txs(&self, addr: &str, limit: usize) -> Result<Vec<String>, AnchorError> {
            self.borrow().address_txs(addr, limit)
        }
        fn tx_input_addresses(&self, txid: &str) -> Result<Vec<String>, AnchorError> {
            self.borrow().tx_input_addresses(txid)
        }
        fn tx_metadata_cbor(&self, txid: &str, label: u64) -> Result<Option<Vec<u8>>, AnchorError> {
            self.borrow().tx_metadata_cbor(txid, label)
        }
    }

    #[test]
    fn not_configured_when_publisher_empty() {
        let sink = sink_with("", 0);
        assert_eq!(
            sink.publish(&anchor(0), AnchorPriority::Immediate),
            Err(AnchorError::NotConfigured)
        );
        assert_eq!(sink.resolve(&[0x11; 32]), Err(AnchorError::NotConfigured));
    }

    #[test]
    fn no_anchor_priority_is_noop() {
        let sink = sink_with("addr_pub", 0);
        assert_eq!(sink.publish(&anchor(0), AnchorPriority::NoAnchor), Ok(None));
    }

    #[test]
    fn publish_then_resolve_roundtrip() {
        let sink = sink_with("addr_pub", 0);
        let a = anchor(5);
        let receipt = sink
            .publish(&a, AnchorPriority::Immediate)
            .unwrap()
            .unwrap();
        assert_eq!(receipt.backend, AnchorBackend::Settlement);
        let got = sink.resolve(&a.ref_id).unwrap();
        assert_eq!(got, Some(a));
    }

    #[test]
    fn publish_idempotent_same_seq_noop() {
        let sink = sink_with("addr_pub", 0);
        let a = anchor(5);
        assert!(
            sink.publish(&a, AnchorPriority::Immediate)
                .unwrap()
                .is_some()
        );
        // Neo lại cùng seq → no-op (không thêm tx thứ 2).
        assert_eq!(sink.publish(&a, AnchorPriority::Immediate).unwrap(), None);
    }

    #[test]
    fn publish_rollback_lower_seq_rejected() {
        let sink = sink_with("addr_pub", 0);
        assert!(
            sink.publish(&anchor(5), AnchorPriority::Immediate)
                .unwrap()
                .is_some()
        );
        let err = sink
            .publish(&anchor(3), AnchorPriority::Immediate)
            .unwrap_err();
        assert_eq!(
            err,
            AnchorError::RollbackAttempt {
                on_chain_seq: 5,
                attempted: 3
            }
        );
    }

    #[test]
    fn resolve_ignores_foreign_wallet_tx() {
        // Tx mang label 1234 nhưng input KHÔNG phải publisher → bỏ qua.
        let store = std::rc::Rc::new(RefCell::new(MockQuery {
            publisher: "addr_pub".into(),
            ..Default::default()
        }));
        {
            let mut q = store.borrow_mut();
            let cbor = encode_records(&[SettlementRecord::Anchor(anchor(9))]);
            q.txs.push("tx_evil".into());
            q.inputs
                .insert("tx_evil".into(), vec!["addr_attacker".into()]);
            q.meta.insert("tx_evil".into(), cbor);
        }
        let submitter = MockSubmitter {
            publisher: "addr_pub".into(),
            store: store.clone(),
            fail_times: RefCell::new(0),
        };
        let cfg = SinkConfig {
            publisher_address: "addr_pub".into(),
            ..Default::default()
        };
        let sink = SettlementSink::new(cfg, store, submitter);
        assert_eq!(sink.resolve(&[0x11; 32]).unwrap(), None);
    }

    #[test]
    fn datum_too_large_rejected() {
        let store = std::rc::Rc::new(RefCell::new(MockQuery {
            publisher: "addr_pub".into(),
            ..Default::default()
        }));
        let submitter = MockSubmitter {
            publisher: "addr_pub".into(),
            store: store.clone(),
            fail_times: RefCell::new(0),
        };
        let cfg = SinkConfig {
            publisher_address: "addr_pub".into(),
            max_metadatum_bytes: 10, // cực nhỏ để ép vượt
            ..Default::default()
        };
        let sink = SettlementSink::new(cfg, store, submitter);
        let err = sink
            .publish(&anchor(0), AnchorPriority::Immediate)
            .unwrap_err();
        assert!(matches!(err, AnchorError::DatumTooLarge { .. }));
    }

    #[test]
    fn retry_only_on_network_then_succeeds() {
        let sink = sink_with("addr_pub", 2); // 2 lần Network rồi OK
        let mut slept = Vec::new();
        let r = publish_with_retry(&sink, &anchor(0), AnchorPriority::Immediate, 5, 10, |ms| {
            slept.push(ms)
        })
        .unwrap();
        assert!(r.is_some());
        assert_eq!(slept, vec![10, 20]); // backoff mũ 2 lần
    }
    // Hợp nhất AnchoredTable (resolve Settlement → verify_resolved dùng chung) — test
    // cần chain ký thật, đặt ở `tests/settlement.rs`.
}
