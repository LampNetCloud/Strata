//! Router + handler — **§3** `Strata-API.md`, style axum (`Router::new().route(...)`).
//!
//! Nguyên tắc xuyên suốt:
//! - Daemon **không ký**: chữ ký do client gửi, daemon gắn vào version rồi để **core** kiểm
//!   (`append_version`/`genesis` gọi `verify_strict`). Không có đường nào ghi mà bỏ qua sig.
//! - Daemon **không băm hộ**: `state_root`, `version_hash`, `mmr_root`, proof đều do core tính.
//! - Mọi ghi đều nằm trong **khoá của riêng ref** (`store::lock`) ⇒ hai request cùng ref
//!   không thể chen nhau tạo fork.

use crate::anchor::backend_name;
use crate::dto::*;
use crate::error::{ApiError, ApiResult};
use crate::hexs;
use crate::registry::KeyRegistry;
use crate::store::{AnchorState, ChainEntry, ChainStore, lock};
use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use ed25519_dalek::Signature;
use lampnet_strata::chain::{Policy, StrataChain, StrataError};
use lampnet_strata::refid::{decode_ref_id, encode_ref_id, gen_ref_id_raw};
use lampnet_strata::state::{build_state_root, prove_field};
use lampnet_strata::version::{Did, Hash32, StrataVersion};
use lampnet_strata::{AnchorPriority, AnchorSink, AuditAction, AuditEntry};
use serde::Deserialize;
use std::sync::Arc;

/// Trạng thái dùng chung của daemon (rẻ để clone — toàn `Arc`).
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<ChainStore>,
    pub registry: Arc<dyn KeyRegistry>,
    pub sink: Arc<dyn AnchorSink + Send + Sync>,
}

impl AppState {
    pub fn new(
        store: Arc<ChainStore>,
        registry: Arc<dyn KeyRegistry>,
        sink: Arc<dyn AnchorSink + Send + Sync>,
    ) -> Self {
        Self {
            store,
            registry,
            sink,
        }
    }
}

/// Router §3. Trả `Router` **mountable** — `lampnet-node` gắn thẳng vào cây route của nó.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/v1/strata/create", post(create))
        // Cùng path, hai method: POST = thêm version; GET ?at= = giá trị tại thời điểm t.
        .route("/v1/strata/:ref/version", post(append).get(version_at))
        .route("/v1/strata/:ref/event", post(event))
        .route("/v1/strata/:ref/head", get(head))
        .route("/v1/strata/:ref/proof/version/:seq", get(proof_version))
        .route("/v1/strata/:ref/proof/field/:key", get(proof_field))
        .route("/v1/strata/:ref/anchor", post(anchor))
        .with_state(state)
}

// ────────────────────────────────────────────────────────────────────────────
// Tiện ích chung
// ────────────────────────────────────────────────────────────────────────────

/// `:ref` nhận **bech32m `lnref1…`** (dạng §2.1 trả ra) hoặc hex 64 ký tự (tiện debug).
fn parse_ref(s: &str) -> ApiResult<Hash32> {
    if let Some(r) = decode_ref_id(s) {
        return Ok(r);
    }
    hexs::decode_fixed::<32>(s).map_err(|e| {
        ApiError::Malformed(format!(
            "ref không hợp lệ (bech32m lnref1… hoặc hex32): {e}"
        ))
    })
}

/// Body JSON hỏng → **400 `MalformedRequest`** theo đúng format lỗi của ta, thay vì lỗi
/// mặc định của axum (client chỉ phải hiểu MỘT format).
fn body<T>(r: Result<Json<T>, JsonRejection>) -> ApiResult<T> {
    r.map(|Json(v)| v)
        .map_err(|e| ApiError::Malformed(e.body_text()))
}

fn action_from_str(s: &str) -> ApiResult<AuditAction> {
    Ok(match s {
        "Create" => AuditAction::Create,
        "Read" => AuditAction::Read,
        "Sign" => AuditAction::Sign,
        "ShareProof" => AuditAction::ShareProof,
        "Update" => AuditAction::Update,
        other => {
            return Err(ApiError::Malformed(format!(
                "action lạ: {other:?} (Create|Read|Sign|ShareProof|Update)"
            )));
        }
    })
}

// ────────────────────────────────────────────────────────────────────────────
// POST /v1/strata/create — §2.1 genesis
// ────────────────────────────────────────────────────────────────────────────

