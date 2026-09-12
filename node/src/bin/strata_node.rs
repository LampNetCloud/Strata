//! Binary daemon tối thiểu — dựng router §3 rồi phục vụ.
//!
//! Cấu hình qua ENV:
//! - `STRATA_NODE_ADDR` — địa chỉ lắng nghe (mặc định `127.0.0.1:6690`).
//! - `STRATA_NODE_EXPOSED` — **bắt buộc KHI** `STRATA_NODE_ADDR` nghe ngoài loopback. Giá
//!   trị duy nhất: `auth-gateway-in-front`. Đường GHI (`/anchor`, `/_anchor_batch`) hiện
//!   **không có tầng xác thực nào** (issue #77 — lỗ ở spec, không phải thiếu middleware),
//!   nên phơi nó ra ngoài loopback là việc người vận hành phải **khai**, không phải việc
//!   xảy ra vì không ai nghĩ tới. Xem [`check_listen_exposure`].
//! - `STRATA_NODE_JOURNAL` — **bắt buộc**. Đường dẫn tệp nhật ký bền vững, hoặc chuỗi
//!   `none` để chạy **phù du** (mất sạch khi restart).
//! - `STRATA_NODE_KEYS` — nạp key-registry: `did_hex32:pubkey_hex32` ngăn bởi dấu phẩy.
//!   Chỉ nạp **khoá công khai** — daemon không bao giờ cầm khoá bí mật (nó không ký).
//!   Bản thật thay bằng PhoenixKey resolver qua trait [`KeyRegistry`].
//!
//! - Backend neo: chọn qua `STRATA_ANCHOR_BACKEND` (`disabled` mặc định | `memory` |
//!   `settlement`) — xem [`sink_config`](lampnet_strata_node::sink_config) để biết
//!   danh sách biến của từng backend. Cấu hình **thiếu là lỗi khởi động**, không
//!   phải cảnh báo: một daemon lên xanh với sink nửa-cấu-hình chỉ lộ ra ở lượt neo
//!   đầu tiên, tức sau khi dữ liệu đã đi vào.

use ed25519_dalek::VerifyingKey;
use lampnet_strata_node::{
    AppState, ChainStore, InMemoryRegistry, Journal, KeyRegistry, build_sink, read_records,
    replay_into, router,
};
use std::sync::Arc;
use std::time::Instant;

/// Vì sao `STRATA_NODE_JOURNAL` **bắt buộc**, chứ không mặc định phù du.
///
/// Chạy phù du là một lựa chọn hợp lệ (dev, test, một lượt thử). Mất hồ sơ vì **không ai
/// nghĩ tới nó** thì không. Hai ca ấy phân biệt được bằng đúng một thứ: người vận hành đã
/// **nói ra** hay chưa.
///
/// Cùng khuôn với `--commit` của bootstrap checkpoint và với "beacon mặc định TẮT": chỗ
/// nào mất mát không lấy lại được thì mặc định phải là chỗ đòi người vận hành khai.
const JOURNAL_ENV: &str = "STRATA_NODE_JOURNAL";

/// Khai rằng có một lớp gác xác thực đứng TRƯỚC daemon (issue #77 mục 1).
const EXPOSED_ENV: &str = "STRATA_NODE_EXPOSED";
/// Giá trị duy nhất được nhận. Là một **câu khẳng định**, không phải `1`/`true`: giá trị
/// kiểu cờ bật được do gõ nhầm hoặc do chép một tệp env của môi trường khác; câu này thì
/// người đặt phải biết mình đang khai cái gì.
const EXPOSED_DECLARATION: &str = "auth-gateway-in-front";

/// Địa chỉ nghe có nằm ngoài loopback không — **fail-closed khi không chắc**.
///
/// Parse được thành `SocketAddr` thì dùng `is_loopback()`. Không parse được (tên miền:cổng)
/// thì chỉ `localhost` được coi là loopback; **mọi thứ khác coi là phơi ra**. Không đi phân
/// giải DNS: một phép đo phụ thuộc trạng thái mạng lúc khởi động sẽ cho hai kết luận khác
/// nhau ở hai lượt chạy của cùng một cấu hình.
fn listens_off_loopback(addr: &str) -> bool {
    if let Ok(sa) = addr.parse::<std::net::SocketAddr>() {
        return !sa.ip().is_loopback();
    }
    let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr);
    !host.trim().eq_ignore_ascii_case("localhost")
}

