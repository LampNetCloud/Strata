//! S1 — `AnchorSink` adapter: `Strata.anchor → Mosaic` (CIP-68). Issue #1 [P0].
//!
//! Map `StrataAnchor` (4 trường, 104 byte — `chain.rs`, `_CONTRACT.md` phương án A)
//! → datum CIP-68 `Constr 0 [metadata, version, extra]` (~180–200B, `Strata-API §8.1a`),
//! `resolve()` verify ngược, + logic idempotency/rollback (§8.1b).
//!
//! **Ranh giới issue #1:** "Strata giữ logic chain; **Mosaic giữ tx; KHÔNG dựng tx neo
//! trong Strata**". Vì vậy seam ra Mosaic = trait [`MosaicBackend`] trao đổi **[`PlutusData`]**
//! (Strata map anchor↔datum; Mosaic/Lucid lo CBOR↔tx↔Preview). Codec CBOR ở đây chỉ để
//! **đặc tả byte + size-guard**; on-chain byte cuối do tầng Mosaic (Phase 2) sinh/kiểm.
//!
//! Verify ngược (§8.1c) cần `mmr_size` TẠI lúc neo → daemon giữ bảng
//! [`AnchoredTable`] `(seq, mmr_root, mmr_size)` (KHÔNG thêm vào core thuần).

use crate::chain::{StrataAnchor, StrataChain};
use crate::version::Hash32;

// ────────────────────────────────────────────────────────────────────────────
// §4.1 — trait + kiểu (daemon-side, ngoài core thuần)
// ────────────────────────────────────────────────────────────────────────────

/// Cadence đẩy neo — khớp Stamp anchor_priority (Stamp-Strata-Mapping §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorPriority {
    /// Đẩy mỗi version (Mosaic A, giá trị cao + finality).
    Immediate,
    /// Mốc/epoch.
    Milestone,
    /// Gom ngày (settlement metadata).
    BatchDaily,
    /// KHÔNG đẩy — sống ở tầng (a)/(b).
    NoAnchor,
}

/// Backend nào thực đẩy on-chain (chọn liên-nền-tảng — Aladin chốt sau).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorBackend {
    /// LampNet settlement, metadata label 1234.
    Settlement,
    /// VeData Mosaic, reference-UTxO CIP-68.
    Mosaic,
}

/// Biên nhận một lần neo thành công.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorReceipt {
    pub txid: String,
    pub backend: AnchorBackend,
    pub slot: Option<u64>,
}

/// Lỗi adapter (§8.1b — 7 biến thể phủ hết case biên).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorError {
    /// Backend chưa cấu hình (thiếu key/URL).
    NotConfigured,
    /// Backend/validator từ chối (VD `seq' != seq+1` on-chain).
    Rejected(String),
    /// Lỗi mạng/timeout — RETRYABLE (chỉ biến thể này được retry).
    Network(String),
    /// INV-E7: backend phát hiện anchor cũ hơn on-chain (fail cứng).
    RollbackAttempt { on_chain_seq: u64, attempted: u64 },
    /// **Backend Mosaic-A: nhảy bậc seq.** Validator Plutus đang chạy ép
    /// `datum_out.seq == datum_in.seq + 1` (`VeDataIO/Code: mosaic/aiken/lib/strata/anchor.ak:55-57`,
    /// test `seq_advances_rejects_skip`). Neo một `seq` cao hơn `on_chain_seq + 1` sẽ bị
    /// chuỗi từ chối, nhưng head local đã tiến ⇒ **mọi lần neo sau kẹt vĩnh viễn**.
    /// Vì vậy sink chặn TẠI CHỖ, trước khi dựng tx (anh Đức chốt hướng B ngày 2026-08-07:
    /// giữ luật on-chain, sửa tầng đẩy — neo đúng từng seq).
    ///
    /// `expected` = seq DUY NHẤT được phép neo tiếp theo; người gọi phải neo `expected`
    /// trước (fail cứng, KHÔNG retryable — retry cùng `attempted` vẫn hỏng y hệt).
    ///
    /// KHÔNG áp cho lần neo ĐẦU TIÊN của một lineage (`on_chain_seq == None`): validator
    /// không guard CREATE nên UTxO anchor đầu được mang `seq` bất kỳ — đó là đường hợp lệ
    /// để đưa chuỗi đã sống off-chain lên neo giữa chừng.
    SeqGap {
        /// `seq` on-chain hiện tại (`None` = lineage chưa neo lần nào — biến thể này
        /// hiện KHÔNG được dựng với `None`; trường giữ để sink tương lai chặn chặt hơn).
        on_chain_seq: Option<u64>,
        /// `seq` DUY NHẤT được phép neo tiếp theo.
        expected: u64,
        /// `seq` mà người gọi vừa thử neo.
        attempted: u64,
    },
    /// Datum vượt maxTxSize/protocol param (fail cứng).
    DatumTooLarge { bytes: usize },
    /// Backend UTxO (Mosaic A): min-ADA không đủ (fail cứng).
    InsufficientAda { need: u64, have: u64 },
}

