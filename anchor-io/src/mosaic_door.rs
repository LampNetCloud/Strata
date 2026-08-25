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
/// Env mạng của cửa — **giữ tên để tương thích, KHÔNG còn được đọc ở đây**.
///
/// 🔺 Mạng đi vào **thông điệp được ký** ([`operator_sig_message`]), nên nó phải có
/// **đúng một** nguồn. Bên gọi (`sink_config`) đã phân giải mạng từ
/// `STRATA_ANCHOR_NETWORK` để chọn endpoint Blockfrost; đọc thêm một env thứ hai ở đây
/// là dựng **hai định nghĩa cho cùng một vị ngữ** — và hai bản ấy lệch nhau vào ngày
/// không ai nhìn, với triệu chứng là `401` không nói lý do.
///
/// ⇒ Mạng nay là **tham số** của [`from_env_with`](MosaicDoorSubmitter::from_env_with).
pub const DOOR_NETWORK_ENV: &str = "MOSAIC_DOOR_NETWORK";

/// Env: khoá bí mật ed25519 (**hex 64 ký tự = 32 byte seed**) ký lô gửi vào cửa.
///
/// Cửa Mosaic giữ **allow-list khoá công khai** (`MOSAIC_DOOR_OPERATOR_KEYS`) và từ chối
/// `401` mọi lô không kèm chữ ký hợp lệ. Token gác **cái ống**; chữ ký này gác **nội
/// dung lô** — token là bí mật dùng chung, nên nó không nói ai đã soạn lô.
pub const DOOR_OPERATOR_SK_ENV: &str = "MOSAIC_DOOR_OPERATOR_SK";

/// Domain-tag của thông điệp ký — **phải khớp từng byte** với
/// `VeDataIO/Core: mosaic/l1/src/door.rs::OPERATOR_SIG_DOMAIN`.
const OPERATOR_SIG_DOMAIN: &[u8] = b"MOSAIC-STRATA-BATCH-v1";

/// Thông điệp 32 byte mà operator ký cho một lô.
///
/// ```text
/// blake2b-256( DOMAIN ‖ u8(len(network)) ‖ network ‖ u64be(label)
///              ‖ u8(beacon) ‖ u64be(len(payload)) ‖ payload )
/// ```
///
/// 🔺 **Đây là bản SAO của một định nghĩa sống ở kho khác** (`door.rs`), và đó là chỗ
/// nguy hiểm: hai bản của cùng một vị ngữ. Lệch một byte thì cửa trả `401` với **đúng
/// một thông điệp chung** — nó cố tình không phân biệt "khoá lạ" với "chữ ký sai" (nếu
/// phân biệt thì cửa thành máy dò allow-list). Nghĩa là **không có gì nói cho ta biết
/// chỗ lệch nằm ở đâu**. Vì thế có một bài kiểm ghim đúng vector byte của cửa.
pub fn operator_sig_message(network: &str, label: u64, beacon: bool, payload: &[u8]) -> [u8; 32] {
    use blake2::digest::{Update, VariableOutput};
    let mut h = blake2::Blake2bVar::new(32).expect("32 byte hợp lệ cho blake2b");
    let net = network.as_bytes();
    h.update(OPERATOR_SIG_DOMAIN);
    h.update(&[net.len() as u8]);
    h.update(net);
    h.update(&label.to_be_bytes());
    h.update(&[u8::from(beacon)]);
    h.update(&(payload.len() as u64).to_be_bytes());
    h.update(payload);
    let mut out = [0u8; 32];
    h.finalize_variable(&mut out).expect("độ dài khớp");
    out
}

/// Đường dẫn cửa — ghim ở một chỗ, không rải literal.
const DOOR_PATH: &str = "/mosaic/v1/strata-anchor-batch";