/// Cổng khởi động cho đường GHI không có xác thực (issue #77 mục 1).
///
/// # Vì sao đây là chỗ vá được ngay, còn phần còn lại của #77 thì không
///
/// `#77` đề nghị thêm middleware xác thực cho `/anchor` + `/_anchor_batch`. Chủ spec chỉnh
/// lại: đó là **lỗ ở spec**, không phải thiếu một bản vá middleware — `Strata-API.md §3`
/// đặc tả thân yêu cầu của `/anchor` đúng bằng `{"priority":"immediate"}`, không có ô nào
/// cho xác thực. Mã đang khớp một đặc tả **thiếu**, nên thêm trường xác thực là **đổi hình
/// dạng trên dây** ⇒ quyết định spec.
///
/// Cái vá được ngay là cổng này, và nó không đụng dây một byte nào.
///
/// # Vì sao "mặc định nghe loopback" không đỡ được
///
/// Mặc định là `127.0.0.1:6690`, nhưng `STRATA_NODE_ADDR` nhận địa chỉ bất kỳ mà **không
/// một dòng cảnh báo nào**. Trong container hoặc mạng nội bộ dùng chung thì "localhost"
/// không còn là ranh giới tin cậy. Bất đối xứng nằm đúng chỗ đau: thêm một *version* phải
/// có chữ ký Ed25519 hợp lệ, còn bắt daemon **tiêu tiền và chốt trạng thái** thì không cần
/// gì — và `publish_anchor()` đẩy `last_anchor_seq` tiến lên là việc **không hoàn tác
/// được** (INV-E7 cấm neo lùi ⇒ mọi lượt neo thật sau đó trả `AnchorRollback`).
///
/// Cùng khuôn với `STRATA_NODE_JOURNAL`: **chỗ nào mất mát không lấy lại được thì mặc định
/// phải là chỗ đòi người vận hành KHAI.** Cổng này không kiểm được rằng lớp gác kia có thật
/// — nó chỉ đảm bảo không ai phơi đường ghi ra ngoài loopback mà **không biết mình đang
/// làm thế**.
fn check_listen_exposure(
    addr: &str,
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<String>, String> {
    if !listens_off_loopback(addr) {
        return Ok(None);
    }
    match get(EXPOSED_ENV).map(|v| v.trim().to_string()) {
        Some(v) if v == EXPOSED_DECLARATION => Ok(Some(format!(
            "⚠️  nghe NGOÀI loopback ({addr}) — đã khai `{EXPOSED_ENV}={EXPOSED_DECLARATION}`; \
             đường GHI (/anchor, /_anchor_batch) KHÔNG có xác thực ở tầng này, lớp gác đứng \
             trước chịu trách nhiệm"
        ))),
        Some(v) => Err(format!(
            "`{EXPOSED_ENV}` phải đúng bằng `{EXPOSED_DECLARATION}`, nhận `{v}`"
        )),
        None => Err(format!(
            "từ chối khởi động: `STRATA_NODE_ADDR={addr}` nghe NGOÀI loopback mà đường GHI \
             không có tầng xác thực nào.\n\
             \n\
             `POST /v1/strata/:ref/anchor` và `POST /v1/strata/_anchor_batch` bắt daemon ký \
             lô bằng khoá operator, dựng tx, và **ví publisher trả phí** — không cần chữ ký \
             nào. Nặng hơn tiền: `publish_anchor()` đẩy `last_anchor_seq` tiến lên, mà \
             INV-E7 cấm neo lùi ⇒ một `seq` bị đẩy quá mức làm MỌI lượt neo thật sau đó trả \
             `AnchorRollback`. Không route sửa, không đường ghi đè.\n\
             \n\
             Hai đường đi tiếp:\n\
             1. nghe loopback (`127.0.0.1:6690`) và để một lớp gác chuyển tiếp vào; hoặc\n\
             2. đặt `{EXPOSED_ENV}={EXPOSED_DECLARATION}` để KHAI rằng đã có lớp gác xác \
             thực đứng trước.\n\
             \n\
             Cổng này không kiểm được lớp gác kia có thật — nó chỉ đảm bảo không ai phơi \
             đường ghi ra mà không biết mình đang làm thế."
        )),
    }
}

/// Dựng kho: replay nhật ký nếu có, hoặc kho phù du nếu người vận hành đã khai `none`.
fn build_store(registry: &dyn KeyRegistry) -> Result<Arc<ChainStore>, String> {
    let spec = std::env::var(JOURNAL_ENV).map_err(|_| {
        format!(
            "thiếu `{JOURNAL_ENV}`. Đặt đường dẫn tệp nhật ký để daemon dựng lại được \
             chính mình sau khi restart, hoặc đặt `{JOURNAL_ENV}=none` để khai RÕ rằng \
             lượt chạy này là phù du (restart = MẤT SẠCH hồ sơ cây: on-chain chỉ có \
             StrataAnchor 104 byte, không có version/sig/policy/fields nào để dựng lại)"
        )
    })?;

    if spec.trim() == "none" {
        println!(
            "⚠️  nhật ký: TẮT (`{JOURNAL_ENV}=none`) — kho PHÙ DU, restart là mất sạch hồ sơ cây"
        );
        return Ok(Arc::new(ChainStore::new()));
    }

    let journal = Arc::new(Journal::open(&spec).map_err(|e| format!("mở nhật ký `{spec}`: {e}"))?);
    let recs = read_records(&spec).map_err(|e| format!("{e}"))?;
    let store = ChainStore::with_journal(journal);

    let t0 = Instant::now();
    let stats = replay_into(&store, registry, &recs).map_err(|e| format!("{e}"))?;
    // In SỐ ĐO, không in "OK": một lượt replay đọc nhầm tệp rỗng cũng "OK".
    println!(
        "nhật ký: {spec} — replay {} bản ghi trong {:?}: {} ref · {} version · {} audit · {} neo",
        stats.records,
        t0.elapsed(),
        stats.refs,
        stats.versions,
        stats.audits,
        stats.anchors
    );
    Ok(Arc::new(store))
}

/// `did_hex:pk_hex,did_hex:pk_hex…` → registry. Sai định dạng ⇒ dừng hẳn (fail-closed:
/// chạy với registry thiếu khoá thì mọi ghi đều 424, im lặng còn tệ hơn).
fn load_registry(spec: &str) -> Result<InMemoryRegistry, String> {
    let reg = InMemoryRegistry::new();
    for (i, item) in spec.split(',').filter(|s| !s.trim().is_empty()).enumerate() {
        let (did_hex, pk_hex) = item
            .trim()
            .split_once(':')
            .ok_or_else(|| format!("STRATA_NODE_KEYS[{i}]: cần dạng did_hex:pubkey_hex"))?;
        let did: [u8; 32] = hex::decode(did_hex)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| format!("STRATA_NODE_KEYS[{i}]: did phải là hex 32 byte"))?;
        let pk_bytes: [u8; 32] = hex::decode(pk_hex)
            .ok()
            .and_then(|v| v.try_into().ok())
            .ok_or_else(|| format!("STRATA_NODE_KEYS[{i}]: pubkey phải là hex 32 byte"))?;
        let pk = VerifyingKey::from_bytes(&pk_bytes)
            .map_err(|e| format!("STRATA_NODE_KEYS[{i}]: pubkey Ed25519 không hợp lệ: {e}"))?;
        reg.register(did, pk);
    }
    Ok(reg)
}

