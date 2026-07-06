//! `lampnet-anchor-sink` — S1 `AnchorSink` (Strata-API §4.1 + §8.1).
//!
//! Adapter MỘT-ĐƯỜNG: nhận [`StrataAnchor`] 104-byte (đã enforce INV-E7 ở core qua
//! `publish_anchor()`), đẩy on-chain. Backend mặc định = **Settlement**: tx metadata
//! **label 1234**, payload CBOR raw-bytes (KHÔNG JSON-hex) — xem [`payload`].
//!
//! Nguyên tắc cứng:
//! - Core `lampnet-strata` KHÔNG biết Cardano; crate này sống ở daemon (§4.3).
//! - Trust: anchor CHỈ hợp lệ khi tx phát từ ví publisher đã pin trong config —
//!   `resolve()` lọc theo địa chỉ INPUT của tx (không phải output; tx lạ *gửi tới*
//!   publisher không được tính). Xem [`settlement`].
//! - Idempotency §8.1b: trước khi build tx, đọc on-chain seq;
//!   `on_chain_seq == anchor.seq` → `Ok(None)`; `>` → [`AnchorError::RollbackAttempt`].
//! - Chỉ [`AnchorError::Network`] retryable (backoff); còn lại fail-hard.
//! - KHÔNG bao giờ in secret (mnemonic/token) ra log/error — mọi type cầm secret
//!   redact trong `Debug`, submitter TS tự lọc trước khi in.

pub mod anchored_log;
pub mod payload;
pub mod settlement;
pub mod verify;

pub use anchored_log::AnchoredLog;
pub use payload::{AnchorRecord, PayloadError, decode_records, encode_records};
pub use settlement::{
    BlockfrostQuery, ChainQuery, SettlementSink, SinkConfig, SubmitOutcome, Submitter,
    TsSubmitter, publish_with_retry,
};
pub use verify::{VerifyError, verify_anchored};

use lampnet_strata::{Hash32, StrataAnchor};

/// Nhãn metadata Cardano cho anchor Strata (đối chiếu `settle.ts` LampNet dùng 1234).
pub const METADATA_LABEL: u64 = 1234;

/// Cadence đẩy anchor — 4-enum = Stamp 4-enum (Stamp-Strata-Mapping §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorPriority {
    /// Đẩy mỗi version (Mosaic A).
    Immediate,
    /// Đẩy theo mốc/epoch.
    Milestone,
    /// Gom ngày (settlement metadata) — rẻ nhất.
    BatchDaily,
    /// KHÔNG đẩy — sống tầng (a)/(b).
    NoAnchor,
}

/// Backend neo (§4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorBackend {
    /// Tx metadata label 1234 (LampNet Settlement).
    Settlement,
    /// Reference UTxO CIP-68 spend-recreate (VeData Mosaic) — chưa cài ở crate này.
    Mosaic,
}

/// Biên nhận sau khi đẩy anchor thành công (§4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorReceipt {
    /// Tx hash on-chain.
    pub txid: String,
    /// Backend đã dùng.
    pub backend: AnchorBackend,
    /// Slot (nếu backend trả).
    pub slot: Option<u64>,
}

/// Lỗi adapter — error-semantics đầy đủ §8.1b. CHỈ `Network` retryable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorError {
    /// Backend chưa cấu hình (thiếu key/URL/publisher).
    NotConfigured,
    /// Backend/validator từ chối — fail cứng, KHÔNG retry.
    Rejected(String),
    /// Lỗi mạng/timeout — RETRYABLE (backoff).
    Network(String),
    /// INV-E7 lớp adapter: on-chain đã có seq CAO HƠN seq đang cố neo.
    RollbackAttempt { on_chain_seq: u64, attempted: u64 },
    /// Payload metadata vượt giới hạn cấu hình / maxTxSize.
    DatumTooLarge { bytes: usize },
    /// Ví không đủ ADA (min-ADA/fee).
    InsufficientAda { need: u64, have: u64 },
}

impl std::fmt::Display for AnchorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for AnchorError {}

impl AnchorError {
    /// CHỈ `Network` được retry (§8.1b "Phân tầng retryable").
    pub fn is_retryable(&self) -> bool {
        matches!(self, AnchorError::Network(_))
    }
}

/// Adapter một-đường (§4.1 + §8.1c). Một trait, nhiều backend.
pub trait AnchorSink {
    /// Đẩy commitment. `priority` lấy từ Stamp anchor_priority.
    /// `Ok(None)` khi: priority == NoAnchor, HOẶC anchor này ĐÃ neo (idempotent no-op).
    fn publish(
        &self,
        anchor: &StrataAnchor,
        priority: AnchorPriority,
    ) -> Result<Option<AnchorReceipt>, AnchorError>;

    /// Đọc anchor MỚI NHẤT (seq cao nhất) on-chain cho một `ref_id`, CHỈ tính tx phát
    /// từ ví publisher. `None` nếu chưa neo bao giờ.
    fn resolve(&self, ref_id: &Hash32) -> Result<Option<StrataAnchor>, AnchorError>;
}
