//! Nhật ký bền vững + replay — `strata-node` dựng lại được chính mình sau restart.
//!
//! Mỗi bài dưới đây chặn **một** cách hỏng, và cách hỏng nào cũng im lặng nếu không có
//! gác: một daemon replay hụt vẫn lên xanh, vẫn phục vụ proof, chỉ là proof của một lịch
//! sử khác lịch sử đã neo lên chuỗi.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use ed25519_dalek::{Signer, SigningKey};
use http_body_util::BodyExt;
use lampnet_strata::state::build_state_root;
use lampnet_strata::version::{Hash32, StrataVersion};
use lampnet_strata::{AuditEntry, Policy};
use lampnet_strata_node::{
    AppState, ChainStore, InMemoryRegistry, Journal, KeyRegistry, MemorySink, read_records,
    replay_into, router, store::lock,
};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const DID: [u8; 32] = [0x11; 32];
const NONCE: [u8; 32] = [0x33; 32];
const V_A: &str = "aa00000000000000000000000000000000000000000000000000000000000001";
const V_B: &str = "bb00000000000000000000000000000000000000000000000000000000000002";

fn sk(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn registry() -> Arc<dyn KeyRegistry> {
    let r = InMemoryRegistry::new();
    r.register(DID, sk(1).verifying_key());
    Arc::new(r)
}

fn policy() -> Policy {
    let mut p = Policy::new();
    p.allow(DID, sk(1).verifying_key());
    p
}

/// Đường dẫn tạm **duy nhất cho mỗi bài** — hai bài dùng chung một tệp thì bài chạy sau
/// replay lịch sử của bài chạy trước, và cả hai vẫn xanh.
fn tmp_path(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "strata-journal-{}-{}-{}.jsonl",
        std::process::id(),
        tag,
        n
    ))
}

