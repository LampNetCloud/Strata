//! DTO request/response — khớp **§3** `Strata-API.md` từng trường, từng tên.
//!
//! Quy ước: mọi hash/CID/sig truyền **hex**; `field_key` truyền **chuỗi thường** (spec ví dụ
//! `"diagnosis"`) nên map thẳng sang `key.as_bytes()`; `value` là hex (thường là
//! `content_cid` 32B thuần — CHỐT-4).

use crate::hexs;
use lampnet_merkle_anchor::mmr::InclusionProof;
use lampnet_strata::state::FieldProof;
use lampnet_strata::state::find_duplicate_key;
use lampnet_strata::version::{Hash32, StrataVersion};
use lampnet_strata::{AnchorPriority, StrataAnchor};
use serde::{Deserialize, Serialize};

/// Một cặp `(key_bytes, value_bytes)` — đúng dạng core nhận (`state::build_state_root`).
pub type FieldPair = (Vec<u8>, Vec<u8>);
/// Toàn bộ `state_fields` của một version.
pub type StateFields = Vec<FieldPair>;

/// Một trường state: `key` chuỗi, `value` hex.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDto {
    pub key: String,
    pub value: String,
}

impl FieldDto {
    /// `(key_bytes, value_bytes)` — dạng core nhận (`build_state_root`/`prove_field`).
    pub fn to_pair(&self) -> Result<FieldPair, String> {
        let v = hexs::decode_var(&self.value)
            .map_err(|e| format!("state_fields[{}]: {e}", self.key))?;
        Ok((self.key.as_bytes().to_vec(), v))
    }
}

/// Chuyển cả danh sách; lỗi hex đầu tiên làm hỏng cả request (fail-closed).
///
/// **Từ chối khoá TRÙNG** (INV-E6, `#39` điểm 2 — phạm vi chốt ở `#40` P6): trùng key là
/// `MalformedRequest` ⇒ **400**, kể cả khi hai mục cùng giá trị.
///
/// `state.rs` ghi trong doc-comment rằng khoá là duy nhất "sau khi caller bảo đảm" — caller
/// chính là chỗ này, và trước đây nó không bảo đảm gì. Hệ quả đã dựng lại được đầu-cuối: gửi
/// `[{diagnosis, aa}, {diagnosis, bb}]` thì `sorted_leaves` sort ỔN ĐỊNH nên sinh HAI lá
/// riêng dưới CÙNG một `state_root` — root đó đi vào `canonical_core` ⇒ vào `version_hash`,
/// được ký Ed25519 và neo on-chain. Sau đó tồn tại **hai field-proof đều verify đúng**, cùng
/// khoá `diagnosis`, một trả `aa` một trả `bb`, cùng `state_root` đã neo. Người ghi luôn có
/// đường chối — non-repudiation của INV-E6 sụp. Cùng lẽ đó, đảo thứ tự hai mục trùng key
/// cho **hai root khác nhau**: không gác thì hỏng **im lặng**, chỉ còn một chữ ký nói về một
/// root phụ thuộc thứ tự người gọi xếp danh sách.
///
/// Đây là cổng ở **CỬA**, và cửa là chỗ đúng: `build_state_root` vô-lỗi và được gọi ở nhiều
/// chỗ nội bộ (`derived_index`, `composite`) nơi tập field đã qua đây. Còn hở, ghi rõ để
/// không ai tưởng đã đóng: `build_state_root`/`prove_field` là API `pub` của SDK, đội tích
/// hợp gọi thẳng crate vẫn dựng được `state_root` mâu thuẫn — họ nay có
/// [`find_duplicate_key`] để gọi, nhưng **gọi hay không vẫn là lựa chọn của họ**. Đóng hẳn
/// thuộc `#39` + đợt đổi byte-layout.
pub fn to_pairs(fields: &[FieldDto]) -> Result<StateFields, String> {
    let pairs: StateFields = fields
        .iter()
        .map(FieldDto::to_pair)
        .collect::<Result<_, _>>()?;

    if let Some(dup) = find_duplicate_key(&pairs) {
        return Err(format!(
            "state_fields: khoá trùng {:?} — một khoá chỉ được xuất hiện MỘT lần (INV-E6); \
             khoá trùng làm state_root cam kết hai giá trị mâu thuẫn cho cùng trường, và làm \
             root phụ thuộc thứ tự truyền vào — mà state_root thì được ký",
            String::from_utf8_lossy(&dup)
        ));
    }
    Ok(pairs)
}

