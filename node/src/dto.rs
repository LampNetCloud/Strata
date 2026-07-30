//! DTO request/response — khớp **§3** `Strata-API.md` từng trường, từng tên.
//!
//! Quy ước: mọi hash/CID/sig truyền **hex**; `field_key` truyền **chuỗi thường** (spec ví dụ
//! `"diagnosis"`) nên map thẳng sang `key.as_bytes()`; `value` là hex (thường là
//! `content_cid` 32B thuần — CHỐT-4).

use crate::hexs;
use lampnet_merkle_anchor::mmr::InclusionProof;
use lampnet_strata::state::FieldProof;
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
pub fn to_pairs(fields: &[FieldDto]) -> Result<StateFields, String> {
    fields.iter().map(FieldDto::to_pair).collect()
}

// ────────────────────────────────────────────────────────────────────────────
// POST /v1/strata/create
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
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