/// ⚠️ **KHÔNG dùng `#[tokio::main]` ở đây.** Sink Settlement cầm client
/// `reqwest::blocking` (Blockfrost + cửa Mosaic), mà một client blocking **dựng bên
/// trong ngữ cảnh async sẽ panic** khi runtime nội bộ của nó bị drop: *"Cannot drop a
/// runtime in a context where blocking is not allowed"*. Nên trình tự là: dựng sink ở
/// ngữ cảnh **đồng bộ** trước, rồi mới mở runtime.
///
/// Thứ tự này cũng đúng về mặt vận hành: cấu hình neo hỏng thì daemon **không được
/// lên**, chứ không phải lên xanh rồi hỏng ở lượt neo đầu tiên.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::var("STRATA_NODE_ADDR").unwrap_or_else(|_| "127.0.0.1:6690".to_string());
    // Cổng đứng TRƯỚC mọi thứ khác: một daemon lên xanh rồi mới phát hiện mình đang phơi
    // đường ghi là phát hiện muộn theo nghĩa đắt nhất.
    if let Some(warn) = check_listen_exposure(&addr, &|k| std::env::var(k).ok())? {
        println!("{warn}");
    }
    let registry = load_registry(&std::env::var("STRATA_NODE_KEYS").unwrap_or_default())?;
    let n_keys = registry.len();

    let choice =
        build_sink(&|k| std::env::var(k).ok()).map_err(|e| format!("cấu hình neo: {e}"))?;
    println!("neo: {}", choice.description);

    // Replay TRƯỚC khi mở cổng: một daemon nhận request trong lúc còn đang dựng lại chính
    // mình sẽ trả 404 cho những ref nó sắp có, và ghi vào một chuỗi chưa đủ dài.
    let registry: Arc<dyn KeyRegistry> = Arc::new(registry);
    let store = build_store(registry.as_ref())?;

    let state = AppState::new(store, registry, choice.sink);

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            println!(
                "strata-node nghe tại http://{addr} — route §3 dưới /v1/strata, {n_keys} khoá \
                 trong registry"
            );
            axum::serve(listener, router(state)).await?;
            Ok::<_, Box<dyn std::error::Error>>(())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |k: &str| owned.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone())
    }

    #[test]
    fn loopback_thi_khong_doi_khai_gi() {
        for addr in [
            "127.0.0.1:6690",
            "[::1]:6690",
            "localhost:6690",
            "127.9.9.9:1",
        ] {
            assert_eq!(
                check_listen_exposure(addr, &env(&[])),
                Ok(None),
                "{addr} là loopback, không được đòi khai"
            );
        }
    }

    /// Ca chính của issue #77 mục 1.
    #[test]
    fn nghe_ngoai_loopback_ma_khong_khai_thi_khong_khoi_dong() {
        for addr in ["0.0.0.0:6690", "10.1.2.3:6690", "node.internal:6690"] {
            let r = check_listen_exposure(addr, &env(&[]));
            assert!(
                r.is_err(),
                "{addr} nghe ngoài loopback mà không khai gác phải TỪ CHỐI khởi động"
            );
            let msg = r.unwrap_err();
            assert!(
                msg.contains("AnchorRollback"),
                "lý do phải nói hệ quả KHÔNG hoàn tác được, không chỉ nói 'thiếu auth': {msg}"
            );
        }
    }

    /// Đối chứng DƯƠNG: khai đúng câu thì chạy, và có cảnh báo.
    #[test]
    fn nghe_ngoai_loopback_va_da_khai_thi_chay_kem_canh_bao() {
        let r = check_listen_exposure("0.0.0.0:6690", &env(&[(EXPOSED_ENV, EXPOSED_DECLARATION)]));
        let warn = r
            .expect("khai đúng thì phải chạy")
            .expect("phải có cảnh báo");
        assert!(warn.contains("NGOÀI loopback"), "{warn}");
    }

    /// Giá trị gần đúng KHÔNG được nhận — cờ bật kiểu `1`/`true` là thứ chép nhầm từ môi
    /// trường khác; câu khẳng định thì người đặt phải biết mình khai cái gì.
    #[test]
    fn gia_tri_khai_gan_dung_bi_tu_choi() {
        for v in ["1", "true", "yes", "auth-gateway", ""] {
            assert!(
                check_listen_exposure("0.0.0.0:6690", &env(&[(EXPOSED_ENV, v)])).is_err(),
                "`{v}` không phải câu khai hợp lệ"
            );
        }
    }

    /// Fail-closed khi KHÔNG parse được: một chuỗi lạ phải bị coi là phơi ra, không phải
    /// bỏ qua. (Đây là chiều hay hỏng: parse lỗi → `else` → cho chạy.)
    #[test]
    fn dia_chi_khong_parse_duoc_thi_coi_nhu_phoi_ra() {
        for addr in ["", "khong-phai-dia-chi", "::::"] {
            assert!(
                listens_off_loopback(addr),
                "`{addr}` không chắc là loopback ⇒ phải coi như phơi ra"
            );
        }
    }
}