fn app_with_journal(path: &PathBuf) -> Router {
    let j = Arc::new(Journal::open(path).expect("mở nhật ký"));
    let store = Arc::new(ChainStore::with_journal(j));
    router(AppState::new(
        store,
        registry(),
        Arc::new(MemorySink::new()),
    ))
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    use tower::ServiceExt;
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

fn f(key: &str, value: &str) -> (Vec<u8>, Vec<u8>) {
    (key.as_bytes().to_vec(), hex::decode(value).unwrap())
}

#[allow(clippy::too_many_arguments)]
fn sig_of(seq: u64, prev: Hash32, cid: &[u8], fields: &[(Vec<u8>, Vec<u8>)], ts: u64) -> String {
    let mut v = StrataVersion::unsigned(
        seq,
        prev,
        cid.to_vec(),
        build_state_root(fields),
        DID,
        policy().policy_hash(),
        ts,
    );
    v.sign(&sk(1));
    hex::encode(v.sig)
}

/// create → trả `(ref bech32, head_version_hash)`.
async fn create_ok(app: &Router) -> (String, Hash32) {
    let fields = vec![f("diagnosis", V_A)];
    let (st, b) = call(
        app,
        "POST",
        "/v1/strata/create",
        Some(json!({
            "author_did": hex::encode(DID),
            "genesis_nonce": hex::encode(NONCE),
            "content_cid": "cafe",
            "state_fields": [{ "key": "diagnosis", "value": V_A }],
            "policy_hash": hex::encode(policy().policy_hash()),
            "ts": 1_000,
            "sig": sig_of(0, [0u8; 32], b"\xca\xfe", &fields, 1_000)
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "create: {b}");
    let vh: Hash32 = hex::decode(b["head_version_hash"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    (b["ref_id"].as_str().unwrap().to_string(), vh)
}

async fn append_ok(app: &Router, r: &str, prev_seq: u64, prev: Hash32, ts: u64) -> Hash32 {
    let fields = vec![f("diagnosis", V_B)];
    let (st, b) = call(
        app,
        "POST",
        &format!("/v1/strata/{r}/version"),
        Some(json!({
            "prev_seq": prev_seq,
            "content_cid": "beef",
            "state_fields": [{ "key": "diagnosis", "value": V_B }],
            "author_did": hex::encode(DID),
            "policy_hash": hex::encode(policy().policy_hash()),
            "ts": ts,
            "sig": sig_of(prev_seq + 1, prev, b"\xbe\xef", &fields, ts)
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "append: {b}");
    hex::decode(b["version_hash"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap()
}

async fn audit_ok(app: &Router, r: &str, ts: u64) {
    let ae = AuditEntry {
        created_ts: ts,
        actor_did: DID,
        action: lampnet_strata::AuditAction::Read,
        signed_hash: [7u8; 32],
        location: [8u8; 32],
    };
    let (st, b) = call(
        app,
        "POST",
        &format!("/v1/strata/{r}/event"),
        Some(json!({
            "kind": "audit",
            "actor_did": hex::encode(DID),
            "action": "Read",
            "signed_hash": hex::encode([7u8; 32]),
            "location": hex::encode([8u8; 32]),
            "ts": ts,
            "sig": hex::encode(sk(1).sign(&ae.canonical()).to_bytes())
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "audit: {b}");
}

async fn anchor_ok(app: &Router, r: &str) -> Value {
    let (st, b) = call(
        app,
        "POST",
        &format!("/v1/strata/{r}/anchor"),
        Some(json!({ "priority": "batch_daily" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "anchor: {b}");
    b
}

/// Replay tệp vào một kho MỚI — đúng việc bin làm lúc khởi động.
fn replay_fresh(path: &PathBuf, reg: Arc<dyn KeyRegistry>) -> Result<ChainStore, String> {
    let recs = read_records(path).map_err(|e| e.to_string())?;
    let store = ChainStore::new();
    replay_into(&store, reg.as_ref(), &recs).map_err(|e| e.to_string())?;
    Ok(store)
}

/// `expect_err` cần `T: Debug`; `ChainStore` không có, và không nên có (nó ôm cả kho).
fn replay_must_fail(path: &PathBuf, reg: Arc<dyn KeyRegistry>, why: &str) -> String {
    match replay_fresh(path, reg) {
        Ok(_) => panic!("{why}"),
        Err(e) => e,
    }
}

fn ref_raw(bech: &str) -> Hash32 {
    lampnet_strata::refid::decode_ref_id(bech).expect("ref hợp lệ")
}

// ────────────────────────────────────────────────────────────────────────────

/// Đường sống: mọi thứ daemon giữ phải sống qua một lượt restart.
///
/// Kiểm **cả bốn** loại trạng thái daemon giữ, vì chúng nằm ở bốn chỗ khác nhau và một
/// bản replay quên đúng một chỗ vẫn xanh với ba bài kiểm còn lại:
/// `chain` (head/MMR) · `fields` (nguồn của `prove_field`) · `audit` · gương `anchored`.
#[tokio::test]
async fn khoi_phuc_day_du_qua_mot_luot_restart() {
    let path = tmp_path("roundtrip");
    let app = app_with_journal(&path);

    let (r, vh0) = create_ok(&app).await;
    let vh1 = append_ok(&app, &r, 0, vh0, 1_100).await;
    append_ok(&app, &r, 1, vh1, 1_200).await;
    audit_ok(&app, &r, 1_300).await;
    let anchor = anchor_ok(&app, &r).await;

    let (_, head_before) = call(&app, "GET", &format!("/v1/strata/{r}/head"), None).await;
    let (_, proof_before) = call(
        &app,
        "GET",
        &format!("/v1/strata/{r}/proof/field/diagnosis"),
        None,
    )
    .await;

    // ── Đối chứng: kho MỚI mà KHÔNG replay thì không có gì cả ────────────────
    // Thiếu vế này thì một bài "replay xong đọc thấy dữ liệu" cũng xanh với một replay
    // không làm gì, miễn là ta lỡ đọc lại chính kho cũ.
    let empty = ChainStore::new();
    assert!(
        empty.get(&ref_raw(&r)).is_none(),
        "đối chứng: kho chưa replay phải RỖNG"
    );

    // ── Restart ──────────────────────────────────────────────────────────────
    let store = replay_fresh(&path, registry()).expect("replay phải đạt");
    let app2 = router(AppState::new(
        Arc::new(store),
        registry(),
        Arc::new(MemorySink::new()),
    ));

    let (st, head_after) = call(&app2, "GET", &format!("/v1/strata/{r}/head"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        head_after, head_before,
        "head + mmr_root phải khớp từng byte"
    );

    let (_, proof_after) = call(
        &app2,
        "GET",
        &format!("/v1/strata/{r}/proof/field/diagnosis"),
        None,
    )
    .await;
    assert_eq!(
        proof_after, proof_before,
        "`fields` phải sống — state_root là hàm một chiều, mất fields là mất prove_field \
         vĩnh viễn dù chain còn nguyên"
    );

    // Gương `anchored`: neo lại ĐÚNG seq đã neo phải bị chặn (INV-E7), chứng tỏ gương sống.
    let (st, b) = call(
        &app2,
        "POST",
        &format!("/v1/strata/{r}/anchor"),
        Some(json!({ "priority": "batch_daily" })),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "gương `anchored` phải sống qua restart, nếu không ref neo lại được từ đầu: {b}"
    );
    assert_eq!(b["error"], "AnchorRollback");
    assert_eq!(anchor["seq"], 2, "neo ở head seq=2");
}

/// Audit-log sống riêng: `log_root` phải dựng lại đúng.
///
/// Tách khỏi bài trên vì `audit` là **cây thứ ba**, không nằm trong `chain` lẫn `fields`
/// — một replay bỏ qua nhánh `Audit` vẫn qua sạch bài trên.
#[tokio::test]
async fn audit_log_song_qua_restart() {
    let path = tmp_path("audit");
    let app = app_with_journal(&path);
    let (r, _) = create_ok(&app).await;
    audit_ok(&app, &r, 1_100).await;
    audit_ok(&app, &r, 1_200).await;

    let store = replay_fresh(&path, registry()).expect("replay phải đạt");
    let e = store.get(&ref_raw(&r)).expect("ref phải có");
    let g = lock(&e);
    assert_eq!(g.audit.len(), 2, "hai entry audit phải sống");
}

/// 🪤 Đuôi rách — tiến trình chết giữa một lượt `write_all`.
///
/// Bỏ **đúng** dòng cuối, và bỏ nó là khôi phục đúng sự thật: cửa chỉ trả 200 sau khi
/// `append` nhật ký trả `Ok`, nên một dòng chưa có `\n` là một thao tác client **chưa
/// từng được báo thành công**.
#[tokio::test]
async fn duoi_rach_bo_dung_dong_cuoi_va_phan_con_lai_van_song() {
    let path = tmp_path("torn");
    let app = app_with_journal(&path);
    let (r, vh0) = create_ok(&app).await;
    append_ok(&app, &r, 0, vh0, 1_100).await;

    // Cắt cụt dòng cuối (bản ghi Append), giữ nguyên phần trước.
    let raw = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<&str> = raw.lines().collect();
    let last = lines.pop().unwrap();
    let torn = format!("{}\n{}", lines.join("\n"), &last[..last.len() / 2]);
    std::fs::write(&path, torn).unwrap();

    let store = replay_fresh(&path, registry()).expect("đuôi rách KHÔNG được làm hỏng replay");
    let e = store.get(&ref_raw(&r)).expect("create vẫn phải sống");
    let g = lock(&e);
    assert_eq!(
        g.chain.head().seq,
        0,
        "version của dòng rách chưa từng thành công ⇒ không được có mặt"
    );
}

/// 🔴 Sửa một byte trong nhật ký ⇒ daemon **KHÔNG lên**.
///
/// Đây là tính chất mà cách "tuần tự hoá trạng thái rồi nạp lại" **không** mua được: ở
/// đó một tệp bị sửa nạp vào thành một `StrataChain` chưa qua cửa nào, rồi phục vụ proof.
#[tokio::test]
async fn sua_mot_byte_thi_replay_tu_choi_chu_khong_phuc_vu_lich_su_gia() {
    let path = tmp_path("tamper");
    let app = app_with_journal(&path);
    let (r, vh0) = create_ok(&app).await;
    append_ok(&app, &r, 0, vh0, 1_100).await;

    // Đổi `content_cid` của version thứ hai: "beef" → "bee0". Chữ ký cũ không còn phủ nó.
    let raw = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, raw.replace("\"beef\"", "\"bee0\"")).unwrap();

    let err = replay_must_fail(&path, registry(), "nhật ký bị sửa PHẢI chặn khởi động");
    assert!(
        err.contains("BadSignature"),
        "phải đỏ vì chữ ký, tức nó thật sự đi qua lõi: {err}"
    );
}

/// 🔴 Gỡ khoá khỏi registry ⇒ replay từ chối, và nói ra `UnknownAuthor`.
///
/// Đây là cái giá **đã biết** của việc KHÔNG ghi pubkey vào nhật ký (nguồn sự thật thứ
/// hai). Ghi ra thành một bài kiểm để nó là hành vi có chủ ý, không phải một bất ngờ ở
/// lần xoay khoá đầu tiên.
#[tokio::test]
async fn go_khoa_khoi_registry_thi_replay_tu_choi_thay_vi_doan() {
    let path = tmp_path("nokey");
    let app = app_with_journal(&path);
    create_ok(&app).await;

    let empty: Arc<dyn KeyRegistry> = Arc::new(InMemoryRegistry::new());
    let err = replay_must_fail(&path, empty, "registry thiếu khoá PHẢI chặn khởi động");
    assert!(err.contains("UnknownAuthor"), "{err}");
}

/// Header lệch `format` ⇒ từ chối, không đoán.
#[tokio::test]
async fn header_lech_dinh_dang_thi_tu_choi() {
    let path = tmp_path("hdr");
    let app = app_with_journal(&path);
    create_ok(&app).await;

    let raw = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, raw.replace("\"format\":1", "\"format\":999")).unwrap();
    let err = replay_must_fail(&path, registry(), "format lạ PHẢI chặn");
    assert!(
        err.contains("999"),
        "thông điệp phải nêu số đọc được: {err}"
    );
}

/// 🔴 `Anchor.seq` là ĐỐI CHỨNG, không phải giá trị nạp lại.
///
/// Lệch nghĩa là chuỗi dựng lại KHÁC chuỗi lúc ghi — và khi ấy mọi proof daemon sắp phục
/// vụ nói về một lịch sử khác lịch sử đã neo lên chuỗi. Không có gì bật ra nếu ta chỉ nạp
/// con số đó vào gương.
#[tokio::test]
async fn anchor_seq_lech_thi_tu_choi_chu_khong_nap_vao_guong() {
    let path = tmp_path("anchorseq");
    let app = app_with_journal(&path);
    let (r, vh0) = create_ok(&app).await;
    append_ok(&app, &r, 0, vh0, 1_100).await;
    anchor_ok(&app, &r).await;

    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("\"seq\":1"), "bản ghi neo phải khai seq=1");
    std::fs::write(&path, raw.replace("\"seq\":1", "\"seq\":9")).unwrap();

    let err = replay_must_fail(&path, registry(), "seq lệch PHẢI chặn");
    assert!(err.contains("seq 9"), "{err}");
}

/// 🔴 Ghi hỏng THẬT ⇒ nhật ký tự đầu độc, và mọi lượt ghi sau trả **503**.
///
/// Dùng `/dev/full` — mọi lượt ghi trả `ENOSPC` — nên đây là một lỗi I/O thật, không phải
/// một cờ do bài kiểm tự bật.
#[tokio::test]
#[cfg(target_os = "linux")]
async fn ghi_hong_that_thi_dau_doc_va_cua_tra_503() {
    let Ok(j) = Journal::open("/dev/full") else {
        return; // môi trường không có /dev/full — không có gì để đo
    };
    let j = Arc::new(j);
    assert!(!j.is_poisoned(), "chưa ghi thì chưa đầu độc");

    let store = Arc::new(ChainStore::with_journal(j.clone()));
    let app = router(AppState::new(
        store,
        registry(),
        Arc::new(MemorySink::new()),
    ));

    let fields = vec![f("diagnosis", V_A)];
    let (st, b) = call(
        &app,
        "POST",
        "/v1/strata/create",
        Some(json!({
            "author_did": hex::encode(DID),
            "genesis_nonce": hex::encode(NONCE),
            "content_cid": "cafe",
            "state_fields": [{ "key": "diagnosis", "value": V_A }],
            "policy_hash": hex::encode(policy().policy_hash()),
            "ts": 1_000,
            "sig": sig_of(0, [0u8; 32], b"\xca\xfe", &fields, 1_000)
        })),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::SERVICE_UNAVAILABLE,
        "ghi đĩa hỏng KHÔNG được trả 200 — 200 là hứa một thứ sẽ biến mất: {b}"
    );
    assert_eq!(b["error"], "JournalBroken");
    assert!(j.is_poisoned(), "một lượt ghi hỏng phải đầu độc nhật ký");
}

/// Đối chứng cho bài trên: kho **không** nhật ký thì ghi vẫn qua, và không sinh tệp nào.
///
/// Thiếu vế này thì một bản vá làm mọi lượt ghi trả 503 cũng qua được bài `/dev/full`.
#[tokio::test]
async fn khong_co_nhat_ky_thi_duong_ghi_van_chay_binh_thuong() {
    let app = router(AppState::new(
        Arc::new(ChainStore::new()),
        registry(),
        Arc::new(MemorySink::new()),
    ));
    let (r, vh0) = create_ok(&app).await;
    append_ok(&app, &r, 0, vh0, 1_100).await;
}
