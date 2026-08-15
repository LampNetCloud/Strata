//! Chọn `AnchorSink` cho daemon từ ENV.
//!
//! Trước đây `strata_node.rs` cắm cứng [`DisabledSink`](crate::DisabledSink) và ghi
//! *"cắm sink thật là việc của bản triển khai"* — nghĩa là **không có** bản triển
//! khai nào: mọi neo thật trả 501, và đường `OriLife → Strata → Mosaic` không chạy
//! được ngoài test. Module này là bản triển khai đó.
//!
//! # Hình dạng đã chọn (B1′)
//!
//! ```text
//! SettlementSink {
//!     query     : BlockfrostQuery      (đọc on-chain — resolve, INV-E7)
//!     submitter : MosaicDoorSubmitter  (đẩy lô sang cửa Mosaic — Mosaic dựng tx)
//! }
//! ```
//!
//! `submit.ts` **không** nằm trong đường này. Đó là chủ ý: luật `#1` nói *"Mosaic
//! giữ tx; KHÔNG dựng tx neo trong Strata"*, và `submit.ts` là chỗ đã vượt luật.
//!
//! # Vì sao mọi cấu hình thiếu đều là LỖI KHỞI ĐỘNG, không phải cảnh báo
//!
//! Một daemon lên xanh với sink nửa-cấu-hình sẽ trả 501/`NotConfigured` ở lượt neo
//! đầu tiên — tức lỗi hiện ra **sau khi** dữ liệu đã đi vào, ở chỗ khó truy nhất.
//! Rẻ hơn nhiều: chết ngay lúc khởi động, in đúng biến còn thiếu.

use lampnet_anchor_io::BlockfrostQuery;
use lampnet_anchor_io::mosaic_door::MosaicDoorSubmitter;
use lampnet_strata::AnchorSink;
use lampnet_strata::settlement::{METADATA_LABEL, SettlementSink, SinkConfig};
use std::sync::Arc;

/// Backend đang chọn.
pub const BACKEND_ENV: &str = "STRATA_ANCHOR_BACKEND";
/// Ví publisher đã pin — anchor chỉ hợp lệ nếu tx phát từ ví này (§4.3).
pub const PUBLISHER_ENV: &str = "STRATA_PUBLISHER_ADDRESS";
/// Mạng đọc on-chain: `preview` | `preprod` | `mainnet`.
pub const NETWORK_ENV: &str = "STRATA_ANCHOR_NETWORK";
/// `policy_id` hex 56 ⇒ `resolve` chạy chế độ beacon (miễn nhiễm flood-eviction).
pub const BEACON_POLICY_ENV: &str = "STRATA_BEACON_POLICY";
/// `1` ⇒ mỗi lô submit kèm mint/di chuyển beacon.
pub const BEACON_SUBMIT_ENV: &str = "STRATA_ANCHOR_BEACON";
/// Trần số tx quét khi `resolve` ở chế độ legacy.
pub const SCAN_LIMIT_ENV: &str = "STRATA_RESOLVE_SCAN_LIMIT";

