//! Test đầu-cuối lớp cửa HTTP §3 — gọi router thật qua `oneshot`, không mở cổng mạng.
//!
//! Ba nhóm:
//! 1. **Đường sống** — create → version → head → proof (và proof lấy về verify ĐƯỢC bằng
//!    chính hàm verify của core, không chỉ "có trả về JSON").
//! 2. **Bảng lỗi §3.1** — mỗi biến thể một test, kiểm cả HTTP code lẫn tên lỗi.
//! 3. **Neo (§2.7 + §4)** — thứ tự kiểm→đẩy→chốt: rollback bị chặn, và **backend hỏng
//!    KHÔNG làm cháy ref**.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::SigningKey;
use http_body_util::BodyExt;
use lampnet_merkle_anchor::mmr::InclusionProof;
use lampnet_strata::state::{FieldProof, build_state_root, verify_field_proof};
use lampnet_strata::version::{Hash32, StrataVersion};
use lampnet_strata::{Policy, StrataChain};
use lampnet_strata_node::{
    AppState, ChainStore, FailingSink, InMemoryRegistry, MemorySink, router,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

// ── Khung ───────────────────────────────────────────────────────────────────

const DID: [u8; 32] = [0x11; 32];
const DID2: [u8; 32] = [0x22; 32];

fn sk(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

/// App + policy một-thành-viên (DID) đã đăng ký khoá. `sink` chọn theo test.
fn app_with(sink: Arc<dyn lampnet_strata::AnchorSink + Send + Sync>) -> (Router, Policy) {
    let reg = InMemoryRegistry::new();
    reg.register(DID, sk(1).verifying_key());
    reg.register(DID2, sk(2).verifying_key());
    let mut policy = Policy::new();
    policy.allow(DID, sk(1).verifying_key());
    let state = AppState::new(Arc::new(ChainStore::new()), Arc::new(reg), sink);
    (router(state), policy)
}

fn app() -> (Router, Policy) {
    app_with(Arc::new(MemorySink::new()))
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let req = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(v) => req
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, v)
}

/// Ký như client: dựng đúng version rồi lấy `sig` (daemon KHÔNG ký hộ).
#[allow(clippy::too_many_arguments)]
fn sign_version(
    seed: u8,
    seq: u64,
    prev_hash: Hash32,
    content_cid: &[u8],
    fields: &[(Vec<u8>, Vec<u8>)],
    did: [u8; 32],
    policy_hash: Hash32,
    ts: u64,
) -> String {
    let mut v = StrataVersion::unsigned(
        seq,
        prev_hash,
        content_cid.to_vec(),
        build_state_root(fields),
        did,
        policy_hash,
        ts,
    );
    v.sign(&sk(seed));
    hex::encode(v.sig)
}

fn f(key: &str, value: &str) -> (Vec<u8>, Vec<u8>) {
    (key.as_bytes().to_vec(), hex::decode(value).unwrap())
}

const V_A: &str = "aa00000000000000000000000000000000000000000000000000000000000001";
const V_B: &str = "bb00000000000000000000000000000000000000000000000000000000000002";

/// `create` chuẩn; trả `(ref_id bech32, head_version_hash)`.
async fn create_ok(app: &Router, policy: &Policy) -> (String, Hash32) {
    let fields = vec![f("diagnosis", V_A)];
    let sig = sign_version(
        1,
        0,
        [0u8; 32],
        b"\xca\xfe",
        &fields,
        DID,
        policy.policy_hash(),
        1_000,
    );
    let (st, body) = call(
        app,
        "POST",
        "/v1/strata/create",
        Some(json!({
            "author_did": hex::encode(DID),
            "genesis_nonce": hex::encode([0x33u8; 32]),
            "content_cid": "cafe",
            "state_fields": [{ "key": "diagnosis", "value": V_A }],
            "policy_hash": hex::encode(policy.policy_hash()),
            "ts": 1_000,
            "sig": sig
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "create: {body}");
    let r = body["ref_id"].as_str().unwrap().to_string();
    let vh = hex::decode(body["head_version_hash"].as_str().unwrap()).unwrap();
    (r, vh.try_into().unwrap())
}

/// Body của một version tiếp theo (seq = prev_seq+1).
fn append_body(
    seed: u8,
    did: [u8; 32],
    prev_seq: u64,
    prev_hash: Hash32,
    ts: u64,
    policy: &Policy,
    value: &str,
) -> Value {
    let fields = vec![f("diagnosis", value)];
    let sig = sign_version(
        seed,
        prev_seq + 1,
        prev_hash,
        b"\xbe\xef",
        &fields,
        did,
        policy.policy_hash(),
        ts,
    );
    json!({
        "prev_seq": prev_seq,
        "content_cid": "beef",
        "state_fields": [{ "key": "diagnosis", "value": value }],
        "author_did": hex::encode(did),
        "policy_hash": hex::encode(policy.policy_hash()),
        "ts": ts,
        "sig": sig
    })
}

fn proof_from(v: &Value) -> (InclusionProof, u64, Hash32, u64) {
    let siblings = v["siblings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let h: Hash32 = hex::decode(s[0].as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap();
            (h, s[1].as_bool().unwrap())
        })
        .collect();
    let peaks = v["peaks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            hex::decode(p.as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap()
        })
        .collect();
    let leaf: Hash32 = hex::decode(v["leaf_hash"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    (
        InclusionProof {
            siblings,
            peak_index: v["peak_index"].as_u64().unwrap() as usize,
            peaks,
        },
        v["mmr_size"].as_u64().unwrap(),
        leaf,
        v["leaf_seq"].as_u64().unwrap(),
    )
}

// ── 1. Đường sống ───────────────────────────────────────────────────────────

#[tokio::test]
async fn create_append_head_and_proofs_round_trip() {
    let (app, policy) = app();
    let (r, vh0) = create_ok(&app, &policy).await;

    // head sau genesis
    let (st, h) = call(&app, "GET", &format!("/v1/strata/{r}/head"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(h["head_seq"], 0);
    assert_eq!(h["content_cid"], "cafe");
    assert_eq!(h["ref_id"].as_str().unwrap(), r);

    // append seq=1
    let (st, a) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/version"),
        Some(append_body(1, DID, 0, vh0, 2_000, &policy, V_B)),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "append: {a}");
    assert_eq!(a["seq"], 1);
    assert_eq!(a["prev_hash"].as_str().unwrap(), hex::encode(vh0));

    let (_, h) = call(&app, "GET", &format!("/v1/strata/{r}/head"), None).await;
    assert_eq!(h["head_seq"], 1);
    let root: Hash32 = hex::decode(h["mmr_root"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();

    // proof/version/0 — verify THẬT dưới mmr_root hiện tại (INV-E3: proof cũ vẫn đúng dưới root mới)
    let (st, p) = call(
        &app,
        "GET",
        &format!("/v1/strata/{r}/proof/version/0"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (proof, size, leaf, seq) = proof_from(&p);
    assert_eq!(leaf, vh0);
    assert!(StrataChain::verify_version(root, &leaf, seq, size, &proof));

    // proof/field — verify bằng chính hàm của core
    let (st, fp) = call(
        &app,
        "GET",
        &format!("/v1/strata/{r}/proof/field/diagnosis"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(fp["version_seq"], 1);
    assert_eq!(fp["value"].as_str().unwrap(), V_B); // head, không phải genesis
    let core_fp = FieldProof {
        key: fp["key"].as_str().unwrap().as_bytes().to_vec(),
        value: hex::decode(fp["value"].as_str().unwrap()).unwrap(),
        fvh: hex::decode(fp["fvh"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap(),
        salt: hex::decode(fp["salt"].as_str().unwrap()).unwrap(),
        siblings: fp["siblings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                let h: Hash32 = hex::decode(s[0].as_str().unwrap())
                    .unwrap()
                    .try_into()
                    .unwrap();
                (h, s[1].as_bool().unwrap())
            })
            .collect(),
        state_root: hex::decode(fp["state_root"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap(),
    };
    assert!(verify_field_proof(&core_fp));

    // proof/field?seq=0 — lịch sử vẫn tra được (giá trị CŨ)
    let (_, fp0) = call(
        &app,
        "GET",
        &format!("/v1/strata/{r}/proof/field/diagnosis?seq=0"),
        None,
    )
    .await;
    assert_eq!(fp0["value"].as_str().unwrap(), V_A);

    // version?at — giá trị tại thời điểm t (§2.4)
    let (st, at) = call(
        &app,
        "GET",
        &format!("/v1/strata/{r}/version?at=1500"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(at["seq"], 0, "t=1500 nằm giữa ts 1000 và 2000 ⇒ version 0");
    assert_eq!(
        at["version"]["version_hash"].as_str().unwrap(),
        hex::encode(vh0)
    );
    let (proof, size, leaf, seq) = proof_from(&at["proof"]);
    assert!(StrataChain::verify_version(root, &leaf, seq, size, &proof));

    // t trước genesis ⇒ 404 (không có version nào sống ở đó)
    let (st, _) = call(&app, "GET", &format!("/v1/strata/{r}/version?at=999"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn event_audit_appends_to_log_and_event_version_appends_chain() {
    let (app, policy) = app();
    let (r, vh0) = create_ok(&app, &policy).await;

    // kind=audit — entry vào AuditLog, KHÔNG đẻ version
    let entry = lampnet_strata::AuditEntry {
        created_ts: 1_500,
        actor_did: DID,
        action: lampnet_strata::AuditAction::Read,
        signed_hash: [0x44; 32],
        location: [0x55; 32],
    };
    let sig = hex::encode(ed25519_dalek::Signer::sign(&sk(1), &entry.canonical()).to_bytes());
    let (st, ev) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/event"),
        Some(json!({
            "kind": "audit", "actor_did": hex::encode(DID), "action": "Read",
            "signed_hash": hex::encode([0x44u8; 32]), "location": hex::encode([0x55u8; 32]),
            "ts": 1_500, "sig": sig
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "event audit: {ev}");
    assert_eq!(ev["index"], 0);
    assert_ne!(ev["log_root"].as_str().unwrap(), hex::encode([0u8; 32]));

    let (_, h) = call(&app, "GET", &format!("/v1/strata/{r}/head"), None).await;
    assert_eq!(h["head_seq"], 0, "audit event KHÔNG được đẻ version");

    // kind=version — event là một version (§2.6 cách 1)
    let mut b = append_body(1, DID, 0, vh0, 2_000, &policy, V_B);
    b["kind"] = json!("version");
    let (st, ev) = call(&app, "POST", &format!("/v1/strata/{r}/event"), Some(b)).await;
    assert_eq!(st, StatusCode::OK, "event version: {ev}");
    assert_eq!(ev["seq"], 1);
}

// ── 2. Bảng lỗi §3.1 ────────────────────────────────────────────────────────

#[tokio::test]
async fn create_with_unregistered_did_is_424_unknown_author() {
    let (app, _) = app();
    let unknown = [0x99u8; 32];
    let mut p = Policy::new();
    p.allow(unknown, sk(3).verifying_key());
    let (st, b) = call(
        &app,
        "POST",
        "/v1/strata/create",
        Some(json!({
            "author_did": hex::encode(unknown),
            "genesis_nonce": hex::encode([0x33u8; 32]),
            "content_cid": "cafe", "state_fields": [],
            "policy_hash": hex::encode(p.policy_hash()),
            "ts": 1, "sig": hex::encode([0u8; 64])
        })),
    )
    .await;
    assert_eq!(st, StatusCode::FAILED_DEPENDENCY);
    assert_eq!(b["error"], "UnknownAuthor");
}

#[tokio::test]
async fn create_with_wrong_policy_hash_is_403() {
    let (app, _) = app();
    let (st, b) = call(
        &app,
        "POST",
        "/v1/strata/create",
        Some(json!({
            "author_did": hex::encode(DID),
            "genesis_nonce": hex::encode([0x33u8; 32]),
            "content_cid": "cafe", "state_fields": [],
            "policy_hash": hex::encode([0xEEu8; 32]),   // không phải hash của policy thật
            "ts": 1, "sig": hex::encode([0u8; 64])
        })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(b["error"], "PolicyHashMismatch");
}

#[tokio::test]
async fn create_twice_same_nonce_is_409_ref_exists() {
    let (app, policy) = app();
    create_ok(&app, &policy).await;
    let fields = vec![f("diagnosis", V_A)];
    let sig = sign_version(
        1,
        0,
        [0u8; 32],
        b"\xca\xfe",
        &fields,
        DID,
        policy.policy_hash(),
        1_000,
    );
    let (st, b) = call(
        &app,
        "POST",
        "/v1/strata/create",
        Some(json!({
            "author_did": hex::encode(DID),
            "genesis_nonce": hex::encode([0x33u8; 32]),
            "content_cid": "cafe",
            "state_fields": [{ "key": "diagnosis", "value": V_A }],
            "policy_hash": hex::encode(policy.policy_hash()),
            "ts": 1_000, "sig": sig
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(b["error"], "RefExists");
}

#[tokio::test]
async fn append_with_bad_signature_is_403() {
    let (app, policy) = app();
    let (r, vh0) = create_ok(&app, &policy).await;
    // Ký bằng khoá của DID2 nhưng khai author = DID.
    let mut b = append_body(2, DID, 0, vh0, 2_000, &policy, V_B);
    b["author_did"] = json!(hex::encode(DID));
    let (st, e) = call(&app, "POST", &format!("/v1/strata/{r}/version"), Some(b)).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(e["error"], "BadSignature");
}

#[tokio::test]
async fn append_by_author_outside_policy_is_403_policy_denied() {
    let (app, policy) = app();
    let (r, vh0) = create_ok(&app, &policy).await;
    // DID2 có khoá trong registry nhưng KHÔNG nằm trong policy của ref này.
    let b = append_body(2, DID2, 0, vh0, 2_000, &policy, V_B);
    let (st, e) = call(&app, "POST", &format!("/v1/strata/{r}/version"), Some(b)).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(e["error"], "PolicyDenied");
}

#[tokio::test]
async fn append_with_stale_prev_seq_is_422_seq_not_monotonic() {
    let (app, policy) = app();
    let (r, vh0) = create_ok(&app, &policy).await;
    let mut b = append_body(1, DID, 0, vh0, 2_000, &policy, V_B);
    b["prev_seq"] = json!(7); // head thật đang là 0
    let (st, e) = call(&app, "POST", &format!("/v1/strata/{r}/version"), Some(b)).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(e["error"], "SeqNotMonotonic");
    assert_eq!(e["detail"]["expected"], 0);
    assert_eq!(e["detail"]["got"], 7);
}

#[tokio::test]
async fn append_with_regressing_ts_is_422() {
    let (app, policy) = app();
    let (r, vh0) = create_ok(&app, &policy).await;
    let b = append_body(1, DID, 0, vh0, 500, &policy, V_B); // genesis ts = 1000
    let (st, e) = call(&app, "POST", &format!("/v1/strata/{r}/version"), Some(b)).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(e["error"], "TimestampRegress");
}

#[tokio::test]
async fn audit_event_with_bad_signature_is_403() {
    let (app, policy) = app();
    let (r, _) = create_ok(&app, &policy).await;
    let (st, e) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/event"),
        Some(json!({
            "kind": "audit", "actor_did": hex::encode(DID), "action": "Read",
            "signed_hash": hex::encode([0x44u8; 32]), "location": hex::encode([0x55u8; 32]),
            "ts": 1_500, "sig": hex::encode([7u8; 64])
        })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(e["error"], "BadSignature");
}

#[tokio::test]
async fn unknown_ref_and_seq_and_key_are_404() {
    let (app, policy) = app();
    let (r, _) = create_ok(&app, &policy).await;
    let other = lampnet_strata::refid::encode_ref_id(&[0x77u8; 32]);

    let (st, _) = call(&app, "GET", &format!("/v1/strata/{other}/head"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = call(
        &app,
        "GET",
        &format!("/v1/strata/{r}/proof/version/9"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, b) = call(
        &app,
        "GET",
        &format!("/v1/strata/{r}/proof/field/khong-co"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    assert_eq!(b["error"], "NotFound");
}

#[tokio::test]
async fn malformed_inputs_are_400_in_our_error_format() {
    let (app, policy) = app();
    let (r, _) = create_ok(&app, &policy).await;

    // JSON hỏng
    let bad = Request::builder()
        .method("POST")
        .uri("/v1/strata/create")
        .header("content-type", "application/json")
        .body(Body::from("{ khong-phai-json"))
        .unwrap();
    let resp = app.clone().oneshot(bad).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["error"], "MalformedRequest");

    // `at` thiếu
    let (st, e) = call(&app, "GET", &format!("/v1/strata/{r}/version"), None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(e["error"], "MalformedRequest");

    // ref không phải bech32m/hex32
    let (st, e) = call(&app, "GET", "/v1/strata/khong-phai-ref/head", None).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(e["error"], "MalformedRequest");

    // `seq` không phải số — cũng phải ra format lỗi của ta, không phải của axum
    let (st, e) = call(
        &app,
        "GET",
        &format!("/v1/strata/{r}/proof/field/name?seq=khong-phai-so"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(e["error"], "MalformedRequest");
}

// ── 3. Neo (§2.7 + §4) ──────────────────────────────────────────────────────

#[tokio::test]
async fn anchor_happy_then_rollback_then_new_version_anchors_again() {
    let (app, policy) = app();
    let (r, vh0) = create_ok(&app, &policy).await;

    let (st, a) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/anchor"),
        Some(json!({ "priority": "immediate" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "anchor: {a}");
    assert_eq!(a["seq"], 0);
    assert_eq!(a["backend"], "settlement");
    assert!(a["anchor_txid"].as_str().unwrap().starts_with("memory-"));

    // Neo lại khi head chưa đổi ⇒ INV-E7 rollback (409), KHÔNG đẩy on-chain lần nữa.
    let (st, e) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/anchor"),
        Some(json!({ "priority": "immediate" })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert_eq!(e["error"], "AnchorRollback");
    assert_eq!(e["detail"]["current"], 0);

    // Có version mới ⇒ neo lại được.
    let (st, _) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/version"),
        Some(append_body(1, DID, 0, vh0, 2_000, &policy, V_B)),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, a2) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/anchor"),
        Some(json!({ "priority": "immediate" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(a2["seq"], 1);
}

#[tokio::test]
async fn no_anchor_priority_returns_null_txid_and_does_not_consume_seq() {
    let (app, policy) = app();
    let (r, _) = create_ok(&app, &policy).await;

    let (st, a) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/anchor"),
        Some(json!({ "priority": "no_anchor" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(a["anchor_txid"].is_null());
    assert!(a["backend"].is_null());

    // `no_anchor` KHÔNG chốt seq ⇒ neo thật ngay sau đó vẫn phải đi được.
    let (st, a2) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/anchor"),
        Some(json!({ "priority": "milestone" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{a2}");
    assert_eq!(a2["seq"], 0);
}

#[tokio::test]
async fn failing_backend_does_not_burn_the_ref() {
    // Nếu daemon gọi `publish_anchor()` TRƯỚC khi đẩy on-chain, lần thử thứ hai sẽ là
    // 409 AnchorRollback dù chưa hề có gì trên chain — ref chết vĩnh viễn. Thứ tự đúng
    // (kiểm → đẩy → chốt) phải cho ra 503 cả hai lần.
    let (app, policy) = app_with(Arc::new(FailingSink));
    let (r, _) = create_ok(&app, &policy).await;

    for _ in 0..2 {
        let (st, e) = call(
            &app,
            "POST",
            &format!("/v1/strata/{r}/anchor"),
            Some(json!({ "priority": "immediate" })),
        )
        .await;
        assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(e["error"], "AnchorNetwork");
    }
}

#[tokio::test]
async fn anchor_without_backend_is_501_but_no_anchor_still_ok() {
    let (app, policy) = app_with(Arc::new(lampnet_strata_node::DisabledSink));
    let (r, _) = create_ok(&app, &policy).await;

    let (st, e) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/anchor"),
        Some(json!({ "priority": "immediate" })),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(e["error"], "AnchorNotConfigured");

    let (st, _) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/anchor"),
        Some(json!({ "priority": "no_anchor" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
}

// ════════════════════════════════════════════════════════════════════════════
// Cổng cửa 2026-08-11 — mỗi test khoá lại MỘT lỗ đã dựng lại được bằng PoC
// ════════════════════════════════════════════════════════════════════════════

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// `ts` mili giây (`Date.now()`) bị chặn Ở CỬA.
///
/// Không chặn thì lõi NHẬN nó (lõi chỉ ép không-giảm, không có đồng hồ), và từ đó mọi
/// version với `ts` giây thật đều `TimestampRegress` — ref mất quyền ghi tới năm 56000.
#[tokio::test]
async fn ts_in_milliseconds_rejected_at_the_door() {
    let (app, policy) = app();
    let ms = now() * 1_000; // đúng thứ Date.now() trả
    let fields = vec![f("diagnosis", V_A)];
    let sig = sign_version(
        1,
        0,
        [0u8; 32],
        b"\xca\xfe",
        &fields,
        DID,
        policy.policy_hash(),
        ms,
    );
    let (st, body) = call(
        &app,
        "POST",
        "/v1/strata/create",
        Some(json!({
            "author_did": hex::encode(DID),
            "genesis_nonce": hex::encode([0x33u8; 32]),
            "content_cid": "cafe",
            "state_fields": [{ "key": "diagnosis", "value": V_A }],
            "policy_hash": hex::encode(policy.policy_hash()),
            "ts": ms,
            "sig": sig
        })),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"], "TimestampTooFarFuture");

    // Đối chứng: `ts` giây hợp lệ vẫn qua. `now()` gọi ĐÚNG MỘT lần rồi dùng lại — gọi hai
    // lần thì một lần rơi qua mốc giây làm chữ ký phủ `ts` khác ⇒ 403 ngẫu nhiên trên CI.
    let t = now();
    let (st_ok, _) = call(
        &app,
        "POST",
        "/v1/strata/create",
        Some(json!({
            "author_did": hex::encode(DID),
            "genesis_nonce": hex::encode([0x33u8; 32]),
            "content_cid": "cafe",
            "state_fields": [{ "key": "diagnosis", "value": V_A }],
            "policy_hash": hex::encode(policy.policy_hash()),
            "ts": t,
            "sig": sign_version(1, 0, [0u8; 32], b"\xca\xfe", &fields, DID, policy.policy_hash(), t)
        })),
    )
    .await;
    assert_eq!(st_ok, StatusCode::OK, "ts giây hợp lệ KHÔNG được bị chặn");
}

/// Trần tuyệt đối phải chặn được KỂ CẢ khi đồng hồ daemon vô dụng.
///
/// `check_ts` có hai lớp; lớp biên-lệch phụ thuộc `now_secs()`. Nếu chỉ có lớp đó thì một
/// máy không RTC (đồng hồ boot ở epoch) làm guard tắt im lặng đúng lúc cần nhất. Test này
/// khoá lớp thứ nhất: một `ts` mili giây bị từ chối vì nó vượt `TS_MILLIS_FLOOR`, không phải
/// vì nó xa `now` — nên nó đúng bất kể đồng hồ.
#[tokio::test]
async fn ts_in_milliseconds_rejected_by_absolute_ceiling_not_by_clock() {
    let (app, policy) = app();
    // 10^12 + 1 giây = năm 33658 — vượt trần tuyệt đối, nhưng KHÔNG phải `Date.now()` hôm nay.
    let far = 1_000_000_000_001u64;
    let fields = vec![f("diagnosis", V_A)];
    let sig = sign_version(
        1,
        0,
        [0u8; 32],
        b"\xca\xfe",
        &fields,
        DID,
        policy.policy_hash(),
        far,
    );
    let (st, body) = call(
        &app,
        "POST",
        "/v1/strata/create",
        Some(json!({
            "author_did": hex::encode(DID),
            "genesis_nonce": hex::encode([0x44u8; 32]),
            "content_cid": "cafe",
            "state_fields": [{ "key": "diagnosis", "value": V_A }],
            "policy_hash": hex::encode(policy.policy_hash()),
            "ts": far,
            "sig": sig
        })),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"], "TimestampTooFarFuture");
}

/// Khoá trường TRÙNG bị từ chối — nếu không, `state_root` đã ký + đã neo cam kết HAI giá trị
/// mâu thuẫn cho cùng một khoá, và tồn tại hai field-proof đều verify đúng.
#[tokio::test]
async fn duplicate_field_key_rejected() {
    let (app, policy) = app();
    let fields = vec![f("diagnosis", V_A)];
    let sig = sign_version(
        1,
        0,
        [0u8; 32],
        b"\xca\xfe",
        &fields,
        DID,
        policy.policy_hash(),
        1_000,
    );
    let (st, body) = call(
        &app,
        "POST",
        "/v1/strata/create",
        Some(json!({
            "author_did": hex::encode(DID),
            "genesis_nonce": hex::encode([0x33u8; 32]),
            "content_cid": "cafe",
            "state_fields": [
                { "key": "diagnosis", "value": V_A },
                { "key": "diagnosis", "value": V_B }
            ],
            "policy_hash": hex::encode(policy.policy_hash()),
            "ts": 1_000,
            "sig": sig
        })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "MalformedRequest");
    assert!(
        body["detail"]["reason"]
            .as_str()
            .unwrap()
            .contains("khoá trùng"),
        "thông điệp phải nói rõ nguyên nhân: {body}"
    );
}

/// Người NGOÀI policy không ghi được vào nhật ký truy cập của hồ sơ người khác.
///
/// Trước đây đường audit chỉ `verify_strict` bằng key-registry TOÀN CỤC — xác thực đúng,
/// nhưng đúng cho sai hồ sơ. DID2 có khoá trong registry mà KHÔNG có trong policy.
#[tokio::test]
async fn audit_from_outsider_denied_and_cannot_poison_ts() {
    let (app, policy) = app();
    let (r, _) = create_ok(&app, &policy).await;

    let entry = |did: [u8; 32], seed: u8, ts: u64| {
        let ae = lampnet_strata::AuditEntry {
            created_ts: ts,
            actor_did: did,
            action: lampnet_strata::AuditAction::Read,
            signed_hash: [0x22; 32],
            location: [0x33; 32],
        };
        use ed25519_dalek::Signer;
        let s = sk(seed).sign(&ae.canonical());
        json!({
            "kind": "audit",
            "actor_did": hex::encode(did),
            "action": "Read",
            "signed_hash": hex::encode([0x22u8; 32]),
            "location": hex::encode([0x33u8; 32]),
            "ts": ts,
            "sig": hex::encode(s.to_bytes())
        })
    };

    // DID2 = ngoài policy → 403, KHÔNG phải 200.
    let (st, body) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/event"),
        Some(entry(DID2, 2, 1_000)),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "PolicyDenied");

    // Và không đầu độc được `last_ts` bằng u64::MAX. Chặn ở cổng policy — `check_ts` nằm SAU
    // nên nó không chạy cho người ngoài; trần ts được phủ riêng ở test dưới.
    let (st_max, _) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/event"),
        Some(entry(DID2, 2, u64::MAX)),
    )
    .await;
    assert_eq!(st_max, StatusCode::FORBIDDEN);

    // Chính chủ vẫn ghi được — log KHÔNG bị khoá.
    let (st_owner, body_owner) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/event"),
        Some(entry(DID, 1, 2_000)),
    )
    .await;
    assert_eq!(st_owner, StatusCode::OK, "{body_owner}");
    assert_eq!(body_owner["index"], 0, "mục đầu tiên phải là của chính chủ");
}

/// Chính chủ cũng không đầu độc được `ts` của audit-log (trần ts áp cho mọi actor).
#[tokio::test]
async fn audit_ts_far_future_rejected_even_for_owner() {
    let (app, policy) = app();
    let (r, _) = create_ok(&app, &policy).await;
    let ae = lampnet_strata::AuditEntry {
        created_ts: u64::MAX,
        actor_did: DID,
        action: lampnet_strata::AuditAction::Read,
        signed_hash: [0x22; 32],
        location: [0x33; 32],
    };
    use ed25519_dalek::Signer;
    let s = sk(1).sign(&ae.canonical());
    let (st, body) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/event"),
        Some(json!({
            "kind": "audit",
            "actor_did": hex::encode(DID),
            "action": "Read",
            "signed_hash": hex::encode([0x22u8; 32]),
            "location": hex::encode([0x33u8; 32]),
            "ts": u64::MAX,
            "sig": hex::encode(s.to_bytes())
        })),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["error"], "TimestampTooFarFuture");
}

/// Route KHÔ trả ĐÚNG byte mà đường ghi thật dùng — nếu lệch thì nó vô dụng.
#[tokio::test]
async fn canonical_dry_route_matches_the_real_write_path() {
    let (app, policy) = app();
    let nonce = [0x33u8; 32];

    let (st, dry) = call(
        &app,
        "POST",
        "/v1/strata/_canonical",
        Some(json!({
            "seq": 0,
            "prev_hash": hex::encode([0u8; 32]),
            "content_cid": "cafe",
            "state_fields": [{ "key": "diagnosis", "value": V_A }],
            "author_did": hex::encode(DID),
            "policy_hash": hex::encode(policy.policy_hash()),
            "ts": 1_000,
            "genesis_nonce": hex::encode(nonce)
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{dry}");

    // `state_root` khô == core tính từ cùng fields.
    let fields = vec![f("diagnosis", V_A)];
    assert_eq!(dry["state_root"], hex::encode(build_state_root(&fields)));

    // Ký ĐÚNG version_hash route khô trả về, rồi ghi thật → phải qua.
    let vh: Hash32 = hex::decode(dry["version_hash"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    use ed25519_dalek::Signer;
    let sig = hex::encode(sk(1).sign(&vh).to_bytes());

    let (st2, body2) = call(
        &app,
        "POST",
        "/v1/strata/create",
        Some(json!({
            "author_did": hex::encode(DID),
            "genesis_nonce": hex::encode(nonce),
            "content_cid": "cafe",
            "state_fields": [{ "key": "diagnosis", "value": V_A }],
            "policy_hash": hex::encode(policy.policy_hash()),
            "ts": 1_000,
            "sig": sig
        })),
    )
    .await;
    assert_eq!(
        st2,
        StatusCode::OK,
        "ký trên version_hash của route khô phải ghi được: {body2}"
    );

    // ref_id khô == ref_id thật, và head_version_hash thật == version_hash khô.
    assert_eq!(dry["ref_id"], body2["ref_id"]);
    assert_eq!(dry["version_hash"], body2["head_version_hash"]);

    // Route khô KHÔNG ghi gì — khoá bằng một nonce CHƯA từng qua `create`: nếu route khô có
    // ghi thì ref đó đã tồn tại và `/head` trả 200.
    let nonce_never_written = [0x5au8; 32];
    let (st_dry2, dry2) = call(
        &app,
        "POST",
        "/v1/strata/_canonical",
        Some(json!({
            "seq": 0,
            "prev_hash": hex::encode([0u8; 32]),
            "content_cid": "cafe",
            "state_fields": [{ "key": "diagnosis", "value": V_A }],
            "author_did": hex::encode(DID),
            "policy_hash": hex::encode(policy.policy_hash()),
            "ts": 1_000,
            "genesis_nonce": hex::encode(nonce_never_written)
        })),
    )
    .await;
    assert_eq!(st_dry2, StatusCode::OK, "{dry2}");
    let (st3, _) = call(
        &app,
        "GET",
        &format!("/v1/strata/{}/head", dry2["ref_id"].as_str().unwrap()),
        None,
    )
    .await;
    assert_eq!(
        st3,
        StatusCode::NOT_FOUND,
        "route khô KHÔNG được tạo ref — 200 ở đây nghĩa là nó đã ghi"
    );
}

/// Mã lỗi ở đường audit KHÔNG được phân biệt "DID này có trong policy hồ sơ đó không".
///
/// Với `is_allowed` đặt TRƯỚC `verify_strict`, người lạ gửi chữ ký rác và đọc mã lỗi là dò
/// được: DID trong policy → `BadSignature`, DID ngoài policy → `PolicyDenied`. Không cần
/// khoá, không cần quyền, chỉ cần `ref_id` (công khai) và một danh sách DID ứng viên. Với
/// hồ sơ y tế thì chính quan hệ "bác sĩ D ghi được hồ sơ bệnh nhân P" là thứ phải giấu.
#[tokio::test]
async fn audit_error_code_does_not_leak_policy_membership() {
    let (app, policy) = app();
    let (r, _) = create_ok(&app, &policy).await;

    let probe = |did: [u8; 32]| {
        json!({
            "kind": "audit",
            "actor_did": hex::encode(did),
            "action": "Read",
            "signed_hash": hex::encode([0x22u8; 32]),
            "location": hex::encode([0x33u8; 32]),
            "ts": 1_000,
            "sig": hex::encode([0u8; 64])   // chữ ký rác — người dò KHÔNG có khoá nào
        })
    };

    // DID  = TRONG policy; DID2 = có khoá trong registry nhưng NGOÀI policy.
    let (st_in, body_in) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/event"),
        Some(probe(DID)),
    )
    .await;
    let (st_out, body_out) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/event"),
        Some(probe(DID2)),
    )
    .await;

    assert_eq!(st_in, st_out, "status phải giống nhau");
    assert_eq!(
        body_in["error"], body_out["error"],
        "mã lỗi phải GIỐNG NHAU cho DID trong và ngoài policy — khác nhau là một oracle dò \
         thành viên policy: trong={body_in}, ngoài={body_out}"
    );
    assert_eq!(
        body_in["error"], "BadSignature",
        "cửa đầu tiên phải là cửa đòi SỞ HỮU KHOÁ, không phải cửa quyền"
    );
}

/// `/head` phải trả đủ thứ client cần để append version kế — thiếu là bẫy onboarding.
#[tokio::test]
async fn head_carries_everything_needed_to_append_next() {
    let (app, policy) = app();
    let (r, _) = create_ok(&app, &policy).await;
    let (st, body) = call(&app, "GET", &format!("/v1/strata/{r}/head"), None).await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        body["ts"], 1_000,
        "thiếu ts ⇒ client phải lách bằng /version?at="
    );
    assert_eq!(
        body["policy_hash"],
        hex::encode(policy.policy_hash()),
        "thiếu policy_hash ⇒ client đoán rồi ăn 403"
    );
    assert_eq!(body["author_did"], hex::encode(DID));
}

// ── `_dirty` — nguồn đọc của hàng đợi neo (MB-6, chốt gom lô LIÊN HỘ) ────────

/// `create` với `author_did` + nonce + ts tuỳ chọn. Chính sách một-thành-viên =
/// người tạo, đúng mặc định của route (`policy_authors` vắng).
async fn create_as(
    app: &Router,
    seed: u8,
    did: [u8; 32],
    nonce: u8,
    ts: u64,
) -> (String, Hash32, Policy) {
    let mut policy = Policy::new();
    policy.allow(did, sk(seed).verifying_key());
    let fields = vec![f("diagnosis", V_A)];
    let sig = sign_version(
        seed,
        0,
        [0u8; 32],
        b"\xca\xfe",
        &fields,
        did,
        policy.policy_hash(),
        ts,
    );
    let (st, body) = call(
        app,
        "POST",
        "/v1/strata/create",
        Some(json!({
            "author_did": hex::encode(did),
            "genesis_nonce": hex::encode([nonce; 32]),
            "content_cid": "cafe",
            "state_fields": [{ "key": "diagnosis", "value": V_A }],
            "policy_hash": hex::encode(policy.policy_hash()),
            "ts": ts,
            "sig": sig
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "create: {body}");
    let r = body["ref_id"].as_str().unwrap().to_string();
    let vh = hex::decode(body["head_version_hash"].as_str().unwrap()).unwrap();
    (r, vh.try_into().unwrap(), policy)
}

async fn get_dirty(app: &Router, query: &str) -> Value {
    let (st, v) = call(app, "GET", &format!("/v1/strata/_dirty{query}"), None).await;
    assert_eq!(st, StatusCode::OK, "_dirty: {v}");
    v
}

/// Ca chính: hai lineage của **hai tác giả khác nhau** cùng nằm trong MỘT ảnh chụp,
/// cũ trước mới sau.
///
/// Đây là hình dạng mà chốt 2026-08-19 đòi: hàng đợi **liên hộ**. Nếu route này nhóm
/// hay lọc theo `author_did` thì bên tiêu thụ không bao giờ gom được lô liên hộ, và
/// nó sẽ trả phần cố định `0,190209 tADA` một lần cho **mỗi hộ** thay vì mỗi tx.
#[tokio::test]
async fn dirty_gom_lineage_cua_nhieu_tac_gia_trong_mot_anh_chup_cu_truoc() {
    let (app, _) = app();
    let (r_moi, _, _) = create_as(&app, 1, DID, 0x33, 2_000).await;
    let (r_cu, _, _) = create_as(&app, 2, DID2, 0x44, 1_000).await;

    let v = get_dirty(&app, "").await;
    assert_eq!(v["count"], 2);
    assert_eq!(v["truncated"], false);
    // Chưa neo lần nào ⇒ genesis cũng đang chờ ⇒ 1 version/lineage.
    assert_eq!(v["total_pending_versions"], 2);
    assert_eq!(v["oldest_unanchored_ts"], 1_000);

    let refs = v["refs"].as_array().unwrap();
    assert_eq!(refs[0]["oldest_unanchored_ts"], 1_000, "cũ phải đứng trước");
    assert_eq!(refs[1]["oldest_unanchored_ts"], 2_000);
    assert!(refs[0]["anchored_seq"].is_null(), "chưa neo ⇒ null");
    assert_eq!(refs[0]["pending_versions"], 1);

    // Hai tác giả khác nhau, cùng một ảnh chụp — điều kiện cần của lô liên hộ.
    let authors: Vec<&str> = refs
        .iter()
        .map(|r| r["author_did"].as_str().unwrap())
        .collect();
    assert!(authors.contains(&hex::encode(DID).as_str()));
    assert!(authors.contains(&hex::encode(DID2).as_str()));

    // `ref_id` trả hex32 để nạp thẳng vào `_anchor_batch`.
    let ids: Vec<&str> = refs.iter().map(|r| r["ref_id"].as_str().unwrap()).collect();
    assert!(ids.iter().all(|s| s.len() == 64 && hex::decode(s).is_ok()));
    // Và bech32 mà `create` trả về phải giải ra đúng những ref đó (không phải hai tập khác nhau).
    for bech in [&r_cu, &r_moi] {
        let raw = hex::encode(lampnet_strata::refid::decode_ref_id(bech).unwrap());
        assert!(ids.contains(&raw.as_str()), "thiếu {bech} trong _dirty");
    }
}

/// Neo tới head ⇒ **rời** `_dirty`; ghi thêm một version ⇒ **quay lại**, và mốc tuổi
/// là `ts` của version chưa neo cũ nhất, không phải `ts` của genesis.
///
/// Bài này canh đúng chỗ dễ sai nhất của một hàng đợi suy-ra-từ-trạng-thái: nếu mốc
/// tuổi lấy nhầm genesis thì mọi lineage đã neo một lần sẽ **luôn** trông như quá hạn
/// ⇒ cò tuổi bắn liên tục ⇒ mỗi lượt một tx, đúng hành vi đắt nhất.
#[tokio::test]
async fn dirty_bo_ref_da_neo_va_nhan_lai_khi_co_version_moi() {
    let (app, policy) = app();
    let (r, vh0) = create_ok(&app, &policy).await;

    let (st, _) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/anchor"),
        Some(json!({ "priority": "immediate" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let v = get_dirty(&app, "").await;
    assert_eq!(v["count"], 0, "đã neo tới head thì không còn chờ: {v}");
    assert!(v["oldest_unanchored_ts"].is_null());

    let (st, _) = call(
        &app,
        "POST",
        &format!("/v1/strata/{r}/version"),
        Some(append_body(1, DID, 0, vh0, 5_000, &policy, V_B)),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let v = get_dirty(&app, "").await;
    assert_eq!(v["count"], 1);
    let d = &v["refs"][0];
    assert_eq!(d["head_seq"], 1);
    assert_eq!(d["anchored_seq"], 0);
    assert_eq!(d["pending_versions"], 1);
    assert_eq!(
        d["oldest_unanchored_ts"], 5_000,
        "mốc tuổi phải là version chưa neo cũ nhất (seq 1), KHÔNG phải genesis (ts 1000)"
    );
}

/// `limit` cắt bớt nhưng **nói ra**. Cắt im lặng là cách một hàng đợi tưởng mình
/// đã nhìn hết việc trong khi vẫn còn.
#[tokio::test]
async fn dirty_limit_cat_bot_va_bao_truncated() {
    let (app, _) = app();
    create_as(&app, 1, DID, 0x33, 3_000).await;
    create_as(&app, 2, DID2, 0x44, 1_000).await;

    let v = get_dirty(&app, "?limit=1").await;
    assert_eq!(v["count"], 1);
    assert_eq!(v["truncated"], true, "cắt thì phải khai: {v}");
    assert_eq!(
        v["refs"][0]["oldest_unanchored_ts"], 1_000,
        "cắt phải giữ phần CHỜ LÂU NHẤT, không phải phần đầu ngẫu nhiên"
    );
    // Tổng cũng chỉ tính trên phần trả về — nếu không thì `count` và tổng nói hai chuyện.
    assert_eq!(v["total_pending_versions"], 1);

    let v = get_dirty(&app, "?limit=9").await;
    assert_eq!(
        v["truncated"], false,
        "limit lớn hơn số ref thì không phải cắt"
    );
}

#[tokio::test]
async fn dirty_limit_0_bi_tu_choi() {
    let (app, _) = app();
    let (st, _) = call(&app, "GET", "/v1/strata/_dirty?limit=0", None).await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "limit=0 im lặng trả rỗng là cách hàng đợi tự tắt mà không ai biết"
    );
}