// ────────────────────────────────────────────────────────────────────────────
// POST /v1/strata/create
// ────────────────────────────────────────────────────────────────────────────

/// ⚠️ `Serialize` có mặt vì **nhật ký bền vững ghi lại chính request này** (xem
/// [`crate::journal`]), không phải để trả ra cửa. Thêm/đổi trường ở `CreateReq` là
/// **đổi định dạng nhật ký** ⇒ phải nâng `journal::FORMAT_VERSION`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateReq {
    #[serde(with = "hexs::h32")]
    pub author_did: [u8; 32],
    #[serde(with = "hexs::h32")]
    pub genesis_nonce: [u8; 32],
    #[serde(with = "hexs::hvar")]
    pub content_cid: Vec<u8>,
    #[serde(default)]
    pub state_fields: Vec<FieldDto>,
    #[serde(with = "hexs::h32")]
    pub policy_hash: [u8; 32],
    pub ts: u64,
    #[serde(with = "hexs::h64")]
    pub sig: [u8; 64],
    /// **MỞ RỘNG ngoài §3** (§3 không nói tập author của policy lấy từ đâu): danh sách DID
    /// hex được phép ghi. Vắng ⇒ policy một-thành-viên `[author_did]`. Mọi DID phải phân
    /// giải được qua key-registry, và `policy_hash` gửi lên phải khớp policy dựng ra.
    #[serde(default)]
    pub policy_authors: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateResp {
    /// bech32m `lnref1…` (§2.1: `gen_ref_id` trả String).
    pub ref_id: String,
    pub head_seq: u64,
    pub head_version_hash: String,
    pub mmr_root: String,
}

// ────────────────────────────────────────────────────────────────────────────
// POST /v1/strata/:ref/version
// ────────────────────────────────────────────────────────────────────────────

/// ⚠️ `Serialize` có mặt vì **nhật ký bền vững ghi lại chính request này** (xem
/// [`crate::journal`]), không phải để trả ra cửa. Thêm/đổi trường ở `AppendReq` là
/// **đổi định dạng nhật ký** ⇒ phải nâng `journal::FORMAT_VERSION`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendReq {
    /// seq của head mà client tin là mình đang nối vào (chống ghi đè mù).
    pub prev_seq: u64,
    #[serde(with = "hexs::hvar")]
    pub content_cid: Vec<u8>,
    #[serde(default)]
    pub state_fields: Vec<FieldDto>,
    #[serde(with = "hexs::h32")]
    pub author_did: [u8; 32],
    #[serde(with = "hexs::h32")]
    pub policy_hash: [u8; 32],
    pub ts: u64,
    #[serde(with = "hexs::h64")]
    pub sig: [u8; 64],
}

#[derive(Debug, Clone, Serialize)]
pub struct AppendResp {
    pub seq: u64,
    pub version_hash: String,
    pub mmr_root: String,
    pub prev_hash: String,
}

// ────────────────────────────────────────────────────────────────────────────
// POST /v1/strata/:ref/event
// ────────────────────────────────────────────────────────────────────────────

/// ⚠️ `Serialize` có mặt vì **nhật ký bền vững ghi lại chính request này** (xem
/// [`crate::journal`]), không phải để trả ra cửa. Thêm/đổi trường ở `AuditEventReq` là
/// **đổi định dạng nhật ký** ⇒ phải nâng `journal::FORMAT_VERSION`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEventReq {
    #[serde(with = "hexs::h32")]
    pub actor_did: [u8; 32],
    /// `Create` | `Read` | `Sign` | `ShareProof` | `Update` (audit.rs).
    pub action: String,
    #[serde(with = "hexs::h32")]
    pub signed_hash: [u8; 32],
    #[serde(with = "hexs::h32")]
    pub location: [u8; 32],
    pub ts: u64,
    #[serde(with = "hexs::h64")]
    pub sig: [u8; 64],
}

/// `kind` phân nhánh hai ngữ nghĩa của §2.6: event-là-version, hay entry audit-log.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EventReq {
    Audit(AuditEventReq),
    Version(AppendReq),
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditEventResp {
    pub index: usize,
    pub log_root: String,
}

