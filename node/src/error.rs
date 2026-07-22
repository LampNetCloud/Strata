//! Ánh xạ lỗi → HTTP theo **bảng §3.1** (`Strata-API.md`). Fail-closed: mọi vi phạm
//! invariant từ chối request, KHÔNG ghi một phần.
//!
//! Body lỗi: `{ "error": "<StrataError variant>", "detail": {…} }` — tên biến thể giữ
//! NGUYÊN VĂN tên trong `chain::StrataError` để client map ngược được.
//!
//! Ngoài 11 biến thể lõi, daemon còn 6 lỗi **mức-cửa** (request hỏng, ref không có,
//! backend neo chết…) — §3.1 không phủ; đánh dấu rõ ở bảng dưới, KHÔNG trộn tên với
//! biến thể lõi.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use lampnet_strata::chain::StrataError;
use lampnet_strata::{AnchorError, Hash32};
use serde_json::{Value, json};

/// Lỗi trả ra cửa HTTP.
#[derive(Debug, Clone)]
pub enum ApiError {
    /// Vi phạm invariant ở lõi — ánh xạ theo bảng §3.1.
    Core(StrataError),
    /// Body/param không giải mã được (hex sai độ dài, enum lạ…). 400.
    Malformed(String),
    /// `ref` / `seq` / `key` không tồn tại. 404.
    NotFound(&'static str),
    /// `create` trên ref_id đã tồn tại (cùng `author_did`+`genesis_nonce`). 409.
    RefExists(Hash32),
    /// Backend neo chưa cấu hình (`AnchorError::NotConfigured`). 501.
    AnchorNotConfigured,
    /// Backend neo từ chối (`Rejected`). 502.
    AnchorRejected(String),
    /// Backend neo lỗi mạng (`Network`) — client retry được. 503.
    AnchorNetwork(String),
}

impl From<StrataError> for ApiError {
    fn from(e: StrataError) -> Self {
        ApiError::Core(e)
    }
}

impl From<AnchorError> for ApiError {
    fn from(e: AnchorError) -> Self {
        match e {
            AnchorError::NotConfigured => ApiError::AnchorNotConfigured,
            AnchorError::Rejected(m) => ApiError::AnchorRejected(m),
            AnchorError::Network(m) => ApiError::AnchorNetwork(m),
            // Backend thấy on-chain đã ở seq cao hơn ⇒ đúng nghĩa INV-E7 rollback (409),
            // không phải lỗi hạ tầng: gộp về biến thể lõi để client chỉ học MỘT tên.
            AnchorError::RollbackAttempt {
                on_chain_seq,
                attempted,
            } => ApiError::Core(StrataError::AnchorRollback {
                current: on_chain_seq,
                attempted,
            }),
            // Hai lỗi fail-cứng của backend UTxO — retry vô ích, trả 502 kèm lý do.
            AnchorError::DatumTooLarge { bytes } => {
                ApiError::AnchorRejected(format!("datum {bytes} byte vượt giới hạn tx"))
            }
            AnchorError::InsufficientAda { need, have } => {
                ApiError::AnchorRejected(format!("min-ADA không đủ: cần {need}, có {have}"))
            }
        }
    }
}

fn h(x: &Hash32) -> String {
    hex::encode(x)
}

/// `(status, tên lỗi, detail)` — bảng §3.1 cho phần lõi.
fn split(e: &ApiError) -> (StatusCode, &'static str, Value) {
    match e {
        ApiError::Core(c) => match c {
            // 409 — INV-E1: prev_hash != head (fork / version cũ).
            StrataError::HashLinkBroken { expected, got } => (
                StatusCode::CONFLICT,
                "HashLinkBroken",
                json!({ "expected": h(expected), "got": h(got) }),
            ),
            // 422 — INV-E2: seq nhảy/lùi.
            StrataError::SeqNotMonotonic { expected, got } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "SeqNotMonotonic",
                json!({ "expected": expected, "got": got }),
            ),
            StrataError::SeqOverflow => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "SeqOverflow",
                json!({ "detail": "seq đạt u64::MAX" }),
            ),
            // 403 — INV-E4.
            StrataError::PolicyDenied => (
                StatusCode::FORBIDDEN,
                "PolicyDenied",
                json!({ "detail": "author không nằm trong policy" }),
            ),
            StrataError::BadSignature => (
                StatusCode::FORBIDDEN,
                "BadSignature",
                json!({ "detail": "verify_strict thất bại (sai khoá / malleable / tamper)" }),
            ),
            StrataError::PolicyHashMismatch { expected, got } => (
                StatusCode::FORBIDDEN,
                "PolicyHashMismatch",
                json!({ "expected": h(expected), "got": h(got) }),
            ),
            // 424 — CHỐT-5: key-registry không phân giải được Did.
            StrataError::UnknownAuthor => (
                StatusCode::FAILED_DEPENDENCY,
                "UnknownAuthor",
                json!({ "detail": "key-registry không phân giải được Did → pubkey" }),
            ),
            StrataError::TimestampRegress { prev, got } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "TimestampRegress",
                json!({ "prev": prev, "got": got }),
            ),
            // 409 — INV-E7.
            StrataError::AnchorRollback { current, attempted } => (
                StatusCode::CONFLICT,
                "AnchorRollback",
                json!({ "current": current, "attempted": attempted }),
            ),
            // 403 — INV-E4 field-level (§2.2 V2; §3.1 gộp chung nhóm quyền).
            StrataError::FieldPolicyDenied { field_key } => (
                StatusCode::FORBIDDEN,
                "FieldPolicyDenied",
                json!({ "field_key": String::from_utf8_lossy(field_key) }),
            ),
            StrataError::FieldProofPolicyMismatch { field_key } => (
                StatusCode::FORBIDDEN,
                "FieldProofPolicyMismatch",
                json!({ "field_key": String::from_utf8_lossy(field_key) }),
            ),
        },
        // ── Lỗi mức-cửa (daemon), NGOÀI bảng §3.1 ────────────────────────────
        ApiError::Malformed(m) => (
            StatusCode::BAD_REQUEST,
            "MalformedRequest",
            json!({ "reason": m }),
        ),
        ApiError::NotFound(what) => (StatusCode::NOT_FOUND, "NotFound", json!({ "what": what })),
        ApiError::RefExists(r) => (StatusCode::CONFLICT, "RefExists", json!({ "ref_id": h(r) })),
        ApiError::AnchorNotConfigured => (
            StatusCode::NOT_IMPLEMENTED,
            "AnchorNotConfigured",
            json!({ "detail": "daemon chưa cắm AnchorSink" }),
        ),
        ApiError::AnchorRejected(m) => (
            StatusCode::BAD_GATEWAY,
            "AnchorRejected",
            json!({ "reason": m }),
        ),
        ApiError::AnchorNetwork(m) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "AnchorNetwork",
            json!({ "reason": m }),
        ),
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, name, detail) = split(&self);
        (status, Json(json!({ "error": name, "detail": detail }))).into_response()
    }
}

/// Kết quả handler.
pub type ApiResult<T> = Result<T, ApiError>;
