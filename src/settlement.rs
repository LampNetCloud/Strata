//! S1 backend **Settlement** — neo `StrataAnchor` qua **tx metadata label 1234**
//! (đối chiếu `settle.ts` LampNet). Backend MẶC ĐỊNH của S1 (anh Đức chốt PR #8):
//! payload CBOR **raw bytes** (KHÔNG JSON-hex, tiết kiệm ~50% byte). Mosaic CIP-68
//! (`anchor_sink.rs`) giữ cho hồ sơ giá trị cao.
//!
//! Module này là **lớp THUẦN** (no I/O): codec + logic sink generic theo hai seam
//! [`ChainQuery`] (đọc on-chain) và [`Submitter`] (build+submit tx). Cài đặt I/O thật
//! (Blockfrost + submitter đẩy lô sang cửa Mosaic) sống ở crate riêng
//! `lampnet-anchor-io` — giữ crate
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

use crate::anchor_sink::{
    AnchorBackend, AnchorError, AnchorPriority, AnchorReceipt, AnchorSink, WindowAnchor, WindowScan,
};
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
//   1..=64B. Decode từ chối mọi chunking khác → **canonical ở tầng CẤU TRÚC record**
//   (chunking + map đúng-2-entry + chống dup-key). LƯU Ý (không phải bijection toàn phần):
//   KHÔNG đảm bảo canonical CBOR nguyên thuỷ — int non-minimal / indefinite-length / rác
//   đuôi vẫn decode ra CÙNG record qua ciborium. Vô hại vì trust đến từ **pin
//   publisher-input**, KHÔNG từ băm bytes metadata (không consumer nào hash metadatum làm
//   định-danh). Nếu sau này cần bijection thật: thêm kiểm minimal-encoding + test-vector
//   "2 encoding tương đương → từ chối 1".
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
/// Hex thường (lowercase) — dựng `unit` beacon (`policy ++ ref_id`) không cần kéo crate
/// `hex` vào core no-I/O.
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

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
    /// Slot của block chứa `txid`; `None` nếu chưa thấy tx (chưa confirm).
    ///
    /// Mặc định **báo không hỗ trợ** thay vì trả `None`: `None` nghĩa là *"tx chưa
    /// lên chuỗi"*, còn *"query này không biết slot"* là một câu hoàn toàn khác — mà
    /// bên gọi (quét cửa sổ) sẽ đọc cả hai thành "bỏ qua tx này".
    fn tx_slot(&self, txid: &str) -> Result<Option<u64>, AnchorError> {
        let _ = txid;
        Err(AnchorError::Rejected(
            "tx_slot: ChainQuery này không hỗ trợ quét theo cửa sổ slot".into(),
        ))
    }

    /// Slot của block mới nhất. Bên gọi dùng nó để tự quyết cửa sổ đã đủ sâu để đóng
    /// chưa — lượt quét không tự quyết thay.
    fn tip_slot(&self) -> Result<u64, AnchorError> {
        Err(AnchorError::Rejected(
            "tip_slot: ChainQuery này không hỗ trợ quét theo cửa sổ slot".into(),
        ))
    }

    /// Tx hash MỚI NHẤT có đụng tới asset `unit` (`policyId` ++ `assetName`, hex);
    /// `None` nếu asset chưa từng tồn tại. Dùng cho `beacon_mode` — con-trỏ-latest theo
    /// asset thay vì quét cửa sổ địa chỉ (miễn nhiễm flood). Impl mặc định báo không hỗ
    /// trợ để các `ChainQuery` chỉ dùng đường legacy không phải cài lại.
    fn asset_latest_tx(&self, unit: &str) -> Result<Option<String>, AnchorError> {
        let _ = unit;
        Err(AnchorError::Rejected(
            "asset_latest_tx: ChainQuery này không hỗ trợ beacon_mode".into(),
        ))
    }
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
/// thật (`MosaicDoorSubmitter` — đẩy lô sang cửa Mosaic) ở crate `lampnet-anchor-io`.
pub trait Submitter {
    /// Submit tx với metadatum label 1234 = các record đã cho. Trả txid + địa chỉ ví ký.
    fn submit(&self, records: &[SettlementRecord]) -> Result<SubmitOutcome, AnchorError>;

    /// Địa chỉ ví sẽ ký, **nếu biết được TRƯỚC khi submit**. `None` = không biết trước.
    ///
    /// Vì sao là `Option` chứ không bắt buộc: hai loại submitter có bản chất khác nhau.
    /// Loại tự giữ khoá (mock, ví cục bộ) biết ví của mình ngay lúc dựng. Loại đẩy lô sang
    /// một dịch vụ ngoài — `MosaicDoorSubmitter` — thì **không thể** biết: ví nằm ở phía
    /// cửa Mosaic, và địa chỉ chỉ xuất hiện trong phản hồi, tức là sau khi tx đã đi.
    /// Bắt trait trả `&str` sẽ ép loại thứ hai bịa một giá trị, và một giá trị bịa ở đúng
    /// chỗ đối chiếu danh tính thì tệ hơn hẳn một `None` trung thực.
    ///
    /// `None` ⇒ [`SettlementSink::publish_batch`] GIỮ NGUYÊN phép kiểm hậu-submit làm lưới
    /// cuối. `Some(addr)` khác publisher đã pin ⇒ fail **trước** khi tốn phí.
    fn publisher_address(&self) -> Option<&str> {
        None
    }
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
    /// Trần số tx quét khi `resolve` (ví publisher dùng chung có nhiều tx). Chỉ dùng ở
    /// đường legacy (`beacon_policy = None`).
    pub resolve_scan_limit: usize,
    /// BẬT `beacon_mode` (opt-in, mặc định `None` = legacy address-scan).
    ///
    /// `Some(policy_id_hex)` = `resolve` xác định latest theo **asset** thay vì quét cửa
    /// sổ địa chỉ: beacon NFT `unit = policy_id ++ ref_id` (native minting policy
    /// `sig(publisher)`) đi tới trên UTxO anchor mới nhất. Kẻ lạ không mint/di chuyển được
    /// beacon ⇒ flood tx-gửi-tới-publisher KHÔNG làm mù `resolve` (issue #14). `policy_id`
    /// = 28 byte = 56 hex.
    pub beacon_policy: Option<String>,
}