impl AnchorError {
    /// Chỉ `Network(_)` là retryable (§8.1b — phân tầng retry).
    pub fn is_retryable(&self) -> bool {
        matches!(self, AnchorError::Network(_))
    }
}

impl std::fmt::Display for AnchorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for AnchorError {}

/// Adapter một-đường: nhận `StrataAnchor` (đã enforce INV-E7 ở core), đẩy on-chain.
/// Core KHÔNG biết Cardano; adapter sống ở daemon. Một trait, nhiều backend.
pub trait AnchorSink {
    /// Đẩy commitment. Trả `Ok(None)` nếu `priority == NoAnchor` HOẶC đã neo idempotent.
    fn publish(
        &self,
        anchor: &StrataAnchor,
        priority: AnchorPriority,
    ) -> Result<Option<AnchorReceipt>, AnchorError>;

    /// Đọc anchor mới nhất on-chain cho `ref_id`. `None` nếu chưa neo bao giờ.
    fn resolve(&self, ref_id: &Hash32) -> Result<Option<StrataAnchor>, AnchorError>;
}

// ────────────────────────────────────────────────────────────────────────────
// §8.1a — PlutusData tối thiểu + datum CIP-68
// ────────────────────────────────────────────────────────────────────────────

/// Metadata CIP-68 bắt buộc (tối thiểu để không đội phí / lộ loại — INV-E5).
pub const ANCHOR_NAME: &[u8] = b"LN-STRATA-ANCHOR";
/// CIP-68 datum version.
pub const ANCHOR_DATUM_VERSION: i128 = 1;

/// Subset PlutusData đủ cho datum anchor. Encode CBOR theo quy ước ledger Cardano
/// (`Constr` alt 0..6 → tag `121+alt`; danh sách trường non-empty = mảng indefinite).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlutusData {
    /// Constructor: (alternative, fields).
    Constr(u64, Vec<PlutusData>),
    /// Map (definite-length) — dùng cho `metadata`.
    Map(Vec<(PlutusData, PlutusData)>),
    /// Integer (Plutus không giới hạn u64; ta chỉ dùng `seq`/`version` ≥ 0).
    Int(i128),
    /// Byte string.
    Bytes(Vec<u8>),
}

/// Map anchor 4 trường → datum CIP-68 (thứ tự `extra` = canonical `StrataAnchor`).
pub fn map_anchor_to_datum(a: &StrataAnchor) -> PlutusData {
    let extra = PlutusData::Constr(
        0,
        vec![
            PlutusData::Bytes(a.ref_id.to_vec()),
            PlutusData::Bytes(a.head_version_hash.to_vec()),
            PlutusData::Bytes(a.mmr_root.to_vec()),
            PlutusData::Int(a.seq as i128),
        ],
    );
    let metadata = PlutusData::Map(vec![(
        PlutusData::Bytes(b"name".to_vec()),
        PlutusData::Bytes(ANCHOR_NAME.to_vec()),
    )]);
    PlutusData::Constr(
        0,
        vec![metadata, PlutusData::Int(ANCHOR_DATUM_VERSION), extra],
    )
}

