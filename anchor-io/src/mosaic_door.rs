//! [`MosaicDoorSubmitter`] — đẩy lô sang **cửa Mosaic** thay vì tự dựng tx.
//!
//! Đây là vế thi hành của luật `#1`: *"Strata giữ logic chain; **Mosaic giữ tx;
//! KHÔNG dựng tx neo trong Strata**"*. `submitter/submit.ts` đang dựng tx ngay
//! trong kho này — một chỗ **đã vượt luật**, không phải vùng xám. Submitter này là
//! bản thay thế: Strata vẫn giữ `publish_batch` (kiểm INV-E7) và `encode_records`
//! (một bản encoder duy nhất), nhưng **byte tx** do Mosaic dựng.
//!
//! ```text
//! publish_batch  ──resolve() từng anchor (INV-E7)──▶ encode_records
//!                ──POST /mosaic/v1/strata-anchor-batch {payload_cbor, ref_ids}──▶ Mosaic
//!                ◀── {txid, address, policy_id} ──  (dựng tx + ký + submit ở Mosaic)
//! ```
//!
//! Gọi HTTP sang một dịch vụ **không phải** là dựng tx: crate này không đụng
//! Cardano, không giữ khoá ví, không biết `policyId` được suy ra sao. Đó đúng là
//! điều luật `#1` đòi.
//!
//! ⚠️ `payload_cbor` **không đục hoàn toàn**: `ref_ids` phải đi kèm ở mức cấu trúc
//! vì beacon dựng `unit = policyId ‖ ref_id`. Cửa Mosaic đối chiếu lại danh sách
//! này với chính payload và **từ chối** nếu lệch — nên một lỗi ở đây hỏng to
//! tiếng, không âm thầm mint beacon trỏ vào anchor không có trong lô.

use lampnet_strata::anchor_sink::AnchorError;
use lampnet_strata::settlement::{SettlementRecord, SubmitOutcome, Submitter, encode_records};
use std::time::Duration;

/// Env: URL gốc của cửa Mosaic (VD `http://127.0.0.1:6691`).
pub const DOOR_URL_ENV: &str = "MOSAIC_DOOR_URL";
/// Env: bearer token của cửa. Cửa từ chối 401 nếu thiếu/sai.
pub const DOOR_TOKEN_ENV: &str = "MOSAIC_DOOR_TOKEN";
/// Env: mạng đích gửi kèm request (`preprod` | `preview` | `mainnet`). Vắng ⇒ để
/// cửa dùng mặc định của nó.
pub const DOOR_NETWORK_ENV: &str = "MOSAIC_DOOR_NETWORK";

/// Đường dẫn cửa — ghim ở một chỗ, không rải literal.
const DOOR_PATH: &str = "/mosaic/v1/strata-anchor-batch";

/// Submitter đẩy lô sang cửa Mosaic.
pub struct MosaicDoorSubmitter {
    base_url: String,
    token: String,
    label: u64,
    beacon: bool,
    network: Option<String>,
    timeout_secs: u64,
    client: reqwest::blocking::Client,
}

impl std::fmt::Debug for MosaicDoorSubmitter {
    /// Token KHÔNG bao giờ in ra — nó là khoá mở ví submit của cả hệ.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MosaicDoorSubmitter")
            .field("base_url", &self.base_url)
            .field("label", &self.label)
            .field("beacon", &self.beacon)
            .field("network", &self.network)
            .field("token", &"<REDACTED>")
            .finish()
    }
}

impl MosaicDoorSubmitter {
    /// Dựng từ tham số tường minh.
    pub fn new(base_url: String, token: String, label: u64, beacon: bool) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            label,
            beacon,
            network: None,
            timeout_secs: 120,
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("client build"),
        }
    }

    /// Ghim mạng gửi kèm request.
    pub fn with_network(mut self, network: Option<String>) -> Self {
        self.network = network;
        self
    }

    /// Dựng từ ENV của tiến trình.
    pub fn from_env(label: u64, beacon: bool) -> Result<Option<Self>, String> {
        Self::from_env_with(&|k| std::env::var(k).ok(), label, beacon)
    }

    /// Như [`from_env`](Self::from_env) nhưng **nhận nguồn env qua tham số**.
    ///
    /// Tách ra vì bản chỉ-đọc-`std::env` không kiểm được: người gọi (daemon) tự
    /// nhận env qua hàm cấp để test không đụng biến toàn tiến trình, rồi lại gọi
    /// một hàm bỏ qua nguồn đó — cấu hình trong test và cấu hình lúc chạy thật là
    /// hai thứ khác nhau, mà bộ kiểm vẫn xanh. Chính bộ kiểm của `sink_config` đã
    /// bắt được chỗ này.
    ///
    /// `Ok(None)` = **chưa** cắm cửa (không có URL), khác hẳn "cấu hình sai".
    /// Thiếu token trong khi ĐÃ có URL là **lỗi**: cửa sẽ trả 401 ở mọi lượt neo,
    /// và biết lúc khởi động rẻ hơn biết lúc lô đầu tiên rớt.
    pub fn from_env_with(
        get: &dyn Fn(&str) -> Option<String>,
        label: u64,
        beacon: bool,
    ) -> Result<Option<Self>, String> {
        let Some(url) = get(DOOR_URL_ENV).filter(|u| !u.trim().is_empty()) else {
            return Ok(None);
        };
        let token = get(DOOR_TOKEN_ENV)
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| {
                format!("có {DOOR_URL_ENV} nhưng thiếu {DOOR_TOKEN_ENV}: cửa Mosaic sẽ trả 401")
            })?;
        let network = get(DOOR_NETWORK_ENV).filter(|n| !n.trim().is_empty());
        Ok(Some(
            Self::new(
                url.trim().to_string(),
                token.trim().to_string(),
                label,
                beacon,
            )
            .with_network(network),
        ))
    }

    /// `ref_id` hex của các anchor trong lô, **đúng thứ tự** — cửa đối chiếu theo
    /// thứ tự chứ không theo tập.
    fn ref_ids(records: &[SettlementRecord]) -> Vec<String> {
        records
            .iter()
            .filter_map(|r| match r {
                SettlementRecord::Anchor(a) => Some(hex::encode(a.ref_id)),
                SettlementRecord::KeyRotation(_) => None,
            })
            .collect()
    }
}

