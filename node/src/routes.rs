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
use crate::journal::{JournalError, JournalRecord, ref_hex};
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
use lampnet_strata::{AnchorError, AnchorPriority, AnchorSink, AuditAction, AuditEntry};
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
        // Route KHÔ: dựng lại canonical/hash mà KHÔNG ghi. Không đụng store, không cần ref.
        .route("/v1/strata/_canonical", post(canonical))
        // Cùng path, hai method: POST = thêm version; GET ?at= = giá trị tại thời điểm t.
        .route("/v1/strata/:ref/version", post(append).get(version_at))
        .route("/v1/strata/:ref/event", post(event))
        .route("/v1/strata/:ref/head", get(head))
        .route("/v1/strata/:ref/proof/version/:seq", get(proof_version))
        .route("/v1/strata/:ref/proof/field/:key", get(proof_field))
        .route("/v1/strata/:ref/anchor", post(anchor))
        // Route LÔ (B1′). Tiền tố `_` giống `_canonical`: nó KHÔNG phải một `:ref`,
        // và đặt tên vậy thì không ref_id hợp lệ nào đụng vào được.
        .route("/v1/strata/_anchor_batch", post(anchor_batch))
        // Nguồn đọc của hàng đợi neo phía Mosaic (§13.1 lớp (1)).
        .route("/v1/strata/_dirty", get(dirty))
        // Nguồn LÁ của luồng checkpoint toàn cục (`Specs#32` mục 10).
        .route("/v1/strata/_settlement_window", get(settlement_window))
        .with_state(state)
}

// ────────────────────────────────────────────────────────────────────────────
// Tiện ích chung
// ────────────────────────────────────────────────────────────────────────────

/// Ghi nhật ký hỏng ⇒ **503**, và daemon đã tự đầu độc (mọi lượt ghi sau cũng 503).
///
/// Không nuốt được: lượt ghi vừa rồi ĐÃ vào RAM nhưng KHÔNG vào đĩa, nên trả 200 ở đây
/// là hứa một thứ sẽ biến mất ở lần khởi động sau — đúng loại hỏng im lặng mà cả nhật ký
/// lẫn cửa này sinh ra để chặn.
fn journal_err(e: JournalError) -> ApiError {
    ApiError::JournalBroken(e.to_string())
}

/// Ghi một bản ghi vào nhật ký của kho (không có nhật ký ⇒ không làm gì).
fn journal(st: &AppState, rec: JournalRecord) -> ApiResult<()> {
    match st.store.journal() {
        Some(j) => j.append(&rec).map_err(journal_err),
        None => Ok(()),
    }
}

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

/// Query của [`dirty`]. `limit` vắng ⇒ trả tất cả.
#[derive(Debug, Deserialize)]
pub struct DirtyQuery {
    pub limit: Option<usize>,
}

/// Query của `GET /v1/strata/_settlement_window`.
///
/// Cả hai tham số **bắt buộc**, không có mặc định. Một cửa sổ mặc định là một cửa sổ
/// mà bên gọi không khai — và cam kết on-chain thì phải nói rõ nó cam kết đúng quãng
/// nào, chứ không phải quãng mà server tự chọn hôm đó.
#[derive(Debug, Deserialize)]
pub struct WindowQuery {
    pub from_slot: u64,
    pub to_slot: u64,
}

/// Biên lệch đồng hồ cho phép giữa `ts` client khai và đồng hồ daemon (giây).
///
/// 300 s đủ rộng cho lệch NTP/múi giờ thực tế, và hẹp hơn **7 bậc độ lớn** so với khoảng
/// cách giữa giây và mili giây — nên nó phân loại đúng ca duy nhất cần bắt: `Date.now()`.
const MAX_TS_SKEW_SECS: u64 = 300;

/// Trần TUYỆT ĐỐI cho `ts` tính bằng giây — **không phụ thuộc đồng hồ daemon**.
///
/// `10^12` giây unix rơi vào năm **33658**; mọi `ts` giây thật đều nhỏ hơn con số này trong
/// suốt vòng đời hệ. Ngược lại `Date.now()` hôm nay đã ở `1.78 × 10^12`, tức **mọi** giá trị
/// mili giây đương thời đều vượt trần. Vậy một phép so sánh hằng số phân loại đúng ca nguy
/// hiểm nhất mà KHÔNG cần biết bây giờ là mấy giờ.
const TS_MILLIS_FLOOR: u64 = 1_000_000_000_000;