async fn create(
    State(st): State<AppState>,
    req: Result<Json<CreateReq>, JsonRejection>,
) -> ApiResult<Json<CreateResp>> {
    let req = body(req)?;
    let fields = to_pairs(&req.state_fields).map_err(ApiError::Malformed)?;

    // Tập author của policy: mặc định một-thành-viên = người tạo (xem `CreateReq`).
    let authors: Vec<Did> = match &req.policy_authors {
        Some(list) => list
            .iter()
            .map(|s| hexs::decode_fixed::<32>(s))
            .collect::<Result<_, _>>()
            .map_err(|e| ApiError::Malformed(format!("policy_authors: {e}")))?,
        None => vec![req.author_did],
    };

    // CHỐT-5: khoá đến TỪ key-registry, không suy từ Did.
    let mut policy = Policy::new();
    for did in &authors {
        let pk = st
            .registry
            .resolve(did)
            .ok_or(ApiError::Core(StrataError::UnknownAuthor))?;
        policy.allow(*did, pk);
    }
    // Kiểm sớm để thông điệp rõ; `genesis` cũng kiểm lại — trùng nhau là cố ý (fail-closed).
    let ph = policy.policy_hash();
    if ph != req.policy_hash {
        return Err(StrataError::PolicyHashMismatch {
            expected: ph,
            got: req.policy_hash,
        }
        .into());
    }

    let ref_id = gen_ref_id_raw(&req.author_did, &req.genesis_nonce);
    let mut v0 = StrataVersion::unsigned(
        0,
        [0u8; 32],
        req.content_cid.clone(),
        build_state_root(&fields),
        req.author_did,
        req.policy_hash,
        req.ts,
    );
    v0.sig = req.sig; // daemon KHÔNG ký — chỉ gắn chữ ký của client rồi để core kiểm.

    let chain = StrataChain::genesis(ref_id, v0, &policy)?;
    let resp = CreateResp {
        ref_id: encode_ref_id(&ref_id),
        head_seq: chain.head().seq,
        head_version_hash: hex::encode(chain.head_version_hash()),
        mmr_root: hex::encode(chain.mmr_root()),
    };

    st.store
        .insert(ref_id, ChainEntry::new(chain, policy, fields))
        .map_err(|_| ApiError::RefExists(ref_id))?;
    Ok(Json(resp))
}

// ────────────────────────────────────────────────────────────────────────────
// POST /v1/strata/:ref/version — §2.2 append_version
// ────────────────────────────────────────────────────────────────────────────

/// Phần thân dùng chung cho `POST /version` và `POST /event` với `kind="version"` (§2.6 cách 1).
fn append_inner(e: &mut ChainEntry, req: &AppendReq) -> ApiResult<AppendResp> {
    let fields = to_pairs(&req.state_fields).map_err(ApiError::Malformed)?;
    let head = e.chain.head();
    let head_seq = head.seq;
    let head_vh = head.version_hash();

    // `prev_seq` là head client TIN là mình nối vào; lệch ⇒ seq mới lệch (INV-E2).
    if req.prev_seq != head_seq {
        return Err(StrataError::SeqNotMonotonic {
            expected: head_seq,
            got: req.prev_seq,
        }
        .into());
    }
    let seq = head_seq.checked_add(1).ok_or(StrataError::SeqOverflow)?;

    let mut v = StrataVersion::unsigned(
        seq,
        head_vh,
        req.content_cid.clone(),
        build_state_root(&fields),
        req.author_did,
        req.policy_hash,
        req.ts,
    );
    v.sig = req.sig;

    // Core enforce: seq / hash-link / ts / policy_hash / policy / sig / Did (§2.2).
    let policy = e.policy.clone();
    e.chain.append_version(v, &policy)?;
    // Chỉ ghi `fields` SAU khi core đã nhận version — không để state rớt lại khi bị từ chối.
    e.fields.push(fields);

    Ok(AppendResp {
        seq,
        version_hash: hex::encode(e.chain.head_version_hash()),
        mmr_root: hex::encode(e.chain.mmr_root()),
        prev_hash: hex::encode(head_vh),
    })
}

async fn append(
    State(st): State<AppState>,
    Path(r): Path<String>,
    req: Result<Json<AppendReq>, JsonRejection>,
) -> ApiResult<Json<AppendResp>> {
    let req = body(req)?;
    let ref_id = parse_ref(&r)?;
    let entry = st.store.get(&ref_id).ok_or(ApiError::NotFound("ref"))?;
    let mut g = lock(&entry);
    Ok(Json(append_inner(&mut g, &req)?))
}