/// Lỗi parse datum → anchor (đủ để biết vì sao datum không hợp lệ).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatumError {
    /// Cấu trúc không phải `Constr 0 [meta, version, extra]`.
    Shape(&'static str),
    /// `version` khác `ANCHOR_DATUM_VERSION`.
    BadVersion(i128),
    /// Trường bytes sai độ dài (kỳ vọng 32).
    BadHashLen { field: &'static str, len: usize },
    /// `seq` âm hoặc vượt `u64` (§8.1a — Plutus Int không giới hạn).
    SeqOutOfRange(i128),
}

fn as_hash32(d: &PlutusData, field: &'static str) -> Result<Hash32, DatumError> {
    match d {
        PlutusData::Bytes(b) if b.len() == 32 => {
            let mut h = [0u8; 32];
            h.copy_from_slice(b);
            Ok(h)
        }
        PlutusData::Bytes(b) => Err(DatumError::BadHashLen {
            field,
            len: b.len(),
        }),
        _ => Err(DatumError::Shape(field)),
    }
}

/// Parse datum CIP-68 → anchor 4 trường. Nghịch của [`map_anchor_to_datum`].
pub fn parse_datum_to_anchor(d: &PlutusData) -> Result<StrataAnchor, DatumError> {
    let top = match d {
        PlutusData::Constr(0, fields) if fields.len() == 3 => fields,
        _ => {
            return Err(DatumError::Shape(
                "top must be Constr 0 [meta, version, extra]",
            ));
        }
    };
    // fields[0] = metadata (không dùng để dựng anchor — chỉ CIP-68 bắt buộc có).
    match &top[1] {
        PlutusData::Int(v) if *v == ANCHOR_DATUM_VERSION => {}
        PlutusData::Int(v) => return Err(DatumError::BadVersion(*v)),
        _ => return Err(DatumError::Shape("version must be Int")),
    }
    let extra = match &top[2] {
        PlutusData::Constr(0, e) if e.len() == 4 => e,
        _ => return Err(DatumError::Shape("extra must be Constr 0 [4 fields]")),
    };
    let ref_id = as_hash32(&extra[0], "ref_id")?;
    let head_version_hash = as_hash32(&extra[1], "head_version_hash")?;
    let mmr_root = as_hash32(&extra[2], "mmr_root")?;
    let seq = match &extra[3] {
        PlutusData::Int(s) if *s >= 0 && *s <= u64::MAX as i128 => *s as u64,
        PlutusData::Int(s) => return Err(DatumError::SeqOutOfRange(*s)),
        _ => return Err(DatumError::Shape("seq must be Int")),
    };
    Ok(StrataAnchor {
        ref_id,
        head_version_hash,
        mmr_root,
        seq,
    })
}

// ────────────────────────────────────────────────────────────────────────────
// CBOR (ledger Cardano `Data`) — đặc tả byte + size-guard. Round-trip nội bộ.
// ────────────────────────────────────────────────────────────────────────────

impl PlutusData {
    /// Encode CBOR theo quy ước `Data` của ledger Cardano:
    /// - `Constr` alt 0..6 → tag `121+alt`; alt 7..127 → `1280+(alt-7)`; khác → tag 102.
    /// - danh sách trường Constr non-empty = mảng **indefinite** (`0x9f…0xff`); rỗng = `0x80`.
    /// - `Map` = definite-length; `Int` ≥ 0 = major-0 uint; `Bytes` = major-2.
    pub fn to_cbor(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }

    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            PlutusData::Int(i) => {
                debug_assert!(*i >= 0, "chỉ encode Int không âm trong datum anchor");
                encode_uint(*i as u64, 0, out);
            }
            PlutusData::Bytes(b) => {
                encode_uint(b.len() as u64, 2, out);
                out.extend_from_slice(b);
            }
            PlutusData::Map(pairs) => {
                encode_uint(pairs.len() as u64, 5, out);
                for (k, v) in pairs {
                    k.encode(out);
                    v.encode(out);
                }
            }
            PlutusData::Constr(alt, fields) => {
                let tag: u64 = match *alt {
                    0..=6 => 121 + *alt,
                    7..=127 => 1280 + (*alt - 7),
                    _ => 102,
                };
                // tag (major 6)
                encode_uint(tag, 6, out);
                if *alt > 127 {
                    // tag 102 → array [alt, fields]
                    out.push(0x82); // array(2)
                    encode_uint(*alt, 0, out);
                }
                encode_field_list(fields, out);
            }
        }
    }

    /// Decode một `PlutusData` từ CBOR (nghịch của [`to_cbor`]). Trả phần dư chưa đọc.
    pub fn from_cbor(bytes: &[u8]) -> Result<PlutusData, CborError> {
        let mut c = Cursor { b: bytes, i: 0 };
        let d = c.read_data()?;
        Ok(d)
    }

    /// Detailed-schema JSON của cardano-cli (`--tx-out-inline-datum-value/-file`) — dạng
    /// tx-builder Mosaic (cardano-cli/Lucid) nạp thẳng. `Constr`→`{constructor,fields}`,
    /// `Int`→`{int}`, `Bytes`→`{bytes:<hex>}`, `Map`→`{map:[{k,v}]}`. cardano-cli tự
    /// canonical-hoá CBOR khi build tx → không lo byte-layout ở tầng Strata.
    pub fn to_detailed_json(&self) -> String {
        match self {
            PlutusData::Int(i) => format!("{{\"int\":{i}}}"),
            PlutusData::Bytes(b) => format!("{{\"bytes\":\"{}\"}}", hex_encode(b)),
            PlutusData::Constr(alt, fields) => {
                let fs: Vec<String> = fields.iter().map(PlutusData::to_detailed_json).collect();
                format!("{{\"constructor\":{alt},\"fields\":[{}]}}", fs.join(","))
            }
            PlutusData::Map(pairs) => {
                let ps: Vec<String> = pairs
                    .iter()
                    .map(|(k, v)| {
                        format!(
                            "{{\"k\":{},\"v\":{}}}",
                            k.to_detailed_json(),
                            v.to_detailed_json()
                        )
                    })
                    .collect();
                format!("{{\"map\":[{}]}}", ps.join(","))
            }
        }
    }
}