/// Đồng hồ tường của daemon (unix secs). `None` khi không đọc được (bất khả trên thực tế,
/// nhưng `duration_since` trả `Result`).
///
/// Trả `Option` chứ KHÔNG `unwrap_or(0)`: `0` là một giá trị **hợp lệ trong miền**, nên trộn
/// nó với "không biết" tạo ra hai lỗi ngược chiều nhau. Đồng hồ boot ở epoch (container
/// không RTC) cho `now == 0` ⇒ guard tự tắt IM LẶNG đúng lúc cần nhất; còn đồng hồ ở
/// `1970 + 5s` (chưa kịp NTP) cho `now == 5` ⇒ **mọi** ghi hợp lệ bị 422 kèm thông điệp lạc
/// hướng `now: 5`. Nay `now` chỉ dùng cho lớp thứ hai, và lớp thứ nhất ([`TS_MILLIS_FLOOR`])
/// không cần đồng hồ nên đồng hồ hỏng không tắt được nó.
fn now_secs() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs())
}

/// Chặn `ts` tương lai xa Ở CỬA (xem [`ApiError::TimestampTooFarFuture`]), HAI LỚP.
///
/// 1. **Trần tuyệt đối** `TS_MILLIS_FLOOR` — không cần đồng hồ, không tắt được.
/// 2. **Biên lệch** `MAX_TS_SKEW_SECS` quanh đồng hồ daemon — bắt các ca tương lai gần hơn;
///    bỏ qua khi không đọc được đồng hồ.
///
/// Cân đánh đổi theo chiều đúng: chặn nhầm một ghi là **khả hồi** (client sửa rồi gửi lại),
/// nhận nhầm một `ts` mili giây là **vĩnh viễn** (ref mất quyền ghi, không có route sửa).
///
/// KHÔNG chặn `ts` quá KHỨ: lõi đã ép không-giảm (`TimestampRegress`), và nhập dữ liệu lịch
/// sử là ca dùng hợp lệ. Hệ quả phải nói rõ với bên tiêu thụ: `ts` là **lời khai của tác
/// giả**, chỉ lần neo on-chain mới cho một cận trên thời gian — mà neo là tuỳ chọn.
fn check_ts(ts: u64) -> ApiResult<()> {
    check_ts_at(ts, now_secs())
}