impl Default for SinkConfig {
    fn default() -> Self {
        Self {
            publisher_address: String::new(),
            label: METADATA_LABEL,
            max_metadatum_bytes: 8 * 1024,
            resolve_scan_limit: 500,
            beacon_policy: None,
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
    ///
    /// **Khử trùng TRONG lô** (issue #41 mục 4): hai anchor cùng `ref_id` trong CÙNG một lô
    /// bị từ chối cứng ([`AnchorError::DuplicateRefIdInBatch`]) trước cả lượt đọc on-chain.
    /// Phép so `resolve_many` chỉ so lô với **chuỗi**, không so lô với **chính nó** — nên
    /// hai phần tử trùng `ref_id` (khác `seq`) đều rơi vào nhánh "chưa có on-chain" và cùng
    /// lên MỘT tx. Đó là hai `seq` cùng lúc cho một lineage, không sửa lại được sau khi tx
    /// đã lên chuỗi.
    pub fn publish_batch(
        &self,
        anchors: &[StrataAnchor],
    ) -> Result<Option<AnchorReceipt>, AnchorError> {
        self.ensure_configured()?;
        // Chặn TRƯỚC lượt đọc on-chain: lô hỏng thì không tốn cả lượt quét cửa sổ.
        // O(n²) là cố ý — lô thực tế cỡ vài chục phần tử, và giữ được thứ tự báo lỗi
        // (báo đúng phần tử TRÙNG đầu tiên theo thứ tự người gọi xếp).
        for (i, a) in anchors.iter().enumerate() {
            if anchors[..i].iter().any(|b| b.ref_id == a.ref_id) {
                return Err(AnchorError::DuplicateRefIdInBatch { ref_id: a.ref_id });
            }
        }
        let ref_ids: Vec<Hash32> = anchors.iter().map(|a| a.ref_id).collect();
        let on_chain = self.resolve_many(&ref_ids)?;
        let mut fresh: Vec<SettlementRecord> = Vec::new();
        for a in anchors {
            match on_chain.iter().find(|c| c.ref_id == a.ref_id) {
                Some(c) if c.seq > a.seq => {
                    return Err(AnchorError::RollbackAttempt {
                        on_chain_seq: c.seq,
                        attempted: a.seq,
                    });
                }
                Some(c) if c.seq == a.seq => {
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
        // ── Lưới THỨ NHẤT: hỏi ví TRƯỚC khi submit (issue #41 mục 5) ────────────────
        // Submitter nào biết trước địa chỉ ví ký thì phải nói ra ở đây. Phát hiện sai ví
        // sau khi submit là phát hiện muộn theo nghĩa đắt nhất: tx đã lên chuỗi, phí đã
        // mất, và anchor đó sẽ bị chính `resolve()` bỏ qua (luật tin cậy §4.3 lọc theo
        // input publisher) ⇒ tiền đi mà không neo được gì.
        if let Some(addr) = self.submitter.publisher_address()
            && addr != self.cfg.publisher_address
        {
            return Err(AnchorError::Rejected(format!(
                "ví submitter ({}) != publisher pin trong config ({}) — chặn TRƯỚC khi submit",
                addr, self.cfg.publisher_address
            )));
        }
        let outcome = self.submitter.submit(&fresh)?;
        // ── Lưới THỨ HAI: giữ nguyên, KHÔNG gỡ ─────────────────────────────────────
        // Lưới trên chỉ bắt được submitter *biết trước* ví mình. `MosaicDoorSubmitter`
        // không biết — địa chỉ chỉ có trong phản hồi của cửa Mosaic. Với nó, đây vẫn là
        // chỗ duy nhất bắt được, và bắt muộn còn hơn không bắt.
        //
        // PHẠM VI PHẢI NÓI THẲNG, vì hai lưới cộng lại nghe như một bảo đảm rộng hơn thứ
        // chúng cấp: trên đường sản xuất duy nhất hôm nay (`MosaicDoorSubmitter`), lưới
        // THỨ NHẤT **không chạy** (`publisher_address()` trả `None` có chủ ý), nên chỉ còn
        // lưới này — và nó so với `outcome.address`, tức một giá trị **do chính cửa tự
        // khai** trong phản hồi JSON. Nghĩa là cặp lưới này phát hiện **cấu hình sai**
        // (pin nhầm ví, cửa trỏ nhầm môi trường), KHÔNG phát hiện **một cửa nói dối**:
        // cửa bị chiếm chỉ cần trả đúng chuỗi địa chỉ đã pin trong khi ký bằng ví khác là
        // qua cả hai. Ca đó chỉ lộ ra ở `resolve()` — nơi lọc theo input thật trên chuỗi —
        // và lúc đó phí đã mất, còn gương `anchored` của daemon thì đã tiến.
        // Bịt thật cần một nguồn địa chỉ độc lập với cửa (đọc input tx từ chain-index sau
        // khi có `txid`); đó là việc riêng, không nằm trong bản vá này.
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

    /// `resolve` cho **nhiều** `ref_id` trong MỘT lượt quét.
    ///
    /// Vì sao không lặp `resolve()`: ở chế độ legacy, mỗi `resolve()` quét **cùng một**
    /// cửa sổ tx của **cùng một** ví publisher và đọc **cùng những** metadatum ấy — chỉ
    /// khác mỗi cái `ref_id` đem so. Lặp N lần là làm lại N lần đúng một việc, tức
    /// `N × resolve_scan_limit` lượt gọi mạng.
    ///
    /// Đo thật trên Preprod (2026-08-15, `scan_limit = 500`): lô **3** ref chạy xong
    /// trong vài phút; lô **10** ref **vượt 180 giây** timeout của client — daemon vẫn
    /// hoàn tất và tx vẫn lên chuỗi (`6cc6ab6e…`), nhưng bên gọi đã bỏ cuộc và **không
    /// còn biết txid của lô mình vừa bắn**. Đó là hỏng đúng chỗ đau: lô lên chuỗi mà
    /// bên quyết lô coi như thất bại, rồi bắn lại.
    ///
    /// Quét một lượt, gộp cho cả tập ⇒ chi phí mạng thành **hàm của cửa sổ quét**, không
    /// còn là hàm của kích thước lô. Đúng tính chất mà đường lô sinh ra để có.
    ///
    /// Chế độ beacon vốn đã O(1) theo từng ref (tra asset-index, không quét), nên ở đó
    /// lặp là đúng — và đó cũng là lý do beacon **không phải đồ trang trí**.
    pub fn resolve_many(&self, ref_ids: &[Hash32]) -> Result<Vec<StrataAnchor>, AnchorError> {
        self.ensure_configured()?;
        if ref_ids.is_empty() {
            return Ok(Vec::new());
        }
        match &self.cfg.beacon_policy {
            Some(policy) => {
                let mut out = Vec::new();
                for r in ref_ids {
                    if let Some(a) = self.resolve_via_beacon(r, policy)? {
                        out.push(a);
                    }
                }
                Ok(out)
            }
            None => self.resolve_many_via_address_scan(ref_ids),
        }
    }

    /// Một lượt quét cửa sổ, gộp `best` cho MỌI `ref_id` được hỏi.
    fn resolve_many_via_address_scan(
        &self,
        ref_ids: &[Hash32],
    ) -> Result<Vec<StrataAnchor>, AnchorError> {
        let txs = self
            .query
            .address_txs(&self.cfg.publisher_address, self.cfg.resolve_scan_limit)?;
        let mut best: Vec<Option<StrataAnchor>> = vec![None; ref_ids.len()];
        for txid in txs {
            let Some(cbor) = self.query.tx_metadata_cbor(&txid, self.cfg.label)? else {
                continue;
            };
            // TRUST: chỉ tin tx do publisher CHI. Kiểm SAU khi biết tx có metadata —
            // tx không mang label 1234 thì không cần tốn thêm một lượt gọi nào.
            let inputs = self.query.tx_input_addresses(&txid)?;
            if !inputs.iter().any(|a| a == &self.cfg.publisher_address) {
                continue;
            }
            // Decode MỘT lần cho cả tập ref_id, thay vì decode lại theo từng ref.
            for rec in decode_records_lenient(&cbor) {
                let SettlementRecord::Anchor(a) = rec else {
                    continue;
                };
                if let Some(i) = ref_ids.iter().position(|r| *r == a.ref_id)
                    && best[i].as_ref().is_none_or(|b| a.seq > b.seq)
                {
                    best[i] = Some(a);
                }
            }
        }
        Ok(best.into_iter().flatten().collect())
    }

    /// LEGACY (`beacon_policy = None`): quét cửa sổ hữu hạn tx của ví publisher (MỚI→CŨ)
    /// rồi lọc `input == publisher`. ĐIỂM YẾU issue #14: kẻ tấn công flood tx-gửi-tới-
    /// publisher đẩy anchor thật ra ngoài cửa sổ → trả `None`. An toàn cho publisher
    /// 1-ref_id / reader tin daemon; reader bên thứ ba cần chống-flood dùng `beacon_mode`.
    fn resolve_via_address_scan(
        &self,
        ref_id: &Hash32,
    ) -> Result<Option<StrataAnchor>, AnchorError> {
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
            best = Self::fold_best_anchor(best, &cbor, ref_id);
        }
        Ok(best)
    }

    /// BEACON (`beacon_policy = Some`): xác định latest theo ASSET, không quét cửa sổ.
    /// Beacon NFT `unit = policy ++ ref_id` chỉ publisher mint/di chuyển được (native
    /// policy `sig(publisher)`) ⇒ lịch sử asset toàn do publisher chi ⇒ flood tx-gửi-tới-
    /// publisher KHÔNG chạm beacon (issue #14). O(1) theo tx mới nhất của asset.
    fn resolve_via_beacon(
        &self,
        ref_id: &Hash32,
        policy: &str,
    ) -> Result<Option<StrataAnchor>, AnchorError> {
        // assetName = ref_id (32B ≤ giới hạn 32B của Cardano) → hex 64 ký tự.
        let unit = format!("{policy}{}", hex_lower(ref_id));
        let Some(txid) = self.query.asset_latest_tx(&unit)? else {
            return Ok(None); // beacon chưa từng tồn tại ⇒ ref_id chưa neo
        };
        // Defense-in-depth: beacon chỉ di chuyển được bởi khoá chi của publisher. Đối
        // chiếu input; lệch = bất thường (khoá lộ / indexer sai) → fail-closed, KHÔNG
        // nhầm với "chưa neo".
        let inputs = self.query.tx_input_addresses(&txid)?;
        if !inputs.iter().any(|a| a == &self.cfg.publisher_address) {
            return Err(AnchorError::Rejected(format!(
                "beacon {unit}: tx mới nhất {txid} không do publisher chi — asset-index bất nhất"
            )));
        }
        // ── Từ đây trở xuống, `Ok(None)` là một phát biểu KHÔNG có bằng chứng đỡ.
        //
        // `Ok(None)` ở đường này được đọc là **"ref_id chưa neo bao giờ"** — xem nhánh
        // `:642`, và xem người gọi: `publish_batch` dùng chính kết quả này làm mốc so
        // `seq`, nên `None` = "không có mốc" = **bỏ qua gác chống tụt lùi INV-E7**.
        //
        // Nhưng tới được dòng này nghĩa là beacon **đang tồn tại trên chuỗi** và tx mới
        // nhất đụng nó **do publisher chi** (hai phép kiểm ngay trên). Hai điều kiện đó
        // không chứng minh "đã neo" — beacon dùng native policy `sig(publisher)` nên
        // chuỗi không ép mint phải đi kèm anchor; bất biến ấy là bất biến của công cụ
        // publisher (`anchor-io/src/mosaic_door.rs:22` từ chối mint beacon trỏ vào anchor
        // không có trong lô), không phải của validator. Nhưng chúng thừa sức bác `None`:
        // thứ ta đang cầm là **"không đọc được"**, không phải **"chưa từng có"**.
        //
        // Đây đúng là ca ba-trạng-thái: có · không có · KHÔNG ĐO ĐƯỢC. Trộn trạng thái
        // thứ ba vào trạng thái thứ hai làm phép đo trả về một giá trị hợp lệ đúng lúc
        // nó không đo được gì — và ở đây giá trị ấy mở đúng cái cổng mà INV-E7 đóng.
        // Nên không nhánh nào dưới đây được phép trả `Ok(None)`.
        let unreadable = match self.query.tx_metadata_cbor(&txid, self.cfg.label)? {
            None => format!(
                "tx mới nhất {txid} KHÔNG mang metadata label {}",
                self.cfg.label
            ),
            // Cùng một lớp lỗi, thấp hơn đúng một dòng: tx CÓ label nhưng không chứa
            // record nào cho ref đang hỏi (beacon đi kèm một lô anchor của ref khác).
            // `fold_best_anchor` trả `None` ở cả hai nghĩa, nên chỗ phân biệt phải ở đây.
            Some(cbor) => match Self::fold_best_anchor(None, &cbor, ref_id) {
                Some(a) => return Ok(Some(a)),
                None => format!(
                    "tx mới nhất {txid} có metadata label {} nhưng KHÔNG chứa record anchor \
                     nào cho ref này",
                    self.cfg.label
                ),
            },
        };
        self.beacon_unreadable_fallback(ref_id, &unit, &unreadable)
    }

    /// Beacon tồn tại nhưng tx nó đang nằm trong KHÔNG đọc ra anchor của `ref_id`.
    ///
    /// # Vì sao chỗ này KHÔNG được dừng ở `Err`
    ///
    /// Trạng thái "beacon bị cuốn đi" **không cần ai tấn công**: beacon là native asset
    /// nằm trong UTxO của ví publisher, nên coin-selection của chính ví đó — trả phí,
    /// gộp UTxO, hay một lô anchor của lineage KHÁC — cuốn nó theo là chuyện thường.
    ///
    /// Mà `publish_batch` lấy mốc `seq` qua đúng hàm này (`resolve_many` → `?`). Nên
    /// một `Err` cứng ở đây khoá luôn **đường ghi**: không neo lại được ref đó nữa, và
    /// vì `resolve_many` lặp bằng `?`, một ref kẹt kéo theo **cả lô** — kể cả các ref
    /// có beacon lành. "Đường về: neo lại ref này" tự nó đi qua cái cổng vừa đóng.
    ///
    /// # Bước lùi, và vì sao nó KHÔNG mở lại lỗ fail-open
    ///
    /// Lùi về `resolve_via_address_scan` — cùng `ChainQuery`, không thêm phương thức
    /// nào vào trait. Nó chỉ có thể trả hai thứ, và **không thứ nào là "chưa neo" sai**:
    ///
    /// - **thấy anchor** ⇒ đó là mốc THẬT, `publish_batch` gác INV-E7 như thường, và
    ///   lượt neo kế tiếp kéo beacon về một tx có mang anchor của ref ⇒ **tự lành**;
    /// - **không thấy gì** ⇒ vẫn `Err` như cũ. Flood của issue #14 chỉ đẩy được vào
    ///   nhánh này, tức flood không mua được `Ok(None)` — bảo đảm của beacon-mode giữ
    ///   nguyên.
    ///
    /// Và bước lùi không trả được anchor CŨ hơn sự thật: cửa sổ quét xếp theo độ mới,
    /// `seq` tăng theo thời gian neo (INV-E7), nên tx của `seq` cao luôn mới hơn tx của
    /// `seq` thấp — `seq` thấp lọt vào cửa sổ thì `seq` cao cũng lọt. Quét trả **đúng
    /// mốc mới nhất, hoặc không gì cả**.
    ///
    /// Giá: một lượt quét cửa sổ cho mỗi ref rơi vào nhánh này. Đó là giá của đường
    /// hỏng, không phải của đường thường — đường thường vẫn O(1) theo asset-index.
    fn beacon_unreadable_fallback(
        &self,
        ref_id: &Hash32,
        unit: &str,
        why: &str,
    ) -> Result<Option<StrataAnchor>, AnchorError> {
        if let Some(a) = self.resolve_via_address_scan(ref_id)? {
            return Ok(Some(a));
        }
        Err(AnchorError::Rejected(format!(
            "beacon {unit}: {why} — beacon đã bị cuốn theo một giao dịch khác của ví \
             publisher (coin-selection trả phí, gộp UTxO, hoặc một lô anchor của ref \
             khác). Đây là 'không đọc được', KHÔNG phải 'chưa neo'. Bước lùi quét cửa \
             sổ {} tx của publisher cũng KHÔNG thấy anchor nào cho ref này, nên mốc \
             `seq` không dựng được và INV-E7 không gác được — từ chối thay vì đoán. \
             Đường về: nới `resolve_scan_limit`, hoặc neo lại ref này bằng một lượt \
             publish có beacon lành.",
            self.cfg.resolve_scan_limit
        )))
    }

    /// Quét cửa sổ slot `[from_slot, to_slot)` và trả **mọi** anchor đã phát trong
    /// đó — nguồn lá của luồng checkpoint toàn cục.
    ///
    /// # Luật quét, và vì sao mỗi vế tồn tại
    ///
    /// - `address_txs` trả MỚI → CŨ. Tx có `slot >= to_slot` là **trên** cửa sổ ⇒
    ///   bỏ qua rồi **đi tiếp**; tx đầu tiên có `slot < from_slot` là **dưới** cửa sổ
    ///   ⇒ dừng, và chính nó là **bằng chứng đã phủ hết** cửa sổ.
    /// - Chỉ tin tx do publisher **CHI** — cùng luật với `resolve`, không phải một
    ///   luật thứ hai. Kiểm **sau** khi biết tx có label 1234, để tx không liên quan
    ///   không tốn thêm lượt gọi nào.
    /// - **Hết cửa sổ quét mà chưa chạm đáy ⇒ `Rejected`, KHÔNG trả danh sách
    ///   ngắn.** Đây là gác quan trọng nhất của cả hàm: `root` tính trên tập thiếu
    ///   vẫn là một `root` hợp lệ về hình thức, vẫn chốt lên chuỗi được, và chuỗi
    ///   `epoch` nhìn vẫn liên tục — không có gì bật ra để nói cam kết vừa ghi ít
    ///   hơn sự thật.
    ///
    /// Chi phí: `1 + n` lượt gọi cho `n` tx trong tầm quét (`address_txs` một lượt,
    /// rồi mỗi tx: slot + metadata + input). Đó là **giá của một chu kỳ**, không phải
    /// giá của một lô — nên nó rơi vào nhịp checkpoint, không vào đường neo.
    pub fn scan_window(&self, from_slot: u64, to_slot: u64) -> Result<WindowScan, AnchorError> {
        self.ensure_configured()?;
        if to_slot <= from_slot {
            return Err(AnchorError::Rejected(format!(
                "cửa sổ rỗng hoặc lùi: [{from_slot}, {to_slot})"
            )));
        }
        let tip_slot = self.query.tip_slot()?;
        let txs = self
            .query
            .address_txs(&self.cfg.publisher_address, self.cfg.resolve_scan_limit)?;
        let exhausted_history = txs.len() < self.cfg.resolve_scan_limit;

        let mut anchors: Vec<WindowAnchor> = Vec::new();
        let mut scanned = 0usize;
        let mut reached_below = false;
        for txid in &txs {
            scanned += 1;
            let Some(slot) = self.query.tx_slot(txid)? else {
                // Tx chưa confirm ⇒ chưa có slot ⇒ chưa thuộc cửa sổ nào. Không
                // phải lý do để dừng: nó nằm ở đầu danh sách (mới nhất).
                continue;
            };
            if slot < from_slot {
                reached_below = true;
                break;
            }
            if slot >= to_slot {
                continue;
            }
            let Some(cbor) = self.query.tx_metadata_cbor(txid, self.cfg.label)? else {
                continue;
            };
            let inputs = self.query.tx_input_addresses(txid)?;
            if !inputs.iter().any(|a| a == &self.cfg.publisher_address) {
                continue;
            }
            for rec in decode_records_lenient(&cbor) {
                if let SettlementRecord::Anchor(a) = rec {
                    anchors.push(WindowAnchor {
                        anchor: a,
                        slot,
                        txid: txid.clone(),
                    });
                }
            }
        }

        if !reached_below && !exhausted_history {
            return Err(AnchorError::Rejected(format!(
                "cửa sổ [{from_slot}, {to_slot}) CHƯA quét hết: hết trần {} tx mà chưa chạm tx \
                 nào dưới from_slot. Trả tập thiếu ở đây là chốt một `root` ít hơn sự thật mà \
                 không có gì bật ra — nới `resolve_scan_limit` hoặc thu hẹp cửa sổ.",
                self.cfg.resolve_scan_limit
            )));
        }

        Ok(WindowScan {
            from_slot,
            to_slot,
            tip_slot,
            scanned_txs: scanned,
            anchors,
        })
    }

    /// Gộp record CBOR của một tx vào `best` (anchor `seq` cao nhất khớp `ref_id`).
    fn fold_best_anchor(
        best: Option<StrataAnchor>,
        cbor: &[u8],
        ref_id: &Hash32,
    ) -> Option<StrataAnchor> {
        let mut best = best;
        for rec in decode_records_lenient(cbor) {
            if let SettlementRecord::Anchor(a) = rec
                && a.ref_id == *ref_id
                && best.as_ref().is_none_or(|b| a.seq > b.seq)
            {
                best = Some(a);
            }
        }
        best
    }
}

impl<Q: ChainQuery, S: Submitter> AnchorSink for SettlementSink<Q, S> {
    /// Ghi đè mặc định fail-closed: backend này **quét được** theo slot.
    fn scan_window(&self, from_slot: u64, to_slot: u64) -> Result<WindowScan, AnchorError> {
        SettlementSink::scan_window(self, from_slot, to_slot)
    }

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

    /// Ghi đè mặc định của trait: một lượt quét cho **cả lô** thay vì N lượt.
    fn resolve_many(&self, ref_ids: &[Hash32]) -> Result<Vec<StrataAnchor>, AnchorError> {
        SettlementSink::resolve_many(self, ref_ids)
    }

    fn resolve(&self, ref_id: &Hash32) -> Result<Option<StrataAnchor>, AnchorError> {
        self.ensure_configured()?;
        match &self.cfg.beacon_policy {
            Some(policy) => self.resolve_via_beacon(ref_id, policy),
            None => self.resolve_via_address_scan(ref_id),
        }
    }

    /// Settlement **là** backend gộp lô: `encode_records` gói N anchor vào một
    /// mảng CBOR, một tx. Đây là chỗ nối của `BatchCoordinator` phía Mosaic.
    fn publish_many(
        &self,
        anchors: &[StrataAnchor],
        priority: AnchorPriority,
    ) -> Result<Option<AnchorReceipt>, AnchorError> {
        if priority == AnchorPriority::NoAnchor {
            return Ok(None);
        }
        SettlementSink::publish_batch(self, anchors)
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
        /// Số lượt QUÉT cửa sổ địa chỉ. Đếm được thì mới khoá được tính chất
        /// "một lô = một lượt quét" — không đếm thì ai đó đổi `resolve_many` về
        /// vòng lặp `resolve()` và cả bộ kiểm vẫn xanh.
        scans: std::cell::Cell<usize>,
        /// txid → slot. Vắng mặt = tx chưa confirm.
        slots: HashMap<String, u64>,
        tip: u64,
    }
    impl ChainQuery for MockQuery {
        fn address_txs(&self, addr: &str, limit: usize) -> Result<Vec<String>, AnchorError> {
            self.scans.set(self.scans.get() + 1);
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
        fn tx_slot(&self, txid: &str) -> Result<Option<u64>, AnchorError> {
            Ok(self.slots.get(txid).copied())
        }
        fn tip_slot(&self) -> Result<u64, AnchorError> {
            Ok(self.tip)
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
        fn tx_slot(&self, txid: &str) -> Result<Option<u64>, AnchorError> {
            self.borrow().tx_slot(txid)
        }
        fn tip_slot(&self) -> Result<u64, AnchorError> {
            self.borrow().tip_slot()
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

    /// **Một lô = MỘT lượt quét**, dù lô có bao nhiêu ref.
    ///
    /// Đây không phải tối ưu cho vui: đo thật trên Preprod, bản lặp `resolve()` khiến
    /// lô 10 ref vượt 180 giây và client bỏ cuộc **sau khi** tx đã lên chuỗi — bên
    /// quyết lô mất txid của chính lô mình bắn. Bài kiểm này khoá lại tính chất đó.
    #[test]
    fn mot_lo_chi_quet_mot_luot_du_lo_bao_nhieu_ref() {
        let sink = sink_with("addr_pub", 0);
        let mut a2 = anchor(1);
        a2.ref_id = [0x22; 32];
        let mut a3 = anchor(1);
        a3.ref_id = [0x33; 32];
        let batch = [anchor(1), a2, a3];

        sink.query.borrow().scans.set(0);
        assert!(sink.publish_batch(&batch).unwrap().is_some());
        assert_eq!(
            sink.query.borrow().scans.get(),
            1,
            "3 ref phải dùng ĐÚNG 1 lượt quét — lặp resolve() cho từng ref là 3 lượt"
        );

        // Neo lại y nguyên: vẫn một lượt quét, và lần này ra no-op idempotent.
        sink.query.borrow().scans.set(0);
        assert_eq!(sink.publish_batch(&batch).unwrap(), None);
        assert_eq!(sink.query.borrow().scans.get(), 1);
    }

    /// Gộp quét KHÔNG được làm mất gác rollback: một anchor tụt-lùi-seq trong lô vẫn
    /// phải giết cả lô. (Bài khẳng định đứng cạnh bài đếm ở trên — nếu không, một bản
    /// "gộp" trả về rỗng cũng qua được bài đếm.)
    #[test]
    fn gop_quet_van_giu_gac_rollback_ca_lo() {
        let sink = sink_with("addr_pub", 0);
        let mut a2 = anchor(5);
        a2.ref_id = [0x22; 32];
        assert!(sink.publish_batch(&[anchor(5), a2]).unwrap().is_some());

        // Lô sau: ref thứ nhất tụt về seq 3 ⇒ cả lô bị từ chối, kể cả ref hợp lệ.
        let mut b2 = anchor(9);
        b2.ref_id = [0x22; 32];
        assert_eq!(
            sink.publish_batch(&[anchor(3), b2]).unwrap_err(),
            AnchorError::RollbackAttempt {
                on_chain_seq: 5,
                attempted: 3
            }
        );
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
    // ---- scan_window: nguồn lá của luồng checkpoint toàn cục ----

    fn anchor_of(ref_byte: u8, seq: u64) -> StrataAnchor {
        StrataAnchor {
            ref_id: [ref_byte; 32],
            head_version_hash: [ref_byte ^ 0xff; 32],
            mmr_root: [ref_byte.wrapping_add(1); 32],
            seq,
        }
    }

    /// Dựng một ví publisher với `txs` = `(txid, slot, do_publisher_chi, anchors)`,
    /// MỚI → CŨ theo đúng thứ tự truyền vào.
    fn sink_with_window(
        publisher: &str,
        txs: &[(&str, Option<u64>, bool, Vec<StrataAnchor>)],
        tip: u64,
        scan_limit: usize,
    ) -> SettlementSink<std::rc::Rc<RefCell<MockQuery>>, MockSubmitter> {
        let store = std::rc::Rc::new(RefCell::new(MockQuery {
            publisher: publisher.to_string(),
            tip,
            ..Default::default()
        }));
        {
            let mut q = store.borrow_mut();
            for (txid, slot, mine, anchors) in txs {
                q.txs.push((*txid).to_string());
                if let Some(sl) = slot {
                    q.slots.insert((*txid).to_string(), *sl);
                }
                q.inputs.insert(
                    (*txid).to_string(),
                    vec![if *mine {
                        publisher.to_string()
                    } else {
                        "addr_test1_ke_la".to_string()
                    }],
                );
                let recs: Vec<SettlementRecord> = anchors
                    .iter()
                    .cloned()
                    .map(SettlementRecord::Anchor)
                    .collect();
                q.meta.insert((*txid).to_string(), encode_records(&recs));
            }
        }
        let submitter = MockSubmitter {
            publisher: publisher.to_string(),
            store: store.clone(),
            fail_times: RefCell::new(0),
        };
        let cfg = SinkConfig {
            publisher_address: publisher.to_string(),
            resolve_scan_limit: scan_limit,
            ..Default::default()
        };
        SettlementSink::new(cfg, store, submitter)
    }

    const PUB: &str = "addr_test1_publisher";

    #[test]
    fn scan_window_lay_dung_khoang_va_dung_bien() {
        // Biên: `from` ĐÓNG, `to` MỞ. Hai tx đúng ở hai biên là chỗ duy nhất phân
        // biệt được `[from, to)` với `(from, to]` — thiếu chúng thì mọi cửa sổ liền
        // kề đều hoặc bỏ sót hoặc đếm hai lần một anchor, mà root vẫn "hợp lệ".
        let sink = sink_with_window(
            PUB,
            &[
                ("tx_tren", Some(300), true, vec![anchor_of(0xaa, 9)]),
                ("tx_bien_tren", Some(200), true, vec![anchor_of(0xbb, 9)]),
                ("tx_trong", Some(150), true, vec![anchor_of(0xcc, 1)]),
                ("tx_bien_duoi", Some(100), true, vec![anchor_of(0xdd, 1)]),
                ("tx_duoi", Some(99), true, vec![anchor_of(0xee, 1)]),
            ],
            400,
            50,
        );
        let w = sink.scan_window(100, 200).unwrap();
        let refs: Vec<u8> = w.anchors.iter().map(|a| a.anchor.ref_id[0]).collect();
        assert_eq!(refs, vec![0xcc, 0xdd], "from ĐÓNG, to MỞ");
        assert_eq!(w.tip_slot, 400);
    }

    #[test]
    fn scan_window_bo_tx_khong_do_publisher_chi() {
        // Cùng luật tin cậy với `resolve`: kẻ lạ phát label 1234 tới ví publisher
        // thì record của nó KHÔNG được vào tập lá — nếu vào, người ngoài ghi thêm
        // được vào cam kết của ta mà chỉ tốn phí một tx.
        let sink = sink_with_window(
            PUB,
            &[
                ("tx_ke_la", Some(150), false, vec![anchor_of(0xaa, 1)]),
                ("tx_cua_ta", Some(140), true, vec![anchor_of(0xbb, 1)]),
                ("tx_day", Some(50), true, vec![]),
            ],
            400,
            50,
        );
        let w = sink.scan_window(100, 200).unwrap();
        assert_eq!(w.anchors.len(), 1);
        assert_eq!(w.anchors[0].anchor.ref_id[0], 0xbb);
    }

    /// Gác quan trọng nhất: hết trần quét mà chưa chạm tx nào **dưới** `from_slot`
    /// ⇒ **lỗi**. Trả tập thiếu ở đây là chốt một `root` ít hơn sự thật, mà chuỗi
    /// epoch nhìn vẫn liên tục và cửa sổ vẫn khít — không gì bật ra.
    #[test]
    fn quet_khong_phu_het_cua_so_phai_la_loi_khong_phai_danh_sach_ngan() {
        let sink = sink_with_window(
            PUB,
            &[
                ("t1", Some(190), true, vec![anchor_of(0xaa, 1)]),
                ("t2", Some(180), true, vec![anchor_of(0xbb, 1)]),
                ("t3", Some(170), true, vec![anchor_of(0xcc, 1)]),
            ],
            400,
            3, // trần = đúng số tx ⇒ không chứng minh được đã tới đáy
        );
        let e = sink.scan_window(100, 200).unwrap_err();
        match e {
            AnchorError::Rejected(m) => {
                assert!(m.contains("CHƯA quét hết"), "{m}");
                assert!(m.contains("scan_limit") || m.contains("trần"), "{m}");
            }
            other => panic!("phải là Rejected, gặp {other:?}"),
        }
    }

    /// Lịch sử ngắn hơn trần quét ⇒ đã nhìn hết đời ví ⇒ cửa sổ phủ hết, kể cả khi
    /// không có tx nào nằm dưới `from_slot`. Không có vế này thì chu kỳ ĐẦU TIÊN —
    /// lúc ví chưa có gì cũ hơn cửa sổ — không bao giờ đóng được.
    #[test]
    fn lich_su_ngan_hon_tran_thi_van_phu_het() {
        let sink = sink_with_window(
            PUB,
            &[("t1", Some(150), true, vec![anchor_of(0xaa, 1)])],
            400,
            50,
        );
        let w = sink.scan_window(100, 200).unwrap();
        assert_eq!(w.anchors.len(), 1);
    }

    /// Tx chưa confirm (chưa có slot) nằm ở đầu danh sách: **bỏ qua rồi đi tiếp**,
    /// không được coi là đã chạm đáy — nếu dừng ở đó thì một tx đang chờ xác nhận
    /// che khuất toàn bộ cửa sổ bên dưới.
    #[test]
    fn tx_chua_confirm_khong_lam_dung_lat_quet() {
        let sink = sink_with_window(
            PUB,
            &[
                ("tx_pending", None, true, vec![anchor_of(0xaa, 1)]),
                ("t1", Some(150), true, vec![anchor_of(0xbb, 1)]),
                ("t_day", Some(50), true, vec![]),
            ],
            400,
            50,
        );
        let w = sink.scan_window(100, 200).unwrap();
        assert_eq!(w.anchors.len(), 1);
        assert_eq!(w.anchors[0].anchor.ref_id[0], 0xbb);
    }

    #[test]
    fn cua_so_rong_hoac_lui_bi_tu_choi() {
        let sink = sink_with_window(PUB, &[], 400, 50);
        assert!(sink.scan_window(200, 200).is_err());
        assert!(sink.scan_window(200, 100).is_err());
    }

    /// Backend không quét được slot phải **nói ra**. Rỗng và "không đọc được" là hai
    /// câu khác nhau, mà bên tính `root` coi rỗng là một chu kỳ hợp lệ.
    #[test]
    fn backend_khong_ho_tro_thi_fail_closed_chu_khong_tra_rong() {
        struct KhongBietSlot;
        impl ChainQuery for KhongBietSlot {
            fn address_txs(&self, _: &str, _: usize) -> Result<Vec<String>, AnchorError> {
                Ok(Vec::new())
            }
            fn tx_input_addresses(&self, _: &str) -> Result<Vec<String>, AnchorError> {
                Ok(Vec::new())
            }
            fn tx_metadata_cbor(&self, _: &str, _: u64) -> Result<Option<Vec<u8>>, AnchorError> {
                Ok(None)
            }
        }
        struct KhongSubmit;
        impl Submitter for KhongSubmit {
            fn submit(&self, _: &[SettlementRecord]) -> Result<SubmitOutcome, AnchorError> {
                unreachable!()
            }
        }
        let cfg = SinkConfig {
            publisher_address: PUB.to_string(),
            ..Default::default()
        };
        let sink = SettlementSink::new(cfg, KhongBietSlot, KhongSubmit);
        assert!(matches!(
            sink.scan_window(100, 200),
            Err(AnchorError::Rejected(_))
        ));
    }

    /// Hai tx cùng chở một anchor (thử lại) ⇒ lượt quét trả **cả hai**. Khử trùng là
    /// luật của bên tính `root`, không phải của bên đọc — trộn hai việc thì bên đọc
    /// tự quyết cái mà cam kết on-chain phụ thuộc vào.
    #[test]
    fn khong_khu_trung_o_tang_doc() {
        let sink = sink_with_window(
            PUB,
            &[
                ("t1", Some(150), true, vec![anchor_of(0xaa, 3)]),
                ("t2", Some(140), true, vec![anchor_of(0xaa, 3)]),
                ("t_day", Some(50), true, vec![]),
            ],
            400,
            50,
        );
        let w = sink.scan_window(100, 200).unwrap();
        assert_eq!(w.anchors.len(), 2);
        assert_eq!(w.scanned_txs, 3);
    }

    // ---- issue #41 mục 4+5: lô tự mâu thuẫn, và ví sai phải chặn TRƯỚC khi tốn phí ----

    /// Submitter **đếm số lần bị gọi**, và khai (hoặc cố tình không khai) trước ví của mình.
    ///
    /// Đếm là phần quan trọng nhất của mock này: cả hai bản vá đều nói về chỗ *"lỗi phải
    /// bật ra TRƯỚC khi tx đi"*. Chỉ khẳng định "trả về `Err`" thì một bản vá đặt phép kiểm
    /// SAU `submit` vẫn xanh — mà đó đúng là lỗi đang sửa.
    struct CountingSubmitter {
        /// Ví THẬT sẽ ký; cũng là thứ `submit` trả về ở `SubmitOutcome.address`.
        wallet: String,
        /// `true` = submitter biết trước ví mình (khai qua `publisher_address`).
        knows_wallet_upfront: bool,
        calls: std::rc::Rc<std::cell::Cell<usize>>,
    }
    impl Submitter for CountingSubmitter {
        fn submit(&self, _: &[SettlementRecord]) -> Result<SubmitOutcome, AnchorError> {
            self.calls.set(self.calls.get() + 1);
            Ok(SubmitOutcome {
                txid: "tx_da_ton_phi".into(),
                address: self.wallet.clone(),
            })
        }
        fn publisher_address(&self) -> Option<&str> {
            if self.knows_wallet_upfront {
                Some(self.wallet.as_str())
            } else {
                None
            }
        }
    }

    /// Sink với ví publisher đã pin = [`PUB`], chuỗi TRỐNG (chưa neo gì), submitter đếm được.
    /// Trả thêm bộ đếm `submit` và store để đếm lượt quét on-chain.
    #[allow(clippy::type_complexity)]
    fn counting_sink(
        wallet: &str,
        knows_wallet_upfront: bool,
    ) -> (
        SettlementSink<std::rc::Rc<RefCell<MockQuery>>, CountingSubmitter>,
        std::rc::Rc<std::cell::Cell<usize>>,
        std::rc::Rc<RefCell<MockQuery>>,
    ) {
        let store = std::rc::Rc::new(RefCell::new(MockQuery {
            publisher: PUB.to_string(),
            ..Default::default()
        }));
        let calls = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let submitter = CountingSubmitter {
            wallet: wallet.to_string(),
            knows_wallet_upfront,
            calls: calls.clone(),
        };
        let cfg = SinkConfig {
            publisher_address: PUB.to_string(),
            ..Default::default()
        };
        (
            SettlementSink::new(cfg, store.clone(), submitter),
            calls,
            store,
        )
    }

    /// Hai anchor cùng `ref_id` trong CÙNG một lô ⇒ từ chối cứng, và **không có tx nào**.
    ///
    /// `resolve_many` chỉ so lô với chuỗi, không so lô với chính nó — nên trước bản vá cả
    /// hai phần tử đều rơi vào nhánh "chưa có on-chain" và cùng lên một tx. Kết quả là hai
    /// `seq` cùng lúc cho một lineage, và `resolve()` chọn cái nào là do thứ tự record
    /// trong metadatum quyết định. Không sửa lại được sau khi tx đã lên chuỗi.
    #[test]
    fn batch_with_two_anchors_for_same_ref_id_is_rejected_without_tx() {
        let (sink, calls, _store) = counting_sink(PUB, true);
        let err = sink
            .publish_batch(&[
                anchor_of(0xaa, 1),
                anchor_of(0xbb, 1),
                anchor_of(0xaa, 2), // trùng ref_id với phần tử đầu, khác seq
            ])
            .unwrap_err();
        assert_eq!(
            err,
            AnchorError::DuplicateRefIdInBatch { ref_id: [0xaa; 32] },
            "phải là biến thể RIÊNG, không nhét vào Rejected — bên gọi cần phân biệt \
             lỗi dựng lô với lỗi cửa"
        );
        assert_eq!(calls.get(), 0, "lô hỏng mà vẫn submit = đã tốn phí");
        assert!(!err.is_retryable(), "bắn lại đúng lô ấy vẫn hỏng y hệt");
    }

    /// Trùng `ref_id` bị bắt **trước cả lượt đọc on-chain** — lô đã hỏng thì không được
    /// tốn một lượt quét cửa sổ nào (lượt quét là thứ đắt nhất của đường này).
    #[test]
    fn duplicate_ref_id_rejected_before_any_on_chain_scan() {
        let (sink, calls, store) = counting_sink(PUB, true);
        let err = sink
            .publish_batch(&[anchor_of(0xcc, 1), anchor_of(0xcc, 1)])
            .unwrap_err();
        assert_eq!(
            err,
            AnchorError::DuplicateRefIdInBatch { ref_id: [0xcc; 32] }
        );
        assert_eq!(store.borrow().scans.get(), 0, "chưa được quét lượt nào");
        assert_eq!(calls.get(), 0);
    }

    /// Ví sai + submitter BIẾT TRƯỚC ⇒ chặn trước `submit`, không tx nào ra đời.
    #[test]
    fn wrong_wallet_known_upfront_is_blocked_before_submit() {
        let (sink, calls, _store) = counting_sink("addr_test1_vi_khac", true);
        let err = sink.publish_batch(&[anchor_of(0xaa, 1)]).unwrap_err();
        match &err {
            AnchorError::Rejected(m) => {
                assert!(m.contains("addr_test1_vi_khac"), "{m}");
                assert!(m.contains("TRƯỚC khi submit"), "{m}");
            }
            other => panic!("phải là Rejected, gặp {other:?}"),
        }
        assert_eq!(
            calls.get(),
            0,
            "đây là toàn bộ nội dung bản vá: phí không được mất trước khi biết ví sai"
        );
    }

    /// Ví sai + submitter KHÔNG biết trước ⇒ lưới thứ hai (hậu-submit) vẫn phải bắt.
    ///
    /// Đây là đường của `MosaicDoorSubmitter` thật: địa chỉ chỉ có trong phản hồi của cửa.
    /// Bản vá THÊM lưới trước, không DỜI lưới — gỡ lưới sau là mở lại đúng lỗ vừa bịt, cho
    /// đúng backend duy nhất đang chạy thật.
    #[test]
    fn wrong_wallet_unknown_upfront_still_caught_by_second_net() {
        let (sink, calls, _store) = counting_sink("addr_test1_vi_khac", false);
        let err = sink.publish_batch(&[anchor_of(0xaa, 1)]).unwrap_err();
        match &err {
            AnchorError::Rejected(m) => {
                assert!(m.contains("addr_test1_vi_khac"), "{m}");
                assert!(
                    !m.contains("TRƯỚC khi submit"),
                    "ca này bắt ở lưới SAU; thông điệp không được nói dối là bắt trước: {m}"
                );
            }
            other => panic!("phải là Rejected, gặp {other:?}"),
        }
        assert_eq!(
            calls.get(),
            1,
            "lưới sau chỉ chạy được sau đúng một lượt submit"
        );
    }

    /// Ví đúng ⇒ lô đi qua cả hai lưới. Không có ca này thì một bản vá chặn-tất-cả cũng xanh.
    #[test]
    fn correct_wallet_passes_both_nets() {
        let (sink, calls, _store) = counting_sink(PUB, true);
        let r = sink.publish_batch(&[anchor_of(0xaa, 1)]).unwrap();
        assert_eq!(r.map(|x| x.txid), Some("tx_da_ton_phi".to_string()));
        assert_eq!(calls.get(), 1);
    }

    // Hợp nhất AnchoredTable (resolve Settlement → verify_resolved dùng chung) — test
    // cần chain ký thật, đặt ở `tests/settlement.rs`.
}