/// Hex thường (lowercase) — tránh thêm dep chỉ để encode hex.
fn hex_encode(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

/// Danh sách trường Constr: rỗng = `0x80`; non-empty = indefinite `0x9f … 0xff`.
fn encode_field_list(fields: &[PlutusData], out: &mut Vec<u8>) {
    if fields.is_empty() {
        out.push(0x80);
        return;
    }
    out.push(0x9f);
    for f in fields {
        f.encode(out);
    }
    out.push(0xff);
}

/// Encode giá trị `major`-type với đối số `n` theo quy tắc CBOR (0..23 inline).
fn encode_uint(n: u64, major: u8, out: &mut Vec<u8>) {
    let m = major << 5;
    if n < 24 {
        out.push(m | (n as u8));
    } else if n <= u8::MAX as u64 {
        out.push(m | 24);
        out.push(n as u8);
    } else if n <= u16::MAX as u64 {
        out.push(m | 25);
        out.extend_from_slice(&(n as u16).to_be_bytes());
    } else if n <= u32::MAX as u64 {
        out.push(m | 26);
        out.extend_from_slice(&(n as u32).to_be_bytes());
    } else {
        out.push(m | 27);
        out.extend_from_slice(&n.to_be_bytes());
    }
}

/// Lỗi decode CBOR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CborError {
    Eof,
    Unsupported(u8),
    BadTag(u64),
}

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn byte(&mut self) -> Result<u8, CborError> {
        let v = *self.b.get(self.i).ok_or(CborError::Eof)?;
        self.i += 1;
        Ok(v)
    }
    fn take(&mut self, n: usize) -> Result<&[u8], CborError> {
        let end = self.i.checked_add(n).ok_or(CborError::Eof)?;
        let s = self.b.get(self.i..end).ok_or(CborError::Eof)?;
        self.i = end;
        Ok(s)
    }
    /// Đọc đối số uint sau byte đầu (`ai` = 5 bit thấp).
    fn read_arg(&mut self, ai: u8) -> Result<u64, CborError> {
        Ok(match ai {
            0..=23 => ai as u64,
            24 => self.byte()? as u64,
            25 => u16::from_be_bytes(self.take(2)?.try_into().unwrap()) as u64,
            26 => u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as u64,
            27 => u64::from_be_bytes(self.take(8)?.try_into().unwrap()),
            _ => return Err(CborError::Unsupported(ai)),
        })
    }
    fn read_data(&mut self) -> Result<PlutusData, CborError> {
        let head = self.byte()?;
        let major = head >> 5;
        let ai = head & 0x1f;
        match major {
            0 => Ok(PlutusData::Int(self.read_arg(ai)? as i128)),
            2 => {
                let n = self.read_arg(ai)? as usize;
                Ok(PlutusData::Bytes(self.take(n)?.to_vec()))
            }
            5 => {
                let n = self.read_arg(ai)? as usize;
                let mut pairs = Vec::with_capacity(n);
                for _ in 0..n {
                    let k = self.read_data()?;
                    let v = self.read_data()?;
                    pairs.push((k, v));
                }
                Ok(PlutusData::Map(pairs))
            }
            6 => {
                let tag = self.read_arg(ai)?;
                let (alt, has_prefix_alt) = match tag {
                    121..=127 => (tag - 121, false),
                    1280..=1400 => (tag - 1280 + 7, false),
                    102 => (0, true),
                    _ => return Err(CborError::BadTag(tag)),
                };
                if has_prefix_alt {
                    // array(2) [alt, fields]
                    let _ = self.byte()?; // 0x82
                    let a = self.read_arg_data_int()?;
                    let fields = self.read_field_list()?;
                    Ok(PlutusData::Constr(a, fields))
                } else {
                    let fields = self.read_field_list()?;
                    Ok(PlutusData::Constr(alt, fields))
                }
            }
            _ => Err(CborError::Unsupported(head)),
        }
    }
    fn read_arg_data_int(&mut self) -> Result<u64, CborError> {
        let head = self.byte()?;
        if head >> 5 != 0 {
            return Err(CborError::Unsupported(head));
        }
        self.read_arg(head & 0x1f)
    }
    /// Đọc danh sách trường Constr: `0x80` rỗng, hoặc indefinite `0x9f…0xff`.
    fn read_field_list(&mut self) -> Result<Vec<PlutusData>, CborError> {
        let head = self.byte()?;
        match head {
            0x80 => Ok(Vec::new()),
            0x9f => {
                let mut v = Vec::new();
                loop {
                    if *self.b.get(self.i).ok_or(CborError::Eof)? == 0xff {
                        self.i += 1;
                        break;
                    }
                    v.push(self.read_data()?);
                }
                Ok(v)
            }
            // definite array n (major 4) — chấp nhận để robust.
            h if h >> 5 == 4 => {
                let n = self.read_arg(h & 0x1f)? as usize;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(self.read_data()?);
                }
                Ok(v)
            }
            other => Err(CborError::Unsupported(other)),
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// §8.1b — MosaicAnchorSink (seam Mosaic = MosaicBackend) + idempotency/rollback
// ────────────────────────────────────────────────────────────────────────────

/// AssetClass Cardano = `(policy_id 28B, asset_name)`. Dùng cho **thread-token NFT
/// one-shot** xác thực lineage anchor (§8.1b trust-model).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetClass {
    /// Policy-id (script-hash minting) — 28B.
    pub policy_id: [u8; 28],
    /// Tên asset (thường rỗng cho one-shot NFT, hoặc prefix CIP-68).
    pub asset_name: Vec<u8>,
}