/// Phần THUẦN của [`check_ts`] — nhận đồng hồ làm tham số thay vì tự đi lấy.
///
/// Tách ra vì lớp 1 và lớp 2 **trùng miền trên mọi đầu vào mà một test đi qua HTTP gửi tới
/// được**: mọi `ts` vượt `TS_MILLIS_FLOOR` (`10^12`) thì cũng vượt `now + 300` với `now` là
/// đồng hồ thật (`≈ 1,79 × 10^9`). Nên test qua router **không phân biệt được hai lớp** — gỡ
/// hẳn lớp 1 mà toàn bộ bộ kiểm vẫn xanh, kể cả bài mang tên `..._not_by_clock`.
///
/// Ca duy nhất lớp 1 gánh một mình là `now == None`: đồng hồ đặt **trước** epoch nên
/// `duration_since(UNIX_EPOCH)` lỗi ⇒ lớp 2 bị `if let Some(now)` bỏ qua. Chỉ tới được ca đó
/// khi đồng hồ là **tham số**; `SystemTime::now()` gọi thẳng trong hàm thì không có đường
/// dựng lại nó trong test.
///
/// *Một lớp phòng vệ không có bài kiểm phân biệt được nó với lớp bên cạnh là một lớp sắp bị
/// gộp mất trong lần dọn mã tới — và CI sẽ đồng ý với người gộp.*
fn check_ts_at(ts: u64, now: Option<u64>) -> ApiResult<()> {
    if ts >= TS_MILLIS_FLOOR {
        return Err(ApiError::TimestampTooFarFuture {
            got: ts,
            now: now.unwrap_or(0),
            max_skew: MAX_TS_SKEW_SECS,
        });
    }
    if let Some(now) = now
        && ts > now.saturating_add(MAX_TS_SKEW_SECS)
    {
        return Err(ApiError::TimestampTooFarFuture {
            got: ts,
            now,
            max_skew: MAX_TS_SKEW_SECS,
        });
    }
    Ok(())
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

/// Phần THUẦN của `create` — dựng `ChainEntry` + `CreateResp` từ request đã nhận.
///
/// Tách ra vì **replay nhật ký phải đi đúng đường này**, không phải một đường thứ hai:
/// hai đường dựng genesis là hai định nghĩa cho cùng một vị ngữ, và chúng lệch nhau vào
/// ngày không ai nhìn. Ở đây "đúng đường" gồm cả phần dễ quên — `policy` dựng từ khoá
/// **do registry trả**, không phải khoá suy từ `Did` (CHỐT-5).
pub(crate) fn create_inner(
    registry: &dyn KeyRegistry,
    req: &CreateReq,
) -> ApiResult<(Hash32, ChainEntry, CreateResp)> {
    check_ts(req.ts)?;
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
        let pk = registry
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
    Ok((ref_id, ChainEntry::new(chain, policy, fields), resp))
}

async fn create(
    State(st): State<AppState>,
    req: Result<Json<CreateReq>, JsonRejection>,
) -> ApiResult<Json<CreateResp>> {
    let req = body(req)?;
    let (ref_id, entry, resp) = create_inner(st.registry.as_ref(), &req)?;

    // Nhật ký đi CÙNG phép chèn, dưới cùng một khoá — xem `ChainStore::insert_journaled`
    // cho lý do I/O nằm dưới khoá ghi của kho.
    let rec = JournalRecord::Create {
        r: ref_hex(&ref_id),
        req: req.clone(),
    };
    st.store
        .insert_journaled(ref_id, entry, Some(rec))
        .map_err(|e| match e {
            crate::store::StoreError::RefExists => ApiError::RefExists(ref_id),
            crate::store::StoreError::Journal(j) => journal_err(j),
        })?;
    Ok(Json(resp))
}

// ────────────────────────────────────────────────────────────────────────────
// POST /v1/strata/_canonical — route KHÔ, KHÔNG ghi, KHÔNG cần ref tồn tại
// ────────────────────────────────────────────────────────────────────────────

/// Dựng lại `state_root` + `canonical_core` + `version_hash` từ đúng các trường client sắp
/// ký, rồi trả về — **không** chạm `store`, **không** cần chữ ký, **không** đẻ trạng thái.
///
/// Đây là đường đối chiếu byte mà trước nay không có. Client phải tự cài lại cây state
/// (sort khoá tăng dần, lá lẻ **carry** chứ không nhân đôi) và encoding canonical (u64 BE,
/// len-prefix `u32_be` cho `content_cid`) ở ngôn ngữ của mình; lệch một bit thì đường ghi
/// trả `403 BadSignature` — một thông điệp không hề nhắc tới `state_root`, nên người tích
/// hợp không có manh mối nào. Route này biến hai ngày đoán mò thành một lượt HTTP.
///
/// An toàn: mọi giá trị trả về đều là thứ đường ghi thành công đã trả (`version_hash`,
/// `state_root`, `ref_id`) hoặc suy được từ input của chính client (`canonical_core`) —
/// không lộ thêm gì. Không có `ref` trong đường dẫn nên không rò sự tồn tại của hồ sơ nào.
async fn canonical(
    req: Result<Json<CanonicalReq>, JsonRejection>,
) -> ApiResult<Json<CanonicalResp>> {
    let req = body(req)?;
    // Route khô phải chạy ĐÚNG bộ cổng của đường ghi, chỉ bỏ phần ghi. Thiếu `check_ts` ở
    // đây thì nó "duyệt" một `ts` mili giây và trả về `version_hash`; client ký xong, gọi
    // `create`, ăn 422 — tức công cụ dựng ra để đối chiếu trước khi ký lại bỏ lọt đúng lỗi
    // mà nó tồn tại để bắt.
    check_ts(req.ts)?;
    let fields = to_pairs(&req.state_fields).map_err(ApiError::Malformed)?;
    let state_root = build_state_root(&fields);

    let v = StrataVersion::unsigned(
        req.seq,
        req.prev_hash,
        req.content_cid.clone(),
        state_root,
        req.author_did,
        req.policy_hash,
        req.ts,
    );

    // `genesis_nonce` có ⇒ trả luôn ref_id dự kiến, để client đối chiếu TRƯỚC khi `create`
    // (sai nonce = sai ref_id = bất-khả-hồi, không có route đổi).
    let ref_id = match &req.genesis_nonce {
        Some(h) => {
            let nonce = hexs::decode_fixed::<32>(h)
                .map_err(|e| ApiError::Malformed(format!("genesis_nonce: {e}")))?;
            Some(encode_ref_id(&gen_ref_id_raw(&req.author_did, &nonce)))
        }
        None => None,
    };

    Ok(Json(CanonicalResp {
        canonical_core: hex::encode(v.canonical_core()),
        version_hash: hex::encode(v.version_hash()),
        state_root: hex::encode(state_root),
        ref_id,
    }))
}

// ────────────────────────────────────────────────────────────────────────────
// POST /v1/strata/:ref/version — §2.2 append_version
// ────────────────────────────────────────────────────────────────────────────

/// Phần thân dùng chung cho `POST /version` và `POST /event` với `kind="version"` (§2.6 cách 1).
pub(crate) fn append_inner(e: &mut ChainEntry, req: &AppendReq) -> ApiResult<AppendResp> {
    check_ts(req.ts)?;
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
    let resp = append_inner(&mut g, &req)?;
    // Ghi nhật ký SAU khi lõi nhận, và VẪN dưới khoá của ref: thứ tự các bản ghi của một
    // ref trong tệp phải đúng thứ tự chúng được áp, nếu không replay dựng ra một chuỗi
    // khác. Thả khoá trước khi ghi là mở đúng khe cho hai version đảo chỗ.
    journal(
        &st,
        JournalRecord::Append {
            r: ref_hex(&ref_id),
            req: req.clone(),
        },
    )?;
    Ok(Json(resp))
}

// ────────────────────────────────────────────────────────────────────────────
// POST /v1/strata/:ref/event — §2.6
// ────────────────────────────────────────────────────────────────────────────

/// Phần THUẦN của nhánh `kind="audit"` — dùng chung với replay nhật ký.
///
/// ── Hai cổng dưới đây vá một lỗ ĐÃ DỰNG LẠI ĐƯỢC bằng 2 request HTTP ──────
///
/// Đường audit và đường version cùng ghi vào MỘT `ChainEntry`, nhưng trước đây chỉ đường
/// version đi qua `check_auth` → `policy.is_allowed`. Đường audit chỉ phân giải khoá từ
/// key-registry TOÀN CỤC rồi `verify_strict` — nghĩa là bất kỳ ai có DID trong registry
/// (đủ điều kiện để tạo hồ sơ của CHÍNH MÌNH) đều ghi được vào nhật ký truy cập của hồ sơ
/// NGƯỜI KHÁC. Chữ ký hợp lệ, nhưng hợp lệ cho sai hồ sơ: xác thực ≠ phân quyền.
///
/// Nối với `ts` thì thành bất-khả-hồi: người ngoài ghi một mục `ts = u64::MAX`,
/// `AuditLog.last_ts` nhảy lên trần, và từ đó MỌI mục thật của chính chủ trả
/// `TimestampRegress` vĩnh viễn (`audit.rs` append-only, không có API xoá/reset).
///
/// THỨ TỰ CÓ CHỦ Ý — chữ ký TRƯỚC, policy SAU, `ts` cuối. Đặt `is_allowed` lên trước
/// `verify_strict` (bản nháp đầu) mở ra một **oracle dò thành viên policy**: người lạ gửi
/// chữ ký rác `00`×64 với một DID ứng viên và đọc mã lỗi — DID trong policy trả
/// `BadSignature`, DID ngoài policy trả `PolicyDenied`. Ba request là biết "bác sĩ D có
/// quyền ghi hồ sơ bệnh nhân P không", không cần khoá nào. Chính quan hệ đó mới là thứ
/// phải giấu.
///
/// Đặt `verify_strict` trước thì cửa đầu tiên đòi **sở hữu khoá riêng**: người không có
/// khoá luôn nhận `BadSignature`, không phân biệt được gì. Người CÓ khoá đi tiếp và có
/// thể học rằng DID của CHÍNH MÌNH không nằm trong policy — đó là quyền của chính họ,
/// không phải rò rỉ. `check_ts` xuống cuối cùng vì `now` của daemon chỉ nên lộ cho bên đã
/// qua cả xác thực lẫn phân quyền.
pub(crate) fn audit_inner(
    registry: &dyn KeyRegistry,
    g: &mut ChainEntry,
    a: &AuditEventReq,
) -> ApiResult<AuditEventResp> {
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
    let pk = registry
        .resolve(&a.actor_did)
        .ok_or(ApiError::Core(StrataError::UnknownAuthor))?;
    // `verify_strict` (không phải `verify`) — cùng độ chặt với core: loại chữ ký
    // malleable / khoá small-order (INV-E4).
    let sig = Signature::from_bytes(&a.sig);
    pk.verify_strict(&ae.canonical(), &sig)
        .map_err(|_| ApiError::Core(StrataError::BadSignature))?;
    if !g.policy.is_allowed(&a.actor_did) {
        return Err(StrataError::PolicyDenied.into());
    }
    check_ts(a.ts)?;

    let index = g.audit.append_access(ae)?;
    let log_root = hex::encode(g.audit.root());
    Ok(AuditEventResp { index, log_root })
}

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

    match &req {
        // Cách 1: event = một version state-rỗng.
        EventReq::Version(a) => {
            let resp = append_inner(&mut g, a)?;
            journal(
                &st,
                JournalRecord::Append {
                    r: ref_hex(&ref_id),
                    req: a.clone(),
                },
            )?;
            Ok(Json(resp).into_response())
        }
        // Cách 2: entry vào audit-log (không đẻ version).
        EventReq::Audit(a) => {
            let resp = audit_inner(st.registry.as_ref(), &mut g, a)?;
            journal(
                &st,
                JournalRecord::Audit {
                    r: ref_hex(&ref_id),
                    req: a.clone(),
                },
            )?;
            Ok(Json(resp).into_response())
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
        // Ba trường này là ĐIỀU KIỆN để append version kế. Thiếu chúng, client chỉ cầm
        // `ref_id` phải lách bằng `GET /version?at=<số lớn>` hoặc đoán `policy_hash` rồi ăn
        // 403 — cả hai đều là bẫy onboarding, không phải quyết định thiết kế.
        ts: h.ts,
        policy_hash: hex::encode(h.policy_hash),
        author_did: hex::encode(h.author_did),
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
    q: Result<Query<SeqQuery>, axum::extract::rejection::QueryRejection>,
) -> ApiResult<Json<FieldProofResp>> {
    // Bọc `Result` như `version_at`: `?seq=abc` phải ra ĐÚNG format lỗi của ta, không rơi
    // về format mặc định của axum (client chỉ phải hiểu MỘT format).
    let Query(q) =
        q.map_err(|e| ApiError::Malformed(format!("`seq` không hợp lệ: {}", e.body_text())))?;
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
    // Nhật ký ghi SAU khi on-chain đã nhận — cùng thứ tự với gương `anchored`, và vì cùng
    // lý do: ghi trước rồi backend hỏng thì replay dựng lại một ref "đã neo" mà chuỗi
    // không biết, tức ref bị **cháy** (mọi lượt thử sau trả `AnchorRollback` vĩnh viễn).
    journal(
        st,
        JournalRecord::Anchor {
            r: ref_hex(&ref_id),
            seq: committed.seq,
            txid: txid.clone(),
            backend: backend.clone(),
        },
    )?;
    Ok(AnchorResp::new(&committed, txid, backend))
}

// ────────────────────────────────────────────────────────────────────────────
// Neo LÔ — mối nối B1′ (Mosaic quyết lô)
// ────────────────────────────────────────────────────────────────────────────

/// `GET /v1/strata/_dirty` — những lineage **đang chờ neo**, cũ trước mới sau.
///
/// Đây là nguồn đọc của hàng đợi neo phía Mosaic (`Mosaic-Math §13.1` lớp (1)).
/// Kho này **không** quyết lô và **không** giữ hàng đợi — nó chỉ trả lời một câu
/// hỏi thuần về trạng thái: *cái gì đã ghi mà chưa lên chuỗi.*
///
/// # Không có bộ đếm nào ở đây, và đó là chủ ý
///
/// `head_seq` đọc từ `chain`, `anchored_seq` đọc từ gương `anchored` mà daemon vốn
/// đã giữ để tự kiểm rollback. Ba đại lượng bên tiêu thụ cần đều **tính ra** từ đó:
///
/// ```text
/// dirty_refs           = { ref : chưa neo, hoặc head_seq > anchored_seq }
/// total_pending        = Σ pending_versions        ← số liệu bậc SLA
/// oldest_unanchored_ts = min(oldest_unanchored_ts) ← cò tuổi (N-1: ≤ 24 h)
/// ```
///
/// Một bộ đếm tăng dần thì lệch được, đếm trùng được và mất khi restart; một tổng
/// suy từ nguồn sự thật thì không có chỗ để lệch.
///
/// # `?limit=` cắt bớt, nhưng **không im lặng**
///
/// Cắt xong thì `truncated = true`. Một hàng đợi bị cắt âm thầm sẽ tưởng mình đã
/// nhìn hết việc — đúng loại hỏng không có thông báo lỗi nào.
///
/// ⚠️ **Trả `author_did` nhưng nó KHÔNG còn là ranh giới lô** (chốt 2026-08-19):
/// lô gom **liên hộ**, chia theo **kích cỡ**. Ai nhóm lô theo trường này là đang
/// dựng lại một ràng buộc đã bỏ, và trả giá bằng phần cố định `0,190209 tADA` nhân
/// với *số hộ* thay vì *số tx*.
async fn dirty(
    State(st): State<AppState>,
    Query(q): Query<DirtyQuery>,
) -> ApiResult<Json<DirtyResp>> {
    if let Some(0) = q.limit {
        return Err(ApiError::Malformed(
            "limit=0 vô nghĩa: bỏ hẳn tham số nếu muốn lấy tất cả".into(),
        ));
    }
    tokio::task::spawn_blocking(move || dirty_blocking(&st, q.limit))
        .await
        .map_err(|e| ApiError::Malformed(format!("tác vụ đọc _dirty hỏng: {e}")))
        .map(Json)
}

/// Phần THUẦN của [`dirty`] — không async, không I/O ngoài việc khoá từng ref.
fn dirty_blocking(st: &AppState, limit: Option<usize>) -> DirtyResp {
    let mut out: Vec<DirtyRefResp> = Vec::new();

    for (ref_id, entry) in st.store.all() {
        let g = lock(&entry);
        let head_seq = g.chain.head().seq;
        let anchored_seq = g.anchored.as_ref().map(|a| a.seq);

        // Chưa neo lần nào ⇒ CẢ genesis cũng đang chờ. Lấy `head_seq` làm số version
        // chờ ở ca này là bỏ sót đúng một version, và bỏ sót ở ca **duy nhất** mà
        // lineage chưa có gì trên chuỗi để đối chiếu.
        let (pending_versions, first_unanchored) = match anchored_seq {
            Some(a) if head_seq > a => (head_seq - a, a + 1),
            Some(_) => continue, // đã neo tới head: sạch, không thuộc `_dirty`.
            None => (head_seq + 1, 0),
        };

        // `ts` của version cũ nhất chưa neo. Version luôn tồn tại trong phạm vi này
        // (`first_unanchored <= head_seq`), nhưng vẫn fallback về head thay vì panic:
        // một route CHỈ ĐỌC không được là chỗ giết daemon.
        let oldest_unanchored_ts = g
            .chain
            .version(first_unanchored)
            .unwrap_or_else(|| g.chain.head())
            .ts;
        let author_did = g
            .chain
            .version(0)
            .unwrap_or_else(|| g.chain.head())
            .author_did;

        out.push(DirtyRefResp {
            ref_id: hex::encode(ref_id),
            author_did: hex::encode(author_did),
            head_seq,
            anchored_seq,
            pending_versions,
            oldest_unanchored_ts,
        });
    }

    // Cũ trước, mới sau — cam kết ở DTO. `ref_id` là khoá phá hoà để hai lượt gọi
    // trên cùng trạng thái cho **cùng một thứ tự** (`HashMap` không có thứ tự).
    out.sort_by(|a, b| {
        a.oldest_unanchored_ts
            .cmp(&b.oldest_unanchored_ts)
            .then_with(|| a.ref_id.cmp(&b.ref_id))
    });

    let truncated = matches!(limit, Some(n) if out.len() > n);
    if let Some(n) = limit {
        out.truncate(n);
    }

    DirtyResp {
        count: out.len(),
        total_pending_versions: out.iter().map(|r| r.pending_versions).sum(),
        oldest_unanchored_ts: out.iter().map(|r| r.oldest_unanchored_ts).min(),
        truncated,
        refs: out,
    }
}

/// `GET /v1/strata/_settlement_window?from_slot=&to_slot=` — **nguồn lá** của luồng
/// checkpoint toàn cục (`Specs#32` mục 10).
///
/// Trả mọi record anchor `t = 1` dưới label 1234, trong các tx **do publisher đã pin
/// CHI**, có slot ∈ `[from_slot, to_slot)`.
///
/// # Vì sao route này nằm ở kho NÀY
///
/// Bên tính `root` là Mosaic, nhưng decoder label 1234 và luật tin cậy
/// (*"chỉ tin tx do publisher chi"*) là **chain logic** và đã có **đúng một** bản ở
/// `settlement.rs`. Dựng bản thứ hai bên Mosaic để đọc cùng byte đó là đúng lớp lỗi
/// `stamp_id` 32-vs-36 byte: hai bộ giải mã cho một sự thật, lệch nhau vào ngày
/// không ai đang nhìn — và ở đây "lệch" nghĩa là hai bên tính ra hai `root` khác
/// nhau cho cùng một chu kỳ, tức cam kết on-chain trở thành không kiểm được.
///
/// # Fail-closed
///
/// Quét không phủ hết cửa sổ ⇒ **502 `AnchorRejected`**, không phải danh sách ngắn.
/// Xem `WindowResp` cho lý do đầy đủ.
///
/// Route này **không** quyết cửa sổ đã đủ sâu để đóng chưa — nó trả `tip_slot` để
/// bên gọi tự quyết. Độ sâu an toàn là tham số của mạng và của khẩu vị rủi ro; đặt
/// nó vào phép đọc là ghim một hằng vào sai tầng.
async fn settlement_window(
    State(st): State<AppState>,
    Query(q): Query<WindowQuery>,
) -> ApiResult<Json<WindowResp>> {
    if q.to_slot <= q.from_slot {
        return Err(ApiError::Malformed(format!(
            "cửa sổ rỗng hoặc lùi: [{}, {}) — `from_slot` ĐÓNG, `to_slot` MỞ",
            q.from_slot, q.to_slot
        )));
    }
    let scan = tokio::task::spawn_blocking(move || st.sink.scan_window(q.from_slot, q.to_slot))
        .await
        .map_err(|e| ApiError::Malformed(format!("tác vụ quét cửa sổ hỏng: {e}")))??;

    let anchors: Vec<WindowAnchorResp> = scan
        .anchors
        .iter()
        .map(|w| WindowAnchorResp {
            ref_id: hex::encode(w.anchor.ref_id),
            head_version_hash: hex::encode(w.anchor.head_version_hash),
            mmr_root: hex::encode(w.anchor.mmr_root),
            seq: w.anchor.seq,
            slot: w.slot,
            txid: w.txid.clone(),
        })
        .collect();
    Ok(Json(WindowResp {
        from_slot: scan.from_slot,
        to_slot: scan.to_slot,
        tip_slot: scan.tip_slot,
        scanned_txs: scan.scanned_txs,
        count: anchors.len(),
        anchors,
    }))
}

/// `POST /v1/strata/_anchor_batch` — neo N ref trong MỘT tx.
///
/// Đây là cửa mà `BatchCoordinator` phía Mosaic đi vào. Phân vai (đã chốt ở
/// `docs/STRATA-ANCHOR-INTEGRATION-REPORT.md` §9.6):
///
/// - **Mosaic** quyết *khi nào* bắn lô và lô gồm *ref nào* — nó có hàng đợi ưu
///   tiên + ngưỡng depth/age, và neo là việc của nó.
/// - **Strata** (route này) kiểm INV-E7 từng ref rồi encode label 1234 — `resolve`
///   là chain logic, và encoder giữ **một** bản.
/// - **Mosaic** dựng tx + ký + submit.
///
/// ⚠️ Đi vòng qua route này (Mosaic tự gom lô rồi submit thẳng) là **mất gác chống
/// rollback**: lô vẫn lên chuỗi, tx vẫn confirmed, và không còn ai chặn một anchor
/// tụt-lùi-seq. Không lỗi nào bật ra.
async fn anchor_batch(
    State(st): State<AppState>,
    req: Result<Json<AnchorBatchReq>, JsonRejection>,
) -> ApiResult<Json<AnchorBatchResp>> {
    let req = body(req)?;
    tokio::task::spawn_blocking(move || anchor_batch_blocking(&st, &req.refs, req.priority.into()))
        .await
        .map_err(|e| ApiError::Malformed(format!("tác vụ neo lô hỏng: {e}")))?
        .map(Json)
}

/// Cùng trình tự **kiểm → đẩy → chốt** như neo lẻ, nhưng giữ khoá của **mọi** ref
/// trong lô suốt cả ba bước.
///
/// Hai chỗ khác biệt so với neo lẻ, cả hai đều là chuyện đúng-sai chứ không phải
/// tối ưu:
///
/// 1. **Khoá theo thứ tự ref_id đã sắp.** Hai lô giao nhau mà khoá theo thứ tự
///    người gọi gửi lên thì hai luồng có thể giữ chéo khoá của nhau — deadlock,
///    và nó chỉ xuất hiện dưới tải.
/// 2. **Trùng ref trong một lô bị TỪ CHỐI.** Hai entry cùng lineage trong một tx:
///    entry sau mang cùng `seq` với entry trước, nên khi đọc lại chỉ một cái sống,
///    còn lõi thì đã chốt `publish_anchor()` hai lần. Không có thứ tự nào cứu được,
///    nên chặn ở cửa.
fn anchor_batch_blocking(
    st: &AppState,
    refs: &[String],
    priority: AnchorPriority,
) -> ApiResult<AnchorBatchResp> {
    if refs.is_empty() {
        return Err(ApiError::Malformed("lô rỗng: cần ít nhất một ref".into()));
    }
    let mut ids: Vec<Hash32> = refs
        .iter()
        .map(|r| parse_ref(r))
        .collect::<Result<_, _>>()?;
    ids.sort_unstable();
    let before = ids.len();
    ids.dedup();
    if ids.len() != before {
        return Err(ApiError::Malformed(
            "lô có ref trùng: hai anchor cùng một lineage trong MỘT tx thì cái sau là rollback \
             của cái trước"
                .into(),
        ));
    }

    let entries: Vec<_> = ids
        .iter()
        .map(|id| st.store.get(id).ok_or(ApiError::NotFound("ref")))
        .collect::<Result<_, _>>()?;
    let mut guards: Vec<_> = entries.iter().map(|e| lock(e)).collect();

    // 1. Kiểm rollback bằng gương của daemon, TOÀN LÔ trước khi đẩy bất cứ gì.
    let mut anchors = Vec::with_capacity(guards.len());
    for g in &guards {
        let a = g.chain.anchor();
        if let Some(prev) = &g.anchored
            && a.seq <= prev.seq
        {
            return Err(StrataError::AnchorRollback {
                current: prev.seq,
                attempted: a.seq,
            }
            .into());
        }
        anchors.push(a);
    }

    // `no_anchor` = không đẩy và KHÔNG chốt seq (§4.2) — trả về đúng những gì SẼ
    // neo, để bên gọi xem trước được lô mà không tiêu một tx nào.
    //
    // ⚠️ **Xem trước phải chạy ĐỦ gác, nếu không nó nói dối.** Bản đầu chỉ đi qua
    // gương `anchored` của daemon rồi trả sớm — mà gác đắt nhất nằm ở chỗ khác:
    // `publish_batch` đọc **on-chain** rồi so `seq`. Hai chỗ đó lệch nhau ở đúng ca
    // nguy hiểm: daemon vừa restart (gương rỗng) trong khi trên chuỗi ref đã ở `seq`
    // cao hơn. Khi ấy `no_anchor` trả "lô ổn", còn lượt neo thật thì `RollbackAttempt`
    // — và bên gọi dùng `no_anchor` để **tìm ref hỏng** sẽ kết luận *"không ref nào
    // hỏng"* rồi thử lại y nguyên, mãi mãi. (Phát hiện khi chạy thật Preprod
    // 2026-08-20, không phát hiện được khi đọc mã.)
    if matches!(priority, AnchorPriority::NoAnchor) {
        let ref_ids: Vec<Hash32> = anchors.iter().map(|a| a.ref_id).collect();
        let on_chain = st.sink.resolve_many(&ref_ids)?;
        for a in &anchors {
            if let Some(c) = on_chain.iter().find(|c| c.ref_id == a.ref_id)
                && c.seq > a.seq
            {
                return Err(AnchorError::RollbackAttempt {
                    on_chain_seq: c.seq,
                    attempted: a.seq,
                }
                .into());
            }
        }
        return Ok(AnchorBatchResp {
            anchor_txid: None,
            backend: None,
            batch_size: anchors.len(),
            anchors: anchors
                .iter()
                .map(|a| AnchorResp::new(a, None, None))
                .collect(),
        });
    }

    // 2. Đẩy on-chain — MỘT lượt cho cả lô.
    let receipt = st.sink.publish_many(&anchors, priority)?;
    let (txid, backend) = match &receipt {
        Some(rc) => (
            Some(rc.txid.clone()),
            Some(backend_name(rc.backend).to_string()),
        ),
        None => (None, None),
    };

    // 3. On-chain đã nhận ⇒ mới chốt `last_anchor_seq` ở lõi, cho từng ref.
    let mut out = Vec::with_capacity(guards.len());
    let mut recs = Vec::with_capacity(guards.len());
    for g in &mut guards {
        let committed = g.chain.publish_anchor()?;
        g.anchored = Some(AnchorState {
            seq: committed.seq,
            txid: txid.clone(),
            backend: backend.clone(),
        });
        recs.push(JournalRecord::Anchor {
            r: ref_hex(&committed.ref_id),
            seq: committed.seq,
            txid: txid.clone(),
            backend: backend.clone(),
        });
        out.push(AnchorResp::new(&committed, txid.clone(), backend.clone()));
    }
    // MỘT lượt fsync cho cả lô: lô là **một tx**, nên nó cũng là một sự kiện bền vững —
    // N lượt fsync cho một thứ đã tất định là trả giá cho không gì cả. Mọi khoá ref của
    // lô vẫn đang được giữ ở đây, nên không ai chen vào giữa được.
    if let Some(j) = st.store.journal() {
        j.append_many(&recs).map_err(journal_err)?;
    }

    Ok(AnchorBatchResp {
        anchor_txid: txid,
        backend,
        batch_size: out.len(),
        anchors: out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ts` giây thật của hôm nay (2026) — mốc "phải qua" trong mọi ca dưới đây.
    const TS_SECS_TODAY: u64 = 1_786_000_000;
    /// `Date.now()` đương thời tính bằng **mili giây** — đúng ca lớp 1 sinh ra để bắt.
    const TS_MILLIS_TODAY: u64 = 1_786_000_000_000;

    /// Lớp 1 phải chặn `ts` mili giây **khi không có đồng hồ nào** để dựa vào.
    ///
    /// Đây là bài kiểm mà bộ test qua HTTP không viết được: ở đó `now` luôn là đồng hồ thật,
    /// nên lớp 2 bắt hộ và lớp 1 có gỡ đi cũng không ai thấy.
    #[test]
    fn tran_tuyet_doi_chan_ts_mili_giay_ngay_ca_khi_khong_co_dong_ho() {
        assert!(
            check_ts_at(TS_MILLIS_TODAY, None).is_err(),
            "không đọc được đồng hồ thì lớp 1 là gác DUY NHẤT còn lại — nó phải chặn"
        );
        assert!(
            check_ts_at(TS_MILLIS_FLOOR, None).is_err(),
            "biên: >= là chặn"
        );
    }

    /// Vế còn lại của cùng một ca: không có đồng hồ thì `ts` giây **hợp lệ** vẫn phải qua.
    ///
    /// Thiếu vế này thì `check_ts_at(_, None) -> Err` luôn cũng làm bài trên xanh, và daemon
    /// trên máy đồng hồ hỏng sẽ từ chối sạch mọi ghi mà bộ kiểm vẫn báo đạt.
    #[test]
    fn khong_co_dong_ho_thi_ts_giay_that_van_qua() {
        assert!(check_ts_at(TS_SECS_TODAY, None).is_ok());
        assert!(check_ts_at(0, None).is_ok(), "ts quá khứ KHÔNG bị cửa chặn");
    }

    /// Lớp 2 vẫn phải làm việc của nó: tương lai gần thì chỉ có đồng hồ mới bắt được, vì
    /// những giá trị đó nằm xa dưới `TS_MILLIS_FLOOR`.
    #[test]
    fn bien_lech_bat_tuong_lai_gan_ma_tran_tuyet_doi_khong_thay() {
        let now = TS_SECS_TODAY;
        assert!(
            check_ts_at(now + MAX_TS_SKEW_SECS, Some(now)).is_ok(),
            "đúng biên là qua"
        );
        assert!(check_ts_at(now + MAX_TS_SKEW_SECS + 1, Some(now)).is_err());
        assert!(
            check_ts_at(now + MAX_TS_SKEW_SECS + 1, None).is_ok(),
            "chính vì lớp 2 im khi không có đồng hồ nên lớp 1 mới cần tồn tại"
        );
    }
}