impl Submitter for MosaicDoorSubmitter {
    fn submit(&self, records: &[SettlementRecord]) -> Result<SubmitOutcome, AnchorError> {
        // Strata giữ ĐÚNG MỘT encoder. Cửa chỉ chở byte, không dựng lại.
        let payload = encode_records(records);
        let mut body = serde_json::json!({
            "label": self.label,
            "payload_cbor": hex::encode(&payload),
            "ref_ids": Self::ref_ids(records),
            "beacon": self.beacon,
        });
        if let Some(n) = &self.network {
            body["network"] = serde_json::Value::String(n.clone());
        }

        let resp = self
            .client
            .post(format!("{}{DOOR_PATH}", self.base_url))
            .bearer_auth(&self.token)
            .timeout(Duration::from_secs(self.timeout_secs))
            .json(&body)
            .send()
            .map_err(|e| {
                // Cửa không với tới được = lô CHƯA được kiểm-và-đẩy ⇒ retryable.
                // Giữ lô lại là hành vi đúng: submit một lô chưa qua cửa thì không
                // còn ai dựng tx cho nó cả.
                AnchorError::Network(format!("gọi cửa Mosaic: {e}"))
            })?;

        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| {
            AnchorError::Rejected(format!(
                "cửa Mosaic trả body không phải JSON (HTTP {status}): {e} — {}",
                truncate(&text, 300)
            ))
        })?;

        if status.is_success() && v.get("ok").and_then(|b| b.as_bool()) == Some(true) {
            let txid = v
                .get("txid")
                .and_then(|t| t.as_str())
                .ok_or_else(|| AnchorError::Rejected("cửa Mosaic thiếu txid".into()))?
                .to_string();
            // `address` là thứ `publish_batch` đối chiếu với publisher đã pin.
            // Thiếu nó thì gác pin không chạy được ⇒ coi là lỗi, không mặc định rỗng.
            let address = v
                .get("address")
                .and_then(|a| a.as_str())
                .filter(|a| !a.is_empty())
                .ok_or_else(|| {
                    AnchorError::Rejected(
                        "cửa Mosaic thiếu `address` — không đối chiếu được publisher pin".into(),
                    )
                })?
                .to_string();
            return Ok(SubmitOutcome { txid, address });
        }

        let msg = v
            .get("error")
            .and_then(|m| m.as_str())
            .unwrap_or("cửa Mosaic lỗi không rõ")
            .to_string();
        // Phân tầng retry theo `error_kind` do cửa khai — TẤT ĐỊNH thì KHÔNG thử lại.
        Err(match v.get("error_kind").and_then(|k| k.as_str()) {
            Some("NotConfigured") | Some("Unauthorized") => AnchorError::NotConfigured,
            Some("Network") => AnchorError::Network(msg),
            Some("InsufficientAda") => AnchorError::InsufficientAda { need: 0, have: 0 },
            Some("DatumTooLarge") => AnchorError::DatumTooLarge { bytes: 0 },
            _ => AnchorError::Rejected(format!("HTTP {status}: {msg}")),
        })
    }
}

fn truncate(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lampnet_strata::chain::StrataAnchor;

    fn anchor(seq: u64, ref_id: u8) -> SettlementRecord {
        SettlementRecord::Anchor(StrataAnchor {
            ref_id: [ref_id; 32],
            head_version_hash: [0x22; 32],
            mmr_root: [0x33; 32],
            seq,
        })
    }

    #[test]
    fn token_khong_lo_trong_debug() {
        let s = MosaicDoorSubmitter::new("http://x".into(), "sieu-bi-mat".into(), 1234, false);
        let d = format!("{s:?}");
        assert!(d.contains("<REDACTED>"));
        assert!(!d.contains("sieu-bi-mat"));
    }

    #[test]
    fn ref_ids_dung_thu_tu_va_bo_qua_key_rotation() {
        let recs = vec![
            anchor(1, 0xaa),
            SettlementRecord::KeyRotation(vec![0u8; 10]),
            anchor(2, 0xbb),
        ];
        assert_eq!(
            MosaicDoorSubmitter::ref_ids(&recs),
            vec!["aa".repeat(32), "bb".repeat(32)]
        );
    }

    #[test]
    fn url_bo_dau_gach_cheo_thua() {
        let s = MosaicDoorSubmitter::new("http://x:6691/".into(), "t".into(), 1234, false);
        assert_eq!(s.base_url, "http://x:6691");
    }

    #[test]
    fn cua_khong_voi_toi_duoc_la_loi_retryable() {
        // Cổng đóng ⇒ Network (được thử lại), KHÔNG phải Rejected (fail cứng).
        // Phân tầng sai ở đây làm một sự cố mạng thoáng qua giết luôn lượt neo.
        let s = MosaicDoorSubmitter::new("http://127.0.0.1:1".into(), "t".into(), 1234, false);
        let err = s.submit(&[anchor(0, 0x11)]).unwrap_err();
        assert!(
            matches!(err, AnchorError::Network(_)),
            "phải là Network, gặp {err:?}"
        );
        assert!(err.is_retryable());
    }
}