/// Một UTxO ứng viên ở địa chỉ script-anchor: datum + các asset nó GIỮ (để sink lọc
/// theo thread-token). Backend trả nguyên trạng; sink tự xác thực (không tin backend chọn hộ).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAnchor {
    /// Inline-datum CIP-68 của UTxO.
    pub datum: PlutusData,
    /// Thread-token UTxO này mang (None nếu không mang NFT nào khớp lineage).
    pub thread_token: Option<AssetClass>,
}

/// Seam ra Mosaic (VeData). Ranh giới issue #1: **Mosaic dựng+submit tx**, Strata chỉ
/// map datum + gọi. Real impl gọi Mosaic SDK/Lucid (Phase 2); test dùng mock.
pub trait MosaicBackend {
    /// `seq` on-chain hiện tại của `ref_id` (None = chưa neo). Cho idempotency/rollback.
    fn on_chain_seq(&self, ref_id: &Hash32) -> Result<Option<u64>, AnchorError>;
    /// Mosaic dựng reference-UTxO CIP-68 spend-recreate từ datum + submit. Trả receipt.
    fn submit_anchor(&self, datum: &PlutusData) -> Result<AnchorReceipt, AnchorError>;
    /// **HỢP ĐỒNG BẢO MẬT (§8.1b, trust-model thread-token — anh Đức chốt phương án a):**
    /// validator chỉ guard SPEND, KHÔNG guard CREATE → ai cũng gửi được UTxO datum giả
    /// (cùng `ref_id`, seq cao hơn) vào địa chỉ script. Vì vậy backend **KHÔNG được** trả
    /// "UTxO mới nhất tại address" (đầu độc được). Backend PHẢI trả **mọi UTxO ứng viên** ở
    /// địa chỉ anchor cho `ref_id` **kèm asset chúng giữ** (`ResolvedAnchor`), để [`MosaicAnchorSink`]
    /// tự lọc theo **thread-token NFT one-shot** (mint 1 lần từ seed-UTxO genesis; kẻ giả
    /// KHÔNG mint lại được). UTxO không mang đúng NFT → bị sink loại. `Vec` rỗng = chưa neo.
    fn read_anchor(&self, ref_id: &Hash32) -> Result<Vec<ResolvedAnchor>, AnchorError>;
}

/// `AnchorSink` backend Mosaic (CIP-68). Bọc một [`MosaicBackend`] I/O + **pin thread-token
/// one-shot** để xác thực lineage khi `resolve` (phương án a).
pub struct MosaicAnchorSink<B: MosaicBackend> {
    backend: B,
    /// Thread-token NFT one-shot của lineage này (derive từ seed-UTxO genesis, pin ở config).
    /// `None` = **CHẾ ĐỘ KHÔNG XÁC THỰC** — chỉ dùng test/round-trip datum tự tạo; production
    /// PHẢI `with_thread_token` (nếu không, `resolve` đầu độc được — xem doc `read_anchor`).
    expected_token: Option<AssetClass>,
}

impl<B: MosaicBackend> MosaicAnchorSink<B> {
    /// Sink KHÔNG xác thực thread-token — **CHỈ test/round-trip datum tự tạo**. Tên hàm cố
    /// tình dài + rõ để KHÔNG ai vô tình dùng ở production: sink dựng qua đây bị đầu độc
    /// seq-cao ngay (validator chỉ guard SPEND, không guard CREATE — xem doc `read_anchor`).
    /// Production PHẢI [`with_thread_token`](Self::with_thread_token).
    ///
    /// (Trước là `new()`; đổi tên theo review anh Đức PR #6 vòng 2 mục 2 — rào chế độ
    /// không-xác-thực bằng tên hàm thay vì `#[deprecated]` để giữ clippy 0 warning.)
    ///
    /// `#[doc(hidden)]` (review #16): ẩn khỏi doc công khai để khó dùng nhầm ở production —
    /// tăng cường name-guard sẵn có, KHÔNG đảo quyết định name-guard của anh Đức (không
    /// `#[cfg(test)]`: hàm dùng ở integration test `tests/anchor_sink.rs`, gate sẽ phá build).
    #[doc(hidden)]
    pub fn new_unverified_for_tests(backend: B) -> Self {
        Self {
            backend,
            expected_token: None,
        }
    }
    /// Sink production: pin thread-token NFT one-shot của lineage (seed-UTxO genesis).
    /// `resolve` chỉ tin UTxO mang đúng NFT này (phương án a — chuẩn CIP-68 đầy đủ).
    pub fn with_thread_token(backend: B, token: AssetClass) -> Self {
        Self {
            backend,
            expected_token: Some(token),
        }
    }
    pub fn backend(&self) -> &B {
        &self.backend
    }
    /// Thread-token đang pin (None = chế độ không xác thực).
    pub fn expected_token(&self) -> Option<&AssetClass> {
        self.expected_token.as_ref()
    }