// ────────────────────────────────────────────────────────────────────────────
// GET /v1/strata/:ref/head
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct HeadResp {
    pub ref_id: String,
    pub head_seq: u64,
    pub head_version_hash: String,
    pub mmr_root: String,
    pub content_cid: String,
    /// `ts` của head — client PHẢI gửi `ts >= ` giá trị này ở version kế (đơn điệu thời gian,
    /// `chain.rs` `TimestampRegress`). Thiếu trường này thì client chỉ cầm `ref_id` không
    /// append tiếp được, phải lách bằng `GET /version?at=<số lớn>`.
    pub ts: u64,
    /// Cam kết policy của head — client phải gửi ĐÚNG giá trị này ở version kế, nếu không
    /// nhận `403 PolicyHashMismatch`. Cùng lý do như `ts`: không có thì phải đoán.
    pub policy_hash: String,
    /// Tác giả head (tiện đối chiếu; không phải yêu cầu để append).
    pub author_did: String,
}

// ────────────────────────────────────────────────────────────────────────────
// POST /v1/strata/_canonical — route KHÔ (dry-run), KHÔNG ghi
// ────────────────────────────────────────────────────────────────────────────

/// Đầu vào giống hệt `create`/`version` nhưng daemon **không ghi gì**: chỉ dựng lại
/// `state_root` + `canonical_core` + `version_hash` rồi trả về.
///
/// Vì sao cần: client phải tự cài lại HAI cây băm (state-tree + MMR) và MỘT encoding
/// canonical ở ngôn ngữ của mình, rồi ký lên `version_hash`. Sai một bit ⇒ `403 BadSignature`
/// với thông điệp không hề nhắc tới `state_root`. Không có đường nào đối chiếu trước khi ghi
/// ⇒ người tích hợp ngồi đoán. Route này là đường đối chiếu đó.
///
/// Genesis: gửi `seq = 0`, `prev_hash = "00"×32`, kèm `genesis_nonce` để nhận luôn `ref_id`.
#[derive(Debug, Clone, Deserialize)]
pub struct CanonicalReq {
    pub seq: u64,
    #[serde(with = "hexs::h32")]
    pub prev_hash: [u8; 32],
    #[serde(with = "hexs::hvar")]
    pub content_cid: Vec<u8>,
    #[serde(default)]
    pub state_fields: Vec<FieldDto>,
    #[serde(with = "hexs::h32")]
    pub author_did: [u8; 32],
    #[serde(with = "hexs::h32")]
    pub policy_hash: [u8; 32],
    pub ts: u64,
    /// Hex 32B. Có ⇒ trả kèm `ref_id` dự kiến (`H_dom(author_did ‖ nonce)`).
    #[serde(default)]
    pub genesis_nonce: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CanonicalResp {
    /// Toàn bộ `canonical_core` dạng hex — client so BYTE với bản mình dựng.
    pub canonical_core: String,
    /// Thứ phải ký (PureEd25519 TRỰC TIẾP trên 32 byte này, không băm thêm lần nữa).
    pub version_hash: String,
    /// `state_root` daemon dựng từ `state_fields` — chỗ lệch phổ biến nhất.
    pub state_root: String,
    /// `lnref1…` khi request có `genesis_nonce`; `null` khi không.
    pub ref_id: Option<String>,
}

// ────────────────────────────────────────────────────────────────────────────
// Proof
// ────────────────────────────────────────────────────────────────────────────

/// MMR inclusion-proof dạng dây (§3: `leaf_seq`/`leaf_hash`/`mmr_size` + 3 trường của
/// `InclusionProof`). `mmr_size` bắt buộc — verifier cần nó để suy chiều trái/phải.
#[derive(Debug, Clone, Serialize)]
pub struct ProofDto {
    pub leaf_seq: u64,
    pub leaf_hash: String,
    pub mmr_size: u64,
    pub siblings: Vec<(String, bool)>,
    pub peak_index: usize,
    pub peaks: Vec<String>,
}

impl ProofDto {
    pub fn new(leaf_seq: u64, leaf_hash: Hash32, mmr_size: u64, p: &InclusionProof) -> Self {
        Self {
            leaf_seq,
            leaf_hash: hex::encode(leaf_hash),
            mmr_size,
            siblings: p
                .siblings
                .iter()
                .map(|(h, right)| (hex::encode(h), *right))
                .collect(),
            peak_index: p.peak_index,
            peaks: p.peaks.iter().map(hex::encode).collect(),
        }
    }
}

/// `StrataVersion` dạng hex (§3: `"version": { /* StrataVersion hex */ }`).
#[derive(Debug, Clone, Serialize)]
pub struct VersionDto {
    pub seq: u64,
    pub prev_hash: String,
    pub content_cid: String,
    pub state_root: String,
    pub author_did: String,
    pub policy_hash: String,
    pub ts: u64,
    pub sig: String,
    /// Tiện cho client: `version_hash` tính lại từ chính các trường trên (CHỐT-1, không gồm sig).
    pub version_hash: String,
}

impl From<&StrataVersion> for VersionDto {
    fn from(v: &StrataVersion) -> Self {
        Self {
            seq: v.seq,
            prev_hash: hex::encode(v.prev_hash),
            content_cid: hex::encode(&v.content_cid),
            state_root: hex::encode(v.state_root),
            author_did: hex::encode(v.author_did),
            policy_hash: hex::encode(v.policy_hash),
            ts: v.ts,
            sig: hex::encode(v.sig),
            version_hash: hex::encode(v.version_hash()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct VersionAtResp {
    pub seq: u64,
    pub version: VersionDto,
    pub proof: ProofDto,
}

#[derive(Debug, Clone, Serialize)]
pub struct FieldProofResp {
    pub key: String,
    pub value: String,
    pub fvh: String,
    /// Salt làm mù (§6.3), hex. **Rỗng = không làm mù.**
    ///
    /// Có mặt kể cả khi rỗng: verifier phải băm `salt ‖ value` để so `fvh`, nên một
    /// trường vắng mặt lúc blinding được bật sẽ làm **mọi** proof đỏ ở phía client mà
    /// nhìn từ server thì không có gì sai.
    pub salt: String,
    pub siblings: Vec<(String, bool)>,
    pub state_root: String,
    pub version_seq: u64,
}

impl FieldProofResp {
    pub fn new(p: &FieldProof, version_seq: u64) -> Self {
        Self {
            key: String::from_utf8_lossy(&p.key).into_owned(),
            value: hex::encode(&p.value),
            fvh: hex::encode(p.fvh),
            salt: hex::encode(&p.salt),
            siblings: p
                .siblings
                .iter()
                .map(|(h, right)| (hex::encode(h), *right))
                .collect(),
            state_root: hex::encode(p.state_root),
            version_seq,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// POST /v1/strata/:ref/anchor
// ────────────────────────────────────────────────────────────────────────────

/// 4-enum = enum `anchor_priority` của Stamp (§8.4 — Stamp là SSoT của giá trị này).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorityDto {
    Immediate,
    Milestone,
    BatchDaily,
    NoAnchor,
}

impl From<PriorityDto> for AnchorPriority {
    fn from(p: PriorityDto) -> Self {
        match p {
            PriorityDto::Immediate => AnchorPriority::Immediate,
            PriorityDto::Milestone => AnchorPriority::Milestone,
            PriorityDto::BatchDaily => AnchorPriority::BatchDaily,
            PriorityDto::NoAnchor => AnchorPriority::NoAnchor,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnchorReq {
    pub priority: PriorityDto,
}

/// Neo **một lô** nhiều ref trong MỘT tx — mối nối B1′: Mosaic quyết lô, Strata
/// kiểm INV-E7 + encode, Mosaic dựng tx.
#[derive(Debug, Clone, Deserialize)]
pub struct AnchorBatchReq {
    /// Các ref cần neo — bech32m `lnref1…` hoặc hex32. **Không được trùng**: hai
    /// entry cùng một lineage trong một tx thì entry sau là rollback của entry
    /// trước, và không thứ tự nào cứu được điều đó.
    pub refs: Vec<String>,
    pub priority: PriorityDto,
}

/// Kết quả neo lô. MỘT `anchor_txid` cho **cả lô** — đó chính là tính chất đường
/// này giữ (1 tx / N anchor, `~0,896` tADA thay vì `~89,6`).
#[derive(Debug, Clone, Serialize)]
pub struct AnchorBatchResp {
    pub anchor_txid: Option<String>,
    pub backend: Option<String>,
    /// Số anchor thật sự nằm trong lô.
    pub batch_size: usize,
    /// Từng anchor trong lô, đúng thứ tự đã gửi lên chuỗi.
    pub anchors: Vec<AnchorResp>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnchorResp {
    /// §3 ghi `<hex32>` ở route này (khác `create`/`head` trả bech32) — giữ đúng spec.
    pub ref_id: String,
    pub head_version_hash: String,
    pub mmr_root: String,
    pub seq: u64,
    /// `null` khi `no_anchor`, hoặc khi backend báo đã neo idempotent từ trước.
    pub anchor_txid: Option<String>,
    pub backend: Option<String>,
}

/// Một lineage **đang chờ neo** — phần tử của `GET /v1/strata/_dirty`.
///
/// Không có trường nào là bộ đếm: mọi số dưới đây **tính ra** từ trạng thái đã có
/// (`chain` cho `head_seq`, gương `anchored` cho seq đã neo). Một bộ đếm tăng dần
/// là *trạng thái* — lệch được, đếm trùng được, mất khi restart được; một tổng suy
/// từ nguồn sự thật thì không.
#[derive(Debug, Clone, Serialize)]
pub struct DirtyRefResp {
    /// hex32 — **cùng dạng** `AnchorResp.ref_id`, vì người đọc route này nạp thẳng
    /// danh sách đó vào `_anchor_batch`. (Route `create`/`head` trả bech32m; trộn
    /// hai dạng trong một đường ống là cách tự tạo lỗi phân giải.)
    pub ref_id: String,
    /// `author_did` của **genesis** — nhóm tự nhiên của lineage.
    ///
    /// ⚠️ Từ chốt 2026-08-19, đây **KHÔNG còn là ranh giới lô**: lô gom **liên hộ**,
    /// chia theo **kích cỡ**. Trường này còn lại để báo cáo / hạn mức, và để bên
    /// tiêu thụ ráp `lineage → địa chỉ` khi tới lúc — `ref_id` là hàm một chiều nên
    /// không suy ngược được.
    pub author_did: String,
    /// `seq` của head hiện tại.
    pub head_seq: u64,
    /// `seq` đã neo (null = **chưa neo lần nào**).
    pub anchored_seq: Option<u64>,
    /// Số version chưa được neo. Chưa neo lần nào ⇒ `head_seq + 1` (tính cả genesis),
    /// không phải `head_seq` — một chain mới chỉ có genesis vẫn là một lineage đang chờ.
    pub pending_versions: u64,
    /// `ts` của version **cũ nhất chưa neo** — mốc để cò tuổi (`N-1`: cận trên ≤ 24 h)
    /// đo độ tươi của con dấu.
    pub oldest_unanchored_ts: u64,
}

/// Kết quả `GET /v1/strata/_dirty`.
#[derive(Debug, Clone, Serialize)]
pub struct DirtyResp {
    /// Số lineage đang chờ neo (sau khi áp `limit`, nếu có).
    pub count: usize,
    /// Tổng version chưa neo trên toàn bộ danh sách trả về — số liệu **bậc SLA**
    /// (độ sâu hàng đợi), KHÔNG phải cò neo.
    pub total_pending_versions: u64,
    /// `min(oldest_unanchored_ts)` — null khi không có gì chờ.
    pub oldest_unanchored_ts: Option<u64>,
    /// `true` khi `limit` đã cắt bớt danh sách. Người gọi thấy cờ này thì biết mình
    /// **không** đang nhìn toàn cảnh — im lặng cắt là cách một hàng đợi tưởng mình
    /// rỗng trong khi vẫn còn việc.
    pub truncated: bool,
    /// Cũ trước, mới sau (`oldest_unanchored_ts` tăng dần; hoà thì theo `ref_id`).
    /// Thứ tự này là **cam kết**, không phải tình cờ: nó làm cho phép cắt `limit` lấy
    /// đúng phần chờ lâu nhất, và làm lô tất định giữa hai lượt chạy.
    pub refs: Vec<DirtyRefResp>,
}

/// Một anchor **đọc lại từ chuỗi** trong cửa sổ slot — phần tử của
/// `GET /v1/strata/_settlement_window`.
///
/// Bốn trường anchor giữ **đúng thứ tự canonical** của `StrataAnchor`
/// (`ref_id ‖ head_version_hash ‖ mmr_root ‖ seq`): bên tiêu thụ băm lại đúng 104
/// byte đó để dựng lá checkpoint, nên thứ tự ở đây là **hợp đồng byte**, không phải
/// một lựa chọn trình bày.
#[derive(Debug, Clone, Serialize)]
pub struct WindowAnchorResp {
    /// hex32 — cùng dạng `AnchorResp.ref_id`.
    pub ref_id: String,
    pub head_version_hash: String,
    pub mmr_root: String,
    pub seq: u64,
    /// Slot của block chứa tx — bên đọc kiểm lại được nó thuộc cửa sổ nào.
    pub slot: u64,
    /// Tx đã chở record. Có nó thì lời khai *"anchor này nằm trong chu kỳ"* **tra
    /// ngược được** tới một tx thật, thay vì phải tin daemon.
    pub txid: String,
}

/// Kết quả `GET /v1/strata/_settlement_window`.
///
/// ⚠️ **Không có cờ `truncated`, có chủ ý.** Lượt quét không phủ hết cửa sổ là
/// **lỗi** (409), không phải một danh sách ngắn hơn: bên gọi sẽ tính `root` trên tập
/// thiếu, chốt nó lên chuỗi, và không có gì bật ra — chuỗi `epoch` vẫn liên tục, cửa
/// sổ vẫn khít, chỉ nội dung cam kết là ít hơn sự thật. Đó đúng là loại hỏng mà
/// luồng checkpoint sinh ra để chặn, nên nó không được xuất hiện trong chính luồng.
#[derive(Debug, Clone, Serialize)]
pub struct WindowResp {
    pub from_slot: u64,
    pub to_slot: u64,
    /// Slot của block mới nhất lúc quét. Bên gọi tự quyết cửa sổ đã đủ **sâu** để
    /// đóng chưa (rủi ro rollback) — route này **không** quyết thay: độ sâu an toàn
    /// là tham số của mạng và của khẩu vị rủi ro, không phải của phép đọc.
    pub tip_slot: u64,
    /// Số tx đã đọc — giá thật của một chu kỳ, đo được thay vì ước.
    pub scanned_txs: usize,
    pub count: usize,
    /// **Chưa khử trùng**: hai tx cùng chở một anchor (thử lại) thì cả hai xuất
    /// hiện. Luật khử trùng `(ref_id, seq)` thuộc bên tính `root`.
    pub anchors: Vec<WindowAnchorResp>,
}

impl AnchorResp {
    pub fn new(a: &StrataAnchor, txid: Option<String>, backend: Option<String>) -> Self {
        Self {
            ref_id: hex::encode(a.ref_id),
            head_version_hash: hex::encode(a.head_version_hash),
            mmr_root: hex::encode(a.mmr_root),
            seq: a.seq,
            anchor_txid: txid,
            backend,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dto(k: &str, v: &str) -> FieldDto {
        FieldDto {
            key: k.into(),
            value: v.into(),
        }
    }

    #[test]
    fn to_pairs_nhan_key_phan_biet() {
        let out = to_pairs(&[dto("a", "01"), dto("b", "02")]).expect("key phân biệt phải qua");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn to_pairs_tu_choi_trung_key() {
        let err = to_pairs(&[dto("a", "01"), dto("b", "02"), dto("a", "ff")])
            .expect_err("trùng key phải bị từ chối (INV-E6)");
        assert!(
            err.contains("khoá trùng"),
            "thông điệp phải nói rõ lý do: {err}"
        );
        assert!(
            err.contains('a'),
            "thông điệp phải chỉ ĐÚNG key nào trùng: {err}"
        );
    }

    /// `#40` P6 chốt reject **kể cả same-value** — đây là ca dễ bị nới nhất, vì "hai mục
    /// giống hệt nhau thì hại gì" nghe hợp lý. Hại là: nó vẫn thêm một LÁ nên vẫn đổi root.
    #[test]
    fn to_pairs_tu_choi_trung_key_ke_ca_cung_gia_tri() {
        to_pairs(&[dto("dup", "07"), dto("dup", "07")])
            .expect_err("trùng key cùng giá trị VẪN phải bị từ chối");
    }

    /// Gác trùng key không được nuốt mất lỗi hex — hai cửa fail-closed độc lập.
    #[test]
    fn to_pairs_van_bao_loi_hex() {
        let err = to_pairs(&[dto("a", "zz")]).expect_err("hex hỏng phải bị từ chối");
        assert!(
            !err.contains("khoá trùng"),
            "phải là lỗi hex, không phải lỗi trùng: {err}"
        );
    }
}