/// Mạng → (REST base Blockfrost, các env có thể chứa project-id).
fn network_endpoints(net: &str) -> Result<(&'static str, &'static [&'static str]), String> {
    match net {
        "preview" => Ok((
            "https://cardano-preview.blockfrost.io/api/v0",
            &["BLOCKFROST_TOKEN_GREENSUN", "oBLOCKFROST_API_KEY_preview"],
        )),
        "preprod" => Ok((
            "https://cardano-preprod.blockfrost.io/api/v0",
            &["BLOCKFROST_API_KEY", "BLOCKFROST_API_KEY_PREPROD"],
        )),
        "mainnet" => Ok((
            "https://cardano-mainnet.blockfrost.io/api/v0",
            &["Blockfrost_ThanhDuc", "BLOCKFROST_API_KEY_MAINNET"],
        )),
        other => Err(format!(
            "{NETWORK_ENV}=`{other}` không hợp lệ (preview | preprod | mainnet)"
        )),
    }
}

/// Đọc env qua một hàm cấp — để test không phải đụng env toàn tiến trình (và
/// không đua với test chạy song song).
pub type EnvGet<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Mô tả sink đã chọn, kèm dòng log **không chứa secret** cho lúc khởi động.
///
/// `Debug` chỉ in `description` — `sink` cầm token cửa + project-id Blockfrost, và
/// một `{:?}` trong thông điệp test/log là đủ để hai secret đó ra ngoài.
pub struct SinkChoice {
    pub sink: Arc<dyn AnchorSink + Send + Sync>,
    pub description: String,
}

impl std::fmt::Debug for SinkChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SinkChoice")
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

/// Dựng sink từ ENV. `Err` ⇒ daemon **không khởi động**.
pub fn build_sink(get: EnvGet<'_>) -> Result<SinkChoice, String> {
    let backend = get(BACKEND_ENV)
        .map(|b| b.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "disabled".into());

    match backend.as_str() {
        "disabled" => Ok(SinkChoice {
            sink: Arc::new(crate::DisabledSink),
            description: format!(
                "backend=disabled (mọi neo thật → 501). Đặt {BACKEND_ENV}=settlement để neo thật."
            ),
        }),
        "memory" => Ok(SinkChoice {
            sink: Arc::new(crate::MemorySink::new()),
            description: "backend=memory (KHÔNG đụng chuỗi — chỉ dùng cho dev/test)".into(),
        }),
        "settlement" => build_settlement(get),
        other => Err(format!(
            "{BACKEND_ENV}=`{other}` không hợp lệ (disabled | memory | settlement)"
        )),
    }
}

fn build_settlement(get: EnvGet<'_>) -> Result<SinkChoice, String> {
    let publisher = get(PUBLISHER_ENV)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            format!(
                "{PUBLISHER_ENV} còn trống. Đây là ví publisher đã PIN: `resolve` chỉ tin tx do \
                 chính ví này CHI. Thiếu nó thì mọi anchor vừa đẩy sẽ bị chính resolve() bỏ qua."
            )
        })?;

    let net = get(NETWORK_ENV)
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "preprod".into());
    let (base, token_envs) = network_endpoints(&net)?;
    let token = token_envs
        .iter()
        .find_map(|k| get(k).filter(|v| !v.trim().is_empty()))
        .ok_or_else(|| {
            format!("thiếu project-id Blockfrost cho {net}: đặt một trong {token_envs:?}")
        })?;

    let beacon_submit = get(BEACON_SUBMIT_ENV).is_some_and(|v| matches!(v.trim(), "1" | "true"));
    let beacon_policy = get(BEACON_POLICY_ENV)
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty());

    if let Some(p) = &beacon_policy {
        if p.len() != 56 || !p.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "{BEACON_POLICY_ENV} phải là policy_id hex 56 ký tự (28 byte), đang là {} ký tự",
                p.len()
            ));
        }
        // Đọc bằng beacon mà ghi KHÔNG mint beacon ⇒ `resolve` không bao giờ thấy
        // asset ⇒ trả `None` cho mọi ref ⇒ gác idempotency/rollback của
        // `publish_batch` im lặng ngừng hoạt động. Không lỗi nào bật ra.
        if !beacon_submit {
            return Err(format!(
                "{BEACON_POLICY_ENV} đang bật (resolve tra beacon) nhưng {BEACON_SUBMIT_ENV} tắt \
                 (submit KHÔNG mint beacon): resolve sẽ trả None cho mọi ref và gác chống \
                 rollback im lặng ngừng hoạt động. Bật cả hai, hoặc tắt cả hai."
            ));
        }
    }

    let scan_limit = match get(SCAN_LIMIT_ENV) {
        Some(v) => v
            .trim()
            .parse::<usize>()
            .map_err(|e| format!("{SCAN_LIMIT_ENV} phải là số: {e}"))?,
        None => SinkConfig::default().resolve_scan_limit,
    };

    let submitter = MosaicDoorSubmitter::from_env_with(get, METADATA_LABEL, beacon_submit)?
        .ok_or_else(|| {
            format!(
                "backend=settlement nhưng chưa có {}: dưới B1′, tx neo do MOSAIC dựng — Strata \
                 chỉ kiểm INV-E7 + encode rồi đẩy lô sang cửa Mosaic.",
                lampnet_anchor_io::mosaic_door::DOOR_URL_ENV
            )
        })?
        .with_network(Some(net.clone()));

    let cfg = SinkConfig {
        publisher_address: publisher.clone(),
        label: METADATA_LABEL,
        resolve_scan_limit: scan_limit,
        beacon_policy: beacon_policy.clone(),
        ..SinkConfig::default()
    };
    let query = BlockfrostQuery::new(base.to_string(), token.trim().to_string());
    let description = format!(
        "backend=settlement label={METADATA_LABEL} mạng={net} publisher={publisher} \
         beacon_submit={beacon_submit} beacon_resolve={} scan_limit={scan_limit} \
         (tx do cửa Mosaic dựng)",
        beacon_policy.as_deref().unwrap_or("tắt")
    );
    Ok(SinkChoice {
        sink: Arc::new(SettlementSink::new(cfg, query, submitter)),
        description,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn build(pairs: &[(&str, &str)]) -> Result<SinkChoice, String> {
        let map = env(pairs);
        build_sink(&|k| map.get(k).cloned())
    }

    fn full() -> Vec<(&'static str, &'static str)> {
        vec![
            (BACKEND_ENV, "settlement"),
            (PUBLISHER_ENV, "addr_test1_publisher"),
            (NETWORK_ENV, "preprod"),
            ("BLOCKFROST_API_KEY", "preprod_token"),
            ("MOSAIC_DOOR_URL", "http://127.0.0.1:6691"),
            ("MOSAIC_DOOR_TOKEN", "door-token"),
        ]
    }

    #[test]
    fn mac_dinh_la_disabled() {
        let c = build(&[]).unwrap();
        assert!(c.description.contains("disabled"));
    }

    #[test]
    fn settlement_du_cau_hinh_thi_dung_len() {
        let c = build(&full()).unwrap();
        assert!(c.description.contains("backend=settlement"));
        assert!(c.description.contains("preprod"));
        // Mô tả KHÔNG được chứa token — nó đi thẳng vào log khởi động.
        assert!(!c.description.contains("preprod_token"));
        assert!(!c.description.contains("door-token"));
    }

    #[test]
    fn thieu_publisher_thi_khong_khoi_dong() {
        let mut e = full();
        e.retain(|(k, _)| *k != PUBLISHER_ENV);
        let err = build(&e).unwrap_err();
        assert!(err.contains(PUBLISHER_ENV), "{err}");
    }

    #[test]
    fn thieu_cua_mosaic_thi_khong_khoi_dong() {
        let mut e = full();
        e.retain(|(k, _)| *k != "MOSAIC_DOOR_URL");
        let err = build(&e).unwrap_err();
        assert!(err.contains("MOSAIC_DOOR_URL"), "{err}");
    }

    #[test]
    fn co_url_ma_thieu_token_cua_la_loi_chu_khong_phai_chua_cau_hinh() {
        let mut e = full();
        e.retain(|(k, _)| *k != "MOSAIC_DOOR_TOKEN");
        let err = build(&e).unwrap_err();
        assert!(err.contains("MOSAIC_DOOR_TOKEN"), "{err}");
    }

    #[test]
    fn thieu_token_blockfrost_thi_khong_khoi_dong() {
        let mut e = full();
        e.retain(|(k, _)| *k != "BLOCKFROST_API_KEY");
        let err = build(&e).unwrap_err();
        assert!(err.contains("Blockfrost"), "{err}");
    }

    /// Đọc-bằng-beacon mà ghi-không-beacon: `resolve` trả None cho MỌI ref, gác
    /// idempotency/rollback tắt trong im lặng. Phải chặn ở khởi động.
    #[test]
    fn beacon_resolve_bat_ma_beacon_submit_tat_bi_chan() {
        let mut e = full();
        e.push((BEACON_POLICY_ENV, "ab".repeat(28).leak()));
        let err = build(&e).unwrap_err();
        assert!(err.contains(BEACON_SUBMIT_ENV), "{err}");
    }

    #[test]
    fn beacon_bat_ca_hai_thi_qua() {
        let mut e = full();
        e.push((BEACON_POLICY_ENV, "ab".repeat(28).leak()));
        e.push((BEACON_SUBMIT_ENV, "1"));
        let c = build(&e).unwrap();
        assert!(c.description.contains("beacon_submit=true"));
    }

    #[test]
    fn policy_id_sai_do_dai_bi_chan() {
        let mut e = full();
        e.push((BEACON_POLICY_ENV, "abcd"));
        e.push((BEACON_SUBMIT_ENV, "1"));
        assert!(build(&e).unwrap_err().contains(BEACON_POLICY_ENV));
    }

    #[test]
    fn mang_la_bi_chan() {
        let mut e = full();
        e.retain(|(k, _)| *k != NETWORK_ENV);
        e.push((NETWORK_ENV, "prepod"));
        assert!(build(&e).unwrap_err().contains("prepod"));
    }
}