    /// Neo có **retry chỉ khi `Network`** (§8.1b — phân tầng retry): backoff MŨ
    /// `base_backoff_ms << attempt`, tối đa `max_attempts` lần gọi. Lỗi không-retryable
    /// (Rejected/Rollback/DatumTooLarge/…) trả NGAY. `sleep` do caller cấp (giữ core thuần,
    /// test injectable — không `thread::sleep` trong lớp này). `max_attempts=0` coi như 1.
    pub fn publish_with_retry(
        &self,
        anchor: &StrataAnchor,
        priority: AnchorPriority,
        max_attempts: u32,
        base_backoff_ms: u64,
        mut sleep: impl FnMut(u64),
    ) -> Result<Option<AnchorReceipt>, AnchorError> {
        let cap = max_attempts.max(1);
        let mut attempt: u32 = 0;
        loop {
            match self.publish(anchor, priority) {
                Err(e) if e.is_retryable() && attempt + 1 < cap => {
                    sleep(base_backoff_ms.saturating_mul(1u64 << attempt.min(63)));
                    attempt += 1;
                }
                other => return other,
            }
        }
    }
}

impl<B: MosaicBackend> AnchorSink for MosaicAnchorSink<B> {
    fn publish(
        &self,
        anchor: &StrataAnchor,
        priority: AnchorPriority,
    ) -> Result<Option<AnchorReceipt>, AnchorError> {
        if priority == AnchorPriority::NoAnchor {
            return Ok(None); // sống tầng (a)/(b), không đẩy
        }
        // Idempotency + rollback + nhảy bậc (§8.1b): query on-chain seq TRƯỚC khi build.
        //
        // Ba nhánh, KHÔNG được gộp: neo lại đúng seq = no-op; neo lùi = rollback (INV-E7);
        // neo vượt quá một bậc = wedge. Nhánh thứ ba trước đây rơi vào `_ => {}` và được đẩy
        // thẳng lên chuỗi, nơi validator từ chối (`seq' == seq + 1`) — nhưng lúc đó head
        // local đã tiến nên KHÔNG có đường quay lại: mọi lần neo sau đều nhảy bậc y hệt.
        match self.backend.on_chain_seq(&anchor.ref_id)? {
            Some(s) if s == anchor.seq => return Ok(None), // đã neo — no-op idempotent
            Some(s) if s > anchor.seq => {
                return Err(AnchorError::RollbackAttempt {
                    on_chain_seq: s,
                    attempted: anchor.seq,
                });
            }
            Some(s) if anchor.seq > s + 1 => {
                return Err(AnchorError::SeqGap {
                    on_chain_seq: Some(s),
                    expected: s + 1,
                    attempted: anchor.seq,
                });
            }
            // `None` (chưa neo lần nào) KHÔNG bị chặn: validator chỉ guard SPEND, không guard
            // CREATE, nên UTxO anchor đầu tiên được mang `seq` bất kỳ. Đây là đường hợp lệ để
            // đưa một chuỗi đã sống off-chain lên neo giữa chừng. Ràng buộc +1 chỉ bắt đầu
            // từ lần neo THỨ HAI trở đi — đúng bằng phạm vi mà chuỗi thật sự ép.
            _ => {}
        }
        let datum = map_anchor_to_datum(anchor);
        self.backend.submit_anchor(&datum).map(Some)
    }

    /// Đọc anchor đích thực on-chain cho `ref_id`. Chống đầu độc (phương án a):
    /// 1. **Thread-token auth**: nếu sink pin token → chỉ nhận UTxO mang đúng NFT one-shot
    ///    (kẻ giả gửi UTxO datum seq-cao KHÔNG mint được NFT → bị loại IM LẶNG).
    /// 2. **Datum parse-fail → BỎ QUA, quét tiếp** (không `Err` — chống DoS: kẻ lạ không ép
    ///    được `resolve` báo lỗi bằng 1 tx ~0,17 tADA). NHƯNG phân biệt (review #6 vòng 2 mục 3):
    ///    UTxO mang **ĐÚNG thread-token** (lineage đã xác thực) mà datum hỏng → **`log::warn!`**
    ///    (anchor THẬT có thể hỏng — mất im lặng là bug khó lần); datum rác của kẻ lạ → im lặng.
    ///
    /// Trong các UTxO hợp lệ (đã xác thực) lấy `seq` cao nhất = đỉnh lineage. `Ok(None)` = chưa neo.
    fn resolve(&self, ref_id: &Hash32) -> Result<Option<StrataAnchor>, AnchorError> {
        let mut best: Option<StrataAnchor> = None;
        for cand in self.backend.read_anchor(ref_id)? {
            // (1) xác thực thread-token. `authenticated` = UTxO thuộc lineage THẬT.
            let authenticated = match &self.expected_token {
                Some(expected) => cand.thread_token.as_ref() == Some(expected),
                None => false, // chế độ không-xác-thực: không phân biệt được thật/giả
            };
            if self.expected_token.is_some() && !authenticated {
                continue; // không mang NFT one-shot → forged, bỏ qua im lặng (đúng)
            }
            // (2) parse datum — anchor thật hỏng thì WARN, kẻ lạ thì im lặng.
            let anchor = match parse_datum_to_anchor(&cand.datum) {
                Ok(a) => a,
                Err(e) => {
                    if authenticated {
                        log::warn!(
                            "resolve: UTxO mang ĐÚNG thread-token nhưng datum parse-fail \
                             (ref_id={}, err={e:?}) — anchor THẬT có thể hỏng, không nuốt im lặng",
                            hex_encode(ref_id)
                        );
                    }
                    continue;
                }
            };
            // ref_id trong datum phải khớp (chống trộn lineage khác vào).
            if &anchor.ref_id != ref_id {
                continue;
            }
            if best.as_ref().is_none_or(|b| anchor.seq > b.seq) {
                best = Some(anchor);
            }
        }
        Ok(best)
    }
}

