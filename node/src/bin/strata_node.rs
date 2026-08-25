//! Binary daemon tối thiểu — dựng router §3 rồi phục vụ.
//!
//! Cấu hình qua ENV:
//! - `STRATA_NODE_ADDR` — địa chỉ lắng nghe (mặc định `127.0.0.1:6690`).
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