/// Submitter đẩy lô sang cửa Mosaic.
pub struct MosaicDoorSubmitter {
    base_url: String,
    token: String,
    label: u64,
    beacon: bool,
    network: Option<String>,
    /// Khoá ký lô. `None` ⇒ không ký ⇒ cửa trả `401`. Chỉ `None` ở đường dựng tường
    /// minh (test); đường env **bắt buộc** có.
    operator_sk: Option<ed25519_dalek::SigningKey>,
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
            .field(
                "operator_sk",
                &self.operator_sk.as_ref().map(|_| "<REDACTED>"),
            )
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
            operator_sk: None,
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

    /// Ghim khoá ký lô (32 byte seed ed25519).
    pub fn with_operator_sk(mut self, sk: [u8; 32]) -> Self {
        self.operator_sk = Some(ed25519_dalek::SigningKey::from_bytes(&sk));
        self
    }

    /// Khoá công khai của operator đang ký — hex 64 ký tự, đúng dạng cửa nhận.
    ///
    /// Công khai được: nó **là** thứ nằm trong allow-list của cửa. In nó lúc khởi động
    /// cho người vận hành đối chiếu với `MOSAIC_DOOR_OPERATOR_KEYS` **trước** lượt neo
    /// đầu tiên — rẻ hơn nhiều so với đọc một `401` không nói lý do.
    pub fn operator_vkey_hex(&self) -> Option<String> {
        self.operator_sk
            .as_ref()
            .map(|sk| hex::encode(sk.verifying_key().to_bytes()))
    }

    /// Dựng từ ENV của tiến trình.
    pub fn from_env(label: u64, beacon: bool, network: &str) -> Result<Option<Self>, String> {
        Self::from_env_with(&|k| std::env::var(k).ok(), label, beacon, network)
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
        network: &str,
    ) -> Result<Option<Self>, String> {
        let Some(url) = get(DOOR_URL_ENV).filter(|u| !u.trim().is_empty()) else {
            return Ok(None);
        };
        let token = get(DOOR_TOKEN_ENV)
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| {
                format!("có {DOOR_URL_ENV} nhưng thiếu {DOOR_TOKEN_ENV}: cửa Mosaic sẽ trả 401")
            })?;
        let network = network.trim();
        if network.is_empty() {
            return Err(format!(
                "mạng rỗng: nó nằm TRONG thông điệp operator ký, nên gửi một lô mà không \
                 biết mình đang ký cho mạng nào là không làm được (xem {DOOR_URL_ENV})"
            ));
        }