// ────────────────────────────────────────────────────────────────────────────
// §8.1c — bảng daemon `(seq, mmr_root, mmr_size)` + verify ngược
// ────────────────────────────────────────────────────────────────────────────

/// Một dòng neo — metadata đủ để verify version dưới root CŨ. **KHÔNG lưu proof** (review
/// #4): proof tái dựng từ chain local ở `mmr_size` khi cần (`prove_version_at`) → bỏ ràng
/// buộc timing "record TRƯỚC append", và dòng thuần fixed-size nên save/load gọn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorRecord {
    pub ref_id: Hash32,
    pub seq: u64,
    pub mmr_root: Hash32,
    pub mmr_size: u64,
    pub version_hash: Hash32,
}

/// Lỗi ghi bảng neo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableError {
    /// Ghi đè `(ref_id, seq)` bằng GIÁ TRỊ KHÁC (mmr_root/mmr_size/version_hash lệch) —
    /// chống ghi nhầm/đè lịch sử. Ghi lại y hệt = no-op OK (idempotent).
    ConflictingOverwrite { seq: u64 },
    /// `seq` của anchor không có trong chain (anchor không hợp lệ).
    SeqNotInChain { seq: u64 },
}

/// State daemon: mỗi lần `publish_anchor()` ghi một [`AnchorRecord`]. Key **`(ref_id, seq)`**
/// (review #4 — đa-chain: một daemon phục vụ nhiều `ref_id`). KHÔNG thêm vào core thuần.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnchoredTable {
    rows: Vec<AnchorRecord>,
}

impl AnchoredTable {
    pub fn new() -> Self {
        Self::default()
    }
    /// Ghi một lần neo — lấy `(mmr_size, version_hash)` từ `chain` (proof KHÔNG lưu, tái
    /// dựng khi verify). Idempotent: ghi lại `(ref_id, seq)` y hệt = OK; đè bằng giá trị
    /// KHÁC → `ConflictingOverwrite`. Không còn ràng buộc "gọi TRƯỚC append" vì chỉ lưu
    /// `mmr_size` (số) + verify tái dựng ở size đó.
    pub fn record_anchor(
        &mut self,
        chain: &StrataChain,
        anchor: &StrataAnchor,
    ) -> Result<(), TableError> {
        // Size lịch sử = leaf-count khi `seq` là head = **seq+1** (`StrataAnchor` là cam kết
        // head: `mmr_root` = root của leaves `0..=seq`). KHÔNG dùng `prove_version` — nó trả
        // size HIỆN TẠI, sai khi record MUỘN (sau append). Đây là lý do bỏ được ràng buộc
        // "record TRƯỚC append": chỉ cần seq, size suy ra tất định.
        let mmr_size = anchor.seq + 1;
        let version_hash = chain
            .version(anchor.seq)
            .map(|v| v.version_hash())
            .ok_or(TableError::SeqNotInChain { seq: anchor.seq })?;
        let row = AnchorRecord {
            ref_id: anchor.ref_id,
            seq: anchor.seq,
            mmr_root: anchor.mmr_root,
            mmr_size,
            version_hash,
        };
        if let Some(existing) = self.get(&anchor.ref_id, anchor.seq) {
            if existing == &row {
                return Ok(()); // idempotent — ghi lại y hệt.
            }
            return Err(TableError::ConflictingOverwrite { seq: anchor.seq });
        }
        self.rows.push(row);
        Ok(())
    }
    /// Dòng neo tại `(ref_id, seq)` (None nếu chưa từng neo).
    pub fn get(&self, ref_id: &Hash32, seq: u64) -> Option<&AnchorRecord> {
        self.rows
            .iter()
            .find(|r| r.seq == seq && &r.ref_id == ref_id)
    }
    /// Lần neo mới nhất của `ref_id` (seq cao nhất).
    pub fn latest(&self, ref_id: &Hash32) -> Option<&AnchorRecord> {
        self.rows
            .iter()
            .filter(|r| &r.ref_id == ref_id)
            .max_by_key(|r| r.seq)
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Serialize canonical (daemon persist): `u32_be(count) ‖ [ref_id 32 ‖ u64_be(seq) ‖
    /// mmr_root 32 ‖ u64_be(mmr_size) ‖ version_hash 32]*`. Fixed-size mỗi dòng (112B).
    ///
    /// Count đi qua [`crate::u32_be`] để trần `<2³²` (§1.7 quy tắc 3, issue #18) là MỘT van
    /// duy nhất cho mọi prefix — trước đây chỗ này cast `as u32` trực tiếp, tức doc ghi
    /// `u32_be` mà code không đi qua nó. Đây là đường **persist** (không phải hash-canonical)
    /// nên hậu quả nếu truncate là parse strict trả `None` ⇒ **mất bảng đã lưu**, không phải
    /// va chạm `H_dom`; vẫn fail-loud để không prefix nào nằm ngoài hợp đồng.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + self.rows.len() * 112);
        out.extend_from_slice(&crate::u32_be(self.rows.len()));
        for r in &self.rows {
            out.extend_from_slice(&r.ref_id);
            out.extend_from_slice(&r.seq.to_be_bytes());
            out.extend_from_slice(&r.mmr_root);
            out.extend_from_slice(&r.mmr_size.to_be_bytes());
            out.extend_from_slice(&r.version_hash);
        }
        out
    }
    /// Parse strict (nghịch của [`to_bytes`]): count sai / byte cụt / thừa đuôi → `None`.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 {
            return None;
        }
        let count = u32::from_be_bytes(bytes[0..4].try_into().ok()?) as usize;
        let mut pos = 4;
        let mut rows = Vec::with_capacity(count);
        for _ in 0..count {
            if pos + 112 > bytes.len() {
                return None; // dòng cụt
            }
            let ref_id: Hash32 = bytes[pos..pos + 32].try_into().ok()?;
            let seq = u64::from_be_bytes(bytes[pos + 32..pos + 40].try_into().ok()?);
            let mmr_root: Hash32 = bytes[pos + 40..pos + 72].try_into().ok()?;
            let mmr_size = u64::from_be_bytes(bytes[pos + 72..pos + 80].try_into().ok()?);
            let version_hash: Hash32 = bytes[pos + 80..pos + 112].try_into().ok()?;
            rows.push(AnchorRecord {
                ref_id,
                seq,
                mmr_root,
                mmr_size,
                version_hash,
            });
            pos += 112;
        }
        if pos != bytes.len() {
            return None; // thừa byte đuôi
        }
        Some(Self { rows })
    }
}