// ────────────────────────────────────────────────────────────────────────────
// POST /v1/strata/:ref/event — §2.6
// ────────────────────────────────────────────────────────────────────────────

async fn event(
    State(st): State<AppState>,
    Path(r): Path<String>,
    req: Result<Json<EventReq>, JsonRejection>,
) -> ApiResult<axum::response::Response> {
    use axum::response::IntoResponse;
    let req = body(req)?;
    let ref_id = parse_ref(&r)?;
    let entry = st.store.get(&ref_id).ok_or(ApiError::NotFound("ref"))?;
    let mut g = lock(&entry);

    match req {
        // Cách 1: event = một version state-rỗng.
        EventReq::Version(a) => Ok(Json(append_inner(&mut g, &a)?).into_response()),
        // Cách 2: entry vào audit-log (không đẻ version).
        EventReq::Audit(a) => {
            let ae = AuditEntry {
                created_ts: a.ts,
                actor_did: a.actor_did,
                action: action_from_str(&a.action)?,
                signed_hash: a.signed_hash,
                location: a.location,
            };
            // `AuditEntry` không có trường `sig` (chữ ký KHÔNG đi vào leaf, nên không cam kết
            // byte-layout nào cả) — nhưng §3 vẫn bắt gửi `sig`, nên daemon kiểm ở CỬA:
            // Ed25519 trên `canonical()` của chính entry, khoá lấy từ key-registry (CHỐT-5).
            let pk = st
                .registry
                .resolve(&a.actor_did)
                .ok_or(ApiError::Core(StrataError::UnknownAuthor))?;
            // `verify_strict` (không phải `verify`) — cùng độ chặt với core: loại chữ ký
            // malleable / khoá small-order (INV-E4).
            let sig = Signature::from_bytes(&a.sig);
            pk.verify_strict(&ae.canonical(), &sig)
                .map_err(|_| ApiError::Core(StrataError::BadSignature))?;

            let index = g.audit.append_access(ae)?;
            let log_root = hex::encode(g.audit.root());
            Ok(Json(AuditEventResp { index, log_root }).into_response())
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// GET /v1/strata/:ref/head — §2.3
// ────────────────────────────────────────────────────────────────────────────

async fn head(State(st): State<AppState>, Path(r): Path<String>) -> ApiResult<Json<HeadResp>> {
    let ref_id = parse_ref(&r)?;
    let entry = st.store.get(&ref_id).ok_or(ApiError::NotFound("ref"))?;
    let g = lock(&entry);
    let h = g.chain.head();
    Ok(Json(HeadResp {
        ref_id: encode_ref_id(&ref_id),
        head_seq: h.seq,
        head_version_hash: hex::encode(h.version_hash()),
        mmr_root: hex::encode(g.chain.mmr_root()),
        content_cid: hex::encode(&h.content_cid),
    }))
}

// ────────────────────────────────────────────────────────────────────────────
// GET /v1/strata/:ref/version?at=<unix_ts> — §2.4
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AtQuery {
    at: u64,
}

async fn version_at(
    State(st): State<AppState>,
    Path(r): Path<String>,
    q: Result<Query<AtQuery>, axum::extract::rejection::QueryRejection>,
) -> ApiResult<Json<VersionAtResp>> {
    let Query(q) =
        q.map_err(|e| ApiError::Malformed(format!("thiếu/sai `at`: {}", e.body_text())))?;
    let ref_id = parse_ref(&r)?;
    let entry = st.store.get(&ref_id).ok_or(ApiError::NotFound("ref"))?;
    let g = lock(&entry);
    // `None` khi t < ts(genesis) — không có version nào "đang sống" tại t đó.
    let (v, proof) = g
        .chain
        .version_at(q.at)
        .ok_or(ApiError::NotFound("version tại t"))?;
    Ok(Json(VersionAtResp {
        seq: v.seq,
        version: v.into(),
        proof: ProofDto::new(v.seq, v.version_hash(), g.chain.len() as u64, &proof),
    }))
}

// ────────────────────────────────────────────────────────────────────────────
// GET /v1/strata/:ref/proof/version/:seq — §2.5 (INV-E3)
// ────────────────────────────────────────────────────────────────────────────

async fn proof_version(
    State(st): State<AppState>,
    Path((r, seq)): Path<(String, u64)>,
) -> ApiResult<Json<ProofDto>> {
    let ref_id = parse_ref(&r)?;
    let entry = st.store.get(&ref_id).ok_or(ApiError::NotFound("ref"))?;
    let g = lock(&entry);
    let (proof, size, vh) = g
        .chain
        .prove_version(seq)
        .ok_or(ApiError::NotFound("seq"))?;
    Ok(Json(ProofDto::new(seq, vh, size, &proof)))
}

// ────────────────────────────────────────────────────────────────────────────
// GET /v1/strata/:ref/proof/field/:key — §2.5 (INV-E6)
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct SeqQuery {
    /// Vắng ⇒ head (§3 ví dụ trả `version_seq` của head).
    seq: Option<u64>,
}

async fn proof_field(
    State(st): State<AppState>,
    Path((r, key)): Path<(String, String)>,
    Query(q): Query<SeqQuery>,
) -> ApiResult<Json<FieldProofResp>> {
    let ref_id = parse_ref(&r)?;
    let entry = st.store.get(&ref_id).ok_or(ApiError::NotFound("ref"))?;
    let g = lock(&entry);
    let seq = q.seq.unwrap_or_else(|| g.chain.head().seq);
    let fields = g.fields_at(seq).ok_or(ApiError::NotFound("seq"))?;
    let fp = prove_field(fields, key.as_bytes()).ok_or(ApiError::NotFound("field key"))?;
    Ok(Json(FieldProofResp::new(&fp, seq)))
}

// ────────────────────────────────────────────────────────────────────────────
// POST /v1/strata/:ref/anchor — §2.7 + §4
// ────────────────────────────────────────────────────────────────────────────

async fn anchor(
    State(st): State<AppState>,
    Path(r): Path<String>,
    req: Result<Json<AnchorReq>, JsonRejection>,
) -> ApiResult<Json<AnchorResp>> {
    let req = body(req)?;
    let ref_id = parse_ref(&r)?;
    // `sink.publish` là I/O đồng bộ (có thể chờ mạng/chuỗi) và ta phải giữ khoá của ref
    // trong suốt chuỗi kiểm→đẩy→chốt ⇒ đẩy cả khối sang thread blocking, không chiếm
    // thread của runtime async.
    tokio::task::spawn_blocking(move || anchor_blocking(&st, ref_id, req.priority.into()))
        .await
        .map_err(|e| ApiError::Malformed(format!("tác vụ neo hỏng: {e}")))?
        .map(Json)
}

/// Trình tự neo — **thứ tự này là cố ý**:
///
/// 1. Kiểm rollback bằng **gương** `anchored.seq` của daemon (INV-E7) — chưa đụng lõi.
/// 2. Đẩy on-chain qua `AnchorSink`.
/// 3. Thành công mới gọi `chain.publish_anchor()` để chốt `last_anchor_seq` ở lõi.
///
/// Nếu làm ngược (gọi `publish_anchor()` trước rồi mới đẩy), backend hỏng ở bước 2 sẽ để
/// lại `last_anchor_seq` đã tăng mà on-chain KHÔNG có gì — mọi lần thử lại sau đó đều
/// `AnchorRollback` và ref chết vĩnh viễn. Cả ba bước nằm trong khoá của ref nên không có
/// cửa sổ đua nào chen vào giữa.
fn anchor_blocking(
    st: &AppState,
    ref_id: Hash32,
    priority: AnchorPriority,
) -> ApiResult<AnchorResp> {
    let entry = st.store.get(&ref_id).ok_or(ApiError::NotFound("ref"))?;
    let mut g = lock(&entry);

    let a = g.chain.anchor(); // read-only, KHÔNG đụng last_anchor_seq
    if let Some(prev) = &g.anchored
        && a.seq <= prev.seq
    {
        return Err(StrataError::AnchorRollback {
            current: prev.seq,
            attempted: a.seq,
        }
        .into());
    }

    // `no_anchor` = sống ở tầng (a)/(b), KHÔNG đẩy và KHÔNG chốt seq đã neo (§4.2).
    if matches!(priority, AnchorPriority::NoAnchor) {
        return Ok(AnchorResp::new(&a, None, None));
    }

    let receipt = st.sink.publish(&a, priority)?;
    let committed = g.chain.publish_anchor()?; // chốt INV-E7 ở lõi sau khi on-chain đã nhận
    let (txid, backend) = match &receipt {
        Some(rc) => (
            Some(rc.txid.clone()),
            Some(backend_name(rc.backend).to_string()),
        ),
        None => (None, None),
    };
    g.anchored = Some(AnchorState {
        seq: committed.seq,
        txid: txid.clone(),
        backend: backend.clone(),
    });
    Ok(AnchorResp::new(&committed, txid, backend))
}