        let sk_hex = get(DOOR_OPERATOR_SK_ENV)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                format!(
                    "có {DOOR_URL_ENV} nhưng thiếu {DOOR_OPERATOR_SK_ENV}: cửa Mosaic từ chối \
                     401 mọi lô KHÔNG kèm chữ ký operator. Token gác cái ống, chữ ký gác nội \
                     dung lô — có token mà không có khoá ký thì không neo được lô nào"
                )
            })?;
        let sk: [u8; 32] = hex::decode(&sk_hex)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| {
                format!("{DOOR_OPERATOR_SK_ENV} phải là hex 64 ký tự (32 byte seed ed25519)")
            })?;

        Ok(Some(
            Self::new(
                url.trim().to_string(),
                token.trim().to_string(),
                label,
                beacon,
            )
            .with_network(Some(network.to_string()))
            .with_operator_sk(sk),
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

        // Chữ ký operator trên CHÍNH lô này. Ký ở đây chứ không ở tầng trên vì thông
        // điệp phủ `payload` — mà `payload` chỉ tồn tại sau `encode_records`, và Strata
        // giữ ĐÚNG MỘT encoder. Ký một byte khác byte gửi đi là ký một lô khác.
        if let Some(sk) = &self.operator_sk {
            use ed25519_dalek::Signer;
            // Mạng phải là mạng GỬI ĐI. Ký theo một tên mạng rồi gửi tên khác (hoặc
            // không gửi) là tự tạo ra hai lô khác nhau cho cùng một lượt.
            let net = self.network.as_deref().ok_or_else(|| {
                AnchorError::Rejected(format!(
                    "có khoá ký nhưng thiếu mạng: mạng nằm trong thông điệp ký, xem \
                     {DOOR_NETWORK_ENV}"
                ))
            })?;
            let msg = operator_sig_message(net, self.label, self.beacon, &payload);
            body["operator_vkey"] =
                serde_json::Value::String(hex::encode(sk.verifying_key().to_bytes()));
            body["operator_sig"] = serde_json::Value::String(hex::encode(sk.sign(&msg).to_bytes()));
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
            // 🪤 `Unauthorized` từng gộp chung với `NotConfigured`, và chính chỗ gộp đó
            // làm một cửa 401 hiện ra ở đầu Strata là "daemon chưa cắm AnchorSink" —
            // câu chỉ người vận hành đi kiểm cấu hình sink, ĐÚNG chỗ không có lỗi.
            // Hai thứ khác hẳn nhau: `NotConfigured` là *ta chưa cắm gì*; `Unauthorized`
            // là *ta đã cắm, gọi tới nơi, và bị TỪ CHỐI*. Giữ nguyên văn lời cửa nói.
            Some("Unauthorized") => AnchorError::Rejected(format!(
                "cửa Mosaic TỪ CHỐI lô (401): {msg} — kiểm {DOOR_TOKEN_ENV} và \
                 {DOOR_OPERATOR_SK_ENV} (khoá công khai của nó phải nằm trong \
                 `MOSAIC_DOOR_OPERATOR_KEYS` của cửa)"
            )),
            Some("NotConfigured") => AnchorError::NotConfigured,
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

#[cfg(test)]
mod operator_sig_tests {
    use super::*;

    /// 🔺 **Vector ĐỐI CHỨNG sinh từ chính cửa**, không phải từ bản cài ở đây.
    ///
    /// `operator_sig_message` là **bản sao của một định nghĩa sống ở kho khác**
    /// (`VeDataIO/Core: mosaic/l1/src/door.rs`). Một bài kiểm chỉ so bản này với chính
    /// nó sẽ xanh vĩnh viễn kể cả khi hai bên đã lệch — đó là **xanh giả**, và triệu
    /// chứng ngoài đời của nó là `401` mà cửa **cố ý không nói lý do** (phân biệt "khoá
    /// lạ" với "chữ ký sai" biến cửa thành máy dò allow-list).
    ///
    /// Năm vector dưới đây in ra từ `door::operator_sig_message` của Core ngày
    /// 2026-08-25. Đổi một byte ở bất kỳ bên nào ⇒ bài này ĐỎ, thay vì một lô rớt im
    /// lặng ở lượt neo đầu tiên.
    const VECTORS: &[(&str, u64, bool, &[u8], &str)] = &[
        (
            "preprod",
            1234,
            false,
            &[0xa1, 0x01, 0x02],
            "4221b1a2864783b8e80f5ef498736404b8e6271dcd2d8eb99171d85cd8c076f7",
        ),
        (
            "preprod",
            1234,
            true,
            &[0xa1, 0x01, 0x02],
            "4b1bb93e517d7fb7d7c6c52dfbe31af38c28f3addd11db04ab633a7ef9015921",
        ),
        (
            "preview",
            7368,
            false,
            &[0xa1, 0x01, 0x02],
            "af684dcbd68f5d392aa836f9e9851cc5ad060c4a5b4d6067aa2095230acc9822",
        ),
        (
            "mainnet",
            1234,
            false,
            &[0xa1, 0x01, 0x02],
            "e732350dc061e7bef611547bb965688a16366fc2eff6473d6b273c885c7a1dad",
        ),
        (
            "preprod",
            1234,
            false,
            &[],
            "99c293190d1c72dfa283e99500e56b2ffe1fd68b88d9af5d2e32f00915115ae2",
        ),
    ];

    #[test]
    fn thong_diep_ky_khop_tung_byte_voi_cua_mosaic() {
        for (net, label, beacon, payload, want) in VECTORS {
            let got = hex::encode(operator_sig_message(net, *label, *beacon, payload));
            assert_eq!(
                &got, want,
                "lệch vector cửa tại (net={net}, label={label}, beacon={beacon}) — \
                 lệch ở đây nghĩa là mọi lô sẽ ăn 401 mà cửa KHÔNG nói vì sao"
            );
        }
    }

    /// Đối chứng âm: bốn đại lượng đi vào thông điệp phải **thật sự** đổi nó.
    ///
    /// Thiếu bài này thì một bản cài bỏ quên `beacon` (hoặc `label`, hoặc `network`)
    /// vẫn qua được bài trên với 5/5 vector — miễn là nó tình cờ khớp ở đúng những bộ
    /// tham số đã ghim.
    #[test]
    fn moi_dai_luong_deu_doi_thong_diep() {
        let base = operator_sig_message("preprod", 1234, false, &[0xa1]);
        assert_ne!(
            base,
            operator_sig_message("preview", 1234, false, &[0xa1]),
            "network"
        );
        assert_ne!(
            base,
            operator_sig_message("preprod", 1235, false, &[0xa1]),
            "label"
        );
        assert_ne!(
            base,
            operator_sig_message("preprod", 1234, true, &[0xa1]),
            "beacon"
        );
        assert_ne!(
            base,
            operator_sig_message("preprod", 1234, false, &[0xa2]),
            "payload"
        );
    }

    /// Length-prefix của `network` và `payload` phải chặn **nhập nhằng biên**.
    ///
    /// Không có len-prefix thì `net="ab" ‖ payload="c"` và `net="a" ‖ payload="bc"` cho
    /// cùng một thông điệp — và cả hai đều do bên gửi chọn.
    #[test]
    fn bien_giua_network_va_payload_khong_nhap_nhang() {
        assert_ne!(
            operator_sig_message("ab", 1, false, b"c"),
            operator_sig_message("a", 1, false, b"bc"),
        );
    }

    fn env_of(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    const SK_HEX: &str = "0101010101010101010101010101010101010101010101010101010101010101";

    fn build(pairs: &[(&str, &str)]) -> Result<Option<MosaicDoorSubmitter>, String> {
        let m = env_of(pairs);
        MosaicDoorSubmitter::from_env_with(&|k| m.get(k).cloned(), 1234, false, "preprod")
    }

    #[test]
    fn co_url_ma_thieu_khoa_ky_la_loi_khoi_dong() {
        let err = build(&[
            (DOOR_URL_ENV, "http://127.0.0.1:6691"),
            (DOOR_TOKEN_ENV, "t"),
        ])
        .expect_err("thiếu khoá ký PHẢI chặn khởi động, không để hỏng ở lượt neo đầu");
        assert!(err.contains(DOOR_OPERATOR_SK_ENV), "{err}");
    }

    #[test]
    fn khoa_ky_sai_dinh_dang_bi_tu_choi() {
        let err = build(&[
            (DOOR_URL_ENV, "http://127.0.0.1:6691"),
            (DOOR_TOKEN_ENV, "t"),
            (DOOR_OPERATOR_SK_ENV, "khong-phai-hex"),
        ])
        .expect_err("seed sai định dạng phải bị từ chối");
        assert!(err.contains("32 byte"), "{err}");
    }

    #[test]
    fn khong_co_url_van_la_chua_cam_cua_chu_khong_phai_loi() {
        assert!(
            build(&[(DOOR_TOKEN_ENV, "t")])
                .expect("không URL = chưa cắm")
                .is_none()
        );
    }

    #[test]
    fn dung_du_env_thi_dung_va_lo_vkey_de_doi_chieu_allow_list() {
        let s = build(&[
            (DOOR_URL_ENV, "http://127.0.0.1:6691"),
            (DOOR_TOKEN_ENV, "t"),
            (DOOR_OPERATOR_SK_ENV, SK_HEX),
        ])
        .expect("đủ env")
        .expect("có URL");
        let vkey = s.operator_vkey_hex().expect("có khoá ⇒ có vkey");
        assert_eq!(vkey.len(), 64, "vkey hex 64 ký tự — đúng dạng cửa nhận");
        assert!(
            !format!("{s:?}").contains(SK_HEX),
            "Debug KHÔNG được để lộ khoá bí mật"
        );
    }
}