/// Lỗi verify ngược (daemon-side).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    /// `on_chain.ref_id` khác chain local.
    RefIdMismatch,
    /// on-chain đi TRƯỚC local (local stale — cần đồng bộ lại).
    OnChainAhead { on_chain: u64, local: u64 },
    /// Không có version tại `seq` on-chain trong local.
    SeqMissing(u64),
    /// `head_version_hash` on-chain khác version local tại seq đó (local đã diverge).
    HeadMismatch,
    /// Daemon chưa lưu dòng neo cho seq đó (bảng thiếu — hoặc seq chưa neo).
    NotAnchored(u64),
    /// Inclusion-proof không verify dưới `mmr_root` đã neo.
    ProofFail,
}

/// Verify anchor on-chain (đã `resolve`) khớp lịch sử local (§8.1c): version tại
/// `on_chain.seq` thuộc `on_chain.mmr_root` (dưới `mmr_size` + `proof` TẠI lúc neo), và
/// local chưa diverge (version local tại seq == `head_version_hash` đã neo).
pub fn verify_resolved(
    chain: &StrataChain,
    on_chain: &StrataAnchor,
    table: &AnchoredTable,
) -> Result<(), VerifyError> {
    if on_chain.ref_id != chain.anchor().ref_id {
        return Err(VerifyError::RefIdMismatch);
    }
    let local_head = chain.head().seq;
    if on_chain.seq > local_head {
        return Err(VerifyError::OnChainAhead {
            on_chain: on_chain.seq,
            local: local_head,
        });
    }
    // Local version tại seq phải khớp head đã neo (chống divergence). `rec.version_hash`
    // (lưu lúc record) == `local_vh` do version bất biến append-only → CHỈ kiểm một lần ở
    // đây là đủ (review #6 vòng 2 mục 5: gộp check version_hash trùng lặp).
    let local_vh = chain
        .version(on_chain.seq)
        .map(|v| v.version_hash())
        .ok_or(VerifyError::SeqMissing(on_chain.seq))?;
    if local_vh != on_chain.head_version_hash {
        return Err(VerifyError::HeadMismatch);
    }
    // Verify dưới root ĐÃ NEO ở `mmr_size` cũ — proof TÁI DỰNG từ chain local (không lưu).
    let rec = table
        .get(&on_chain.ref_id, on_chain.seq)
        .ok_or(VerifyError::NotAnchored(on_chain.seq))?;
    let (proof, vh) = chain
        .prove_version_at(on_chain.seq, rec.mmr_size)
        .ok_or(VerifyError::SeqMissing(on_chain.seq))?;
    if vh != on_chain.head_version_hash {
        return Err(VerifyError::HeadMismatch);
    }
    if !StrataChain::verify_version(
        on_chain.mmr_root,
        &on_chain.head_version_hash,
        on_chain.seq,
        rec.mmr_size,
        &proof,
    ) {
        return Err(VerifyError::ProofFail);
    }
    Ok(())
}
