//! Binary daemon tối thiểu — dựng router §3 rồi phục vụ.
//!
//! Cấu hình qua ENV:
//! - `STRATA_NODE_ADDR` — địa chỉ lắng nghe (mặc định `127.0.0.1:6690`).
//! - `STRATA_NODE_KEYS` — nạp key-registry: `did_hex32:pubkey_hex32` ngăn bởi dấu phẩy.
//!   Chỉ nạp **khoá công khai** — daemon không bao giờ cầm khoá bí mật (nó không ký).
//!   Bản thật thay bằng PhoenixKey resolver qua trait [`KeyRegistry`].
//!
//! Backend neo mặc định là `DisabledSink` (mọi neo thật → 501): cắm `MosaicAnchorSink`
//! hoặc `SettlementSink` là việc của bản triển khai thật, cần khoá + endpoint chuỗi.

use ed25519_dalek::VerifyingKey;
use lampnet_strata_node::{AppState, ChainStore, DisabledSink, InMemoryRegistry, router};
use std::sync::Arc;

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::var("STRATA_NODE_ADDR").unwrap_or_else(|_| "127.0.0.1:6690".to_string());
    let registry = load_registry(&std::env::var("STRATA_NODE_KEYS").unwrap_or_default())?;
    let n_keys = registry.len();

    let state = AppState::new(
        Arc::new(ChainStore::new()),
        Arc::new(registry),
        Arc::new(DisabledSink),
    );

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!(
        "strata-node nghe tại http://{addr} — route §3 dưới /v1/strata, {n_keys} khoá trong registry"
    );
    axum::serve(listener, router(state)).await?;
    Ok(())
}
