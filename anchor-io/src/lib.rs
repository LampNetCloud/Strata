//! `lampnet-anchor-io` — cài đặt I/O cho S1 AnchorSink backend **Settlement**
//! ([`lampnet_strata::settlement`]). Tách khỏi crate lõi `lampnet-strata` để lõi giữ
//! **no-I/O** (không kéo `reqwest`/process):
//!
//! - [`BlockfrostQuery`] — đọc on-chain Preview qua Blockfrost, impl
//!   [`ChainQuery`](lampnet_strata::settlement::ChainQuery). `resolve` lọc theo địa chỉ
//!   INPUT của tx (chỉ tin tx do publisher CHI — chống đầu độc indexer).
//! - [`MosaicDoorSubmitter`] — **đường hiện hành**: đẩy lô sang cửa Mosaic
//!   (`POST /mosaic/v1/strata-anchor-batch`), Mosaic dựng tx + ký + submit. impl
//!   [`Submitter`](lampnet_strata::settlement::Submitter).
//!
//! # `TsSubmitter` + `submitter/submit.ts` — ĐÃ XOÁ 2026-08-15
//!
//! Chúng từng dựng tx **ngay trong kho này** (Lucid Evolution qua child-process),
//! tức một chỗ **đã vượt** luật `#1`: *"Strata giữ logic chain; Mosaic giữ tx;
//! KHÔNG dựng tx neo trong Strata"*. Luật chuyển giao đặt hai điều kiện, cả hai
//! nay đã đạt:
//!
//! - **(a)** bản Mosaic qua đúng bộ fixture chung `apis/settlement-metadata.json`
//!   — 8 ca dương + 6 ca âm + 1 ca bỏ-qua (`Core: mosaic/l1/tests/settlement_fixture.rs`);
//! - **(b)** submit được tx **thật**: Preprod `d9975f60…` (3 anchor),
//!   `7e78cfaa…` (10 anchor), và `resolve()` đọc lại được **3/3**.
//!
//! Đủ cả hai ⇒ **XOÁ, không giữ song song**: hai đường submit là hai chỗ cầm khoá
//! ví, tức nhân đôi đúng thứ đang muốn gom về một nhà (`VeDataIO/Core#87`).
//!
//! **Bí mật (token cửa, project-id) chỉ đi qua ENV**, KHÔNG qua argv, KHÔNG in ra
//! log/error — mọi type cầm secret đều redact trong `Debug`.

pub mod mosaic_door;
pub use mosaic_door::MosaicDoorSubmitter;

use std::time::Duration;

use lampnet_strata::anchor_sink::AnchorError;
use lampnet_strata::settlement::ChainQuery;

// ───────────────────────────────────────────────────────────────────────────
// BlockfrostQuery (Preview) — reqwest blocking
// ───────────────────────────────────────────────────────────────────────────

/// Query Blockfrost Preview. `project_id` KHÔNG bao giờ in ra (Debug redact).
pub struct BlockfrostQuery {
    base: String,
    project_id: String,
    client: reqwest::blocking::Client,
}

impl std::fmt::Debug for BlockfrostQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockfrostQuery")
            .field("base", &self.base)
            .field("project_id", &"<REDACTED>")
            .finish()
    }
}

impl BlockfrostQuery {
    /// Preview mặc định. `project_id` từ env `BLOCKFROST_TOKEN_GREENSUN` (caller nạp).
    pub fn preview(project_id: String) -> Self {
        Self::new(
            "https://cardano-preview.blockfrost.io/api/v0".into(),
            project_id,
        )
    }

    /// Base URL tuỳ ý (test/local).
    pub fn new(base: String, project_id: String) -> Self {
        Self {
            base,
            project_id,
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("client build"),
        }
    }

    /// GET → (status, body). Lỗi transport → `Network`.
    fn get(&self, path: &str) -> Result<(u16, String), AnchorError> {
        let resp = self
            .client
            .get(format!("{}{}", self.base, path))
            .header("project_id", &self.project_id)
            .send()
            .map_err(net_err)?;
        let status = resp.status().as_u16();
        let body = resp.text().map_err(net_err)?;
        Ok((status, body))
    }

    fn get_json(&self, path: &str) -> Result<Option<serde_json::Value>, AnchorError> {
        let (status, body) = self.get(path)?;
        match status {
            200 => serde_json::from_str(&body)
                .map(Some)
                .map_err(|e| AnchorError::Rejected(format!("Blockfrost JSON hỏng: {e}"))),
            404 => Ok(None),
            429 | 500..=599 => Err(AnchorError::Network(format!("Blockfrost HTTP {status}"))),
            _ => Err(AnchorError::Rejected(format!(
                "Blockfrost HTTP {status}: {}",
                truncate(&body, 300)
            ))),
        }
    }

    /// Số dư lovelace của địa chỉ (0 nếu địa chỉ chưa từng dùng).
    pub fn lovelace_balance(&self, addr: &str) -> Result<u64, AnchorError> {
        let Some(v) = self.get_json(&format!("/addresses/{addr}"))? else {
            return Ok(0);
        };
        let mut total = 0u64;
        if let Some(amounts) = v.get("amount").and_then(|a| a.as_array()) {
            for it in amounts {
                if it.get("unit").and_then(|u| u.as_str()) == Some("lovelace") {
                    total += it
                        .get("quantity")
                        .and_then(|q| q.as_str())
                        .and_then(|q| q.parse::<u64>().ok())
                        .unwrap_or(0);
                }
            }
        }
        Ok(total)
    }

    /// Tx đã confirm chưa (Blockfrost thấy tx). Trả `Some(fee_lovelace)` nếu rồi.
    pub fn tx_fee_if_confirmed(&self, txid: &str) -> Result<Option<u64>, AnchorError> {
        let Some(v) = self.get_json(&format!("/txs/{txid}"))? else {
            return Ok(None);
        };
        Ok(v.get("fees")
            .and_then(|f| f.as_str())
            .and_then(|f| f.parse::<u64>().ok()))
    }
}

fn net_err(e: reqwest::Error) -> AnchorError {
    // reqwest::Error KHÔNG chứa header (project_id an toàn); chỉ chứa URL + kind.
    AnchorError::Network(e.to_string())
}

fn truncate(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

/// `label` trong đáp ứng Blockfrost khớp `want` hay không — **nhận cả hai hình dạng**.
///
/// Tài liệu Blockfrost tả `label` là chuỗi, và mã cũ chỉ so `as_str()`. Nhưng nhãn
/// metadata là một **số nguyên** trong định dạng giao dịch Cardano, nên một đáp ứng
/// trả `1234` (số JSON) thay vì `"1234"` là hợp lệ với chính khái niệm ấy — và khi đó
/// `as_str()` trả `None`, không mục nào khớp, hàm trả `Ok(None)`, rồi `resolve` đọc
/// thành *"ref chưa neo"*. Sai lặng, trên đường gác chống tụt lùi.
///
/// Nhận cả hai gỡ luôn nhu cầu đi đo Blockfrost thật sự trả hình dạng nào: câu hỏi
/// ấy chỉ tồn tại vì mã chỉ chấp nhận một hình dạng. Chuỗi được **so với `want` dạng
/// thập phân** chứ không parse tự do, để `"01234"` hay `" 1234"` không lọt.
fn label_matches(v: Option<&serde_json::Value>, want: u64) -> bool {
    match v {
        Some(serde_json::Value::String(s)) => s == &want.to_string(),
        Some(serde_json::Value::Number(n)) => n.as_u64() == Some(want),
        _ => false,
    }
}

impl ChainQuery for BlockfrostQuery {
    fn address_txs(&self, addr: &str, limit: usize) -> Result<Vec<String>, AnchorError> {
        let mut out = Vec::new();
        let mut page = 1usize;
        while out.len() < limit {
            let count = 100.min(limit - out.len());
            let Some(v) = self.get_json(&format!(
                "/addresses/{addr}/transactions?order=desc&count={count}&page={page}"
            ))?
            else {
                break; // 404 = địa chỉ chưa từng dùng
            };
            let Some(items) = v.as_array() else { break };
            if items.is_empty() {
                break;
            }
            for it in items {
                if let Some(h) = it.get("tx_hash").and_then(|h| h.as_str()) {
                    out.push(h.to_string());
                }
            }
            if items.len() < count {
                break;
            }
            page += 1;
        }
        Ok(out)
    }

    fn tx_input_addresses(&self, txid: &str) -> Result<Vec<String>, AnchorError> {
        let Some(v) = self.get_json(&format!("/txs/{txid}/utxos"))? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        if let Some(inputs) = v.get("inputs").and_then(|i| i.as_array()) {
            for inp in inputs {
                // Bỏ collateral input (không phải "chi tiêu" thật của tx thành công).
                if inp.get("collateral").and_then(|c| c.as_bool()) == Some(true) {
                    continue;
                }
                if let Some(a) = inp.get("address").and_then(|a| a.as_str()) {
                    out.push(a.to_string());
                }
            }
        }
        Ok(out)
    }

    /// `/txs/{hash}` → `slot`. 404 = tx chưa lên chuỗi ⇒ `None`.
    ///
    /// Một lượt gọi riêng cho mỗi tx: `/addresses/{addr}/transactions` trả
    /// `block_height` + `block_time` nhưng **không** trả slot, và cửa sổ checkpoint
    /// được định nghĩa theo **slot** (đơn vị mà datum on-chain mang). Quy đổi
    /// height/time sang slot ở tầng này là đưa một phép ước vào chỗ đang cần một
    /// phép so chính xác.
    fn tx_slot(&self, txid: &str) -> Result<Option<u64>, AnchorError> {
        let Some(v) = self.get_json(&format!("/txs/{txid}"))? else {
            return Ok(None);
        };
        Ok(v.get("slot").and_then(|s| s.as_u64()))
    }

    /// `/blocks/latest` → `slot`.
    fn tip_slot(&self) -> Result<u64, AnchorError> {
        let Some(v) = self.get_json("/blocks/latest")? else {
            return Err(AnchorError::Network(
                "/blocks/latest trả 404 — indexer chưa sẵn sàng".into(),
            ));
        };
        v.get("slot")
            .and_then(|s| s.as_u64())
            .ok_or_else(|| AnchorError::Network("/blocks/latest thiếu trường `slot`".into()))
    }

    fn asset_latest_tx(&self, unit: &str) -> Result<Option<String>, AnchorError> {
        // /assets/{unit}/transactions?order=desc → tx đụng asset, MỚI→CŨ. Phần tử đầu =
        // lần di chuyển beacon gần nhất. 404 = asset chưa từng tồn tại → chưa neo.
        let Some(v) = self.get_json(&format!(
            "/assets/{unit}/transactions?order=desc&count=1&page=1"
        ))?
        else {
            return Ok(None);
        };
        let first = v.as_array().and_then(|items| items.first());
        Ok(first
            .and_then(|it| it.get("tx_hash"))
            .and_then(|h| h.as_str())
            .map(|s| s.to_string()))
    }

    fn tx_metadata_cbor(&self, txid: &str, label: u64) -> Result<Option<Vec<u8>>, AnchorError> {
        let Some(v) = self.get_json(&format!("/txs/{txid}/metadata/cbor"))? else {
            return Ok(None);
        };
        // 200 mà thân không phải mảng = Blockfrost đổi hình dạng, tức KHÔNG ĐỌC ĐƯỢC.
        // Trả `Ok(None)` ở đây là nói "tx này không có metadata", và người gọi
        // (`resolve`) đọc tiếp thành "ref chưa neo" — mở đúng cổng INV-E7 đóng.
        let Some(items) = v.as_array() else {
            return Err(AnchorError::Rejected(format!(
                "Blockfrost /txs/{txid}/metadata/cbor trả 200 nhưng thân không phải mảng — \
                 hình dạng đáp ứng đã đổi. Đây là 'không đọc được', KHÔNG phải 'tx không có \
                 metadata'."
            )));
        };
        for it in items {
            if !label_matches(it.get("label"), label) {
                continue;
            }
            // Blockfrost: field "metadata" (hex) hoặc "cbor_metadata" ("\x" + hex).
            let hex_str = it
                .get("metadata")
                .and_then(|m| m.as_str())
                .or_else(|| it.get("cbor_metadata").and_then(|m| m.as_str()));
            // Nhãn ĐÃ khớp ⇒ mục này là mục của mình. Không giải mã được thì đó là hỏng,
            // không phải "không có". `continue` ở đây làm vòng lặp chạy hết rồi rơi xuống
            // `Ok(None)` — cùng một giá trị với "tx không mang nhãn này".
            let Some(h) = hex_str else {
                return Err(AnchorError::Rejected(format!(
                    "tx {txid} có metadata nhãn {label} nhưng không có trường `metadata` lẫn \
                     `cbor_metadata` — không đọc được nội dung đã neo."
                )));
            };
            let h = h.strip_prefix("\\x").unwrap_or(h);
            return hex::decode(h).map(Some).map_err(|e| {
                AnchorError::Rejected(format!(
                    "tx {txid} nhãn {label}: hex của metadata hỏng ({e}) — không đọc được nội \
                     dung đã neo."
                ))
            });
        }
        // Tới đây là đã duyệt hết mục mà không mục nào mang nhãn này. Đây mới đúng nghĩa
        // "tx này không neo gì cho nhãn đang hỏi" — trạng thái CÓ THẬT, không phải trạng
        // thái mù.
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blockfrost_debug_redacts_token() {
        let q = BlockfrostQuery::preview("secret_token_abc123".into());
        let dbg = format!("{q:?}");
        assert!(dbg.contains("<REDACTED>"));
        assert!(!dbg.contains("secret_token_abc123"));
    }

    // ── #41 mục 6: nhãn metadata phải khớp ở CẢ HAI hình dạng JSON ────────────

    /// Hình dạng mã cũ đã nhận. Giữ ca này để việc nới rộng không đánh mất nó.
    #[test]
    fn label_matches_string_shape() {
        assert!(label_matches(Some(&serde_json::json!("1234")), 1234));
    }

    /// Hình dạng mã cũ KHÔNG nhận — và đó là chỗ hỏng: `as_str()` trả `None`, không
    /// mục nào khớp, `tx_metadata_cbor` trả `Ok(None)`, `resolve` đọc thành "chưa neo".
    #[test]
    fn label_matches_number_shape() {
        assert!(
            label_matches(Some(&serde_json::json!(1234)), 1234),
            "nhãn dạng SỐ phải khớp — mã cũ chỉ so as_str() nên trượt lặng"
        );
    }

    /// Nới rộng không được nới thành parse tự do: chuỗi phải khớp đúng dạng thập phân.
    #[test]
    fn label_does_not_match_loose_strings() {
        for s in ["01234", " 1234", "1234 ", "0x4d2", ""] {
            assert!(
                !label_matches(Some(&serde_json::json!(s)), 1234),
                "chuỗi {s:?} không được coi là nhãn 1234"
            );
        }
    }

    /// Nhãn khác, thiếu nhãn, hoặc nhãn kiểu lạ — đều là KHÔNG khớp, và đó là trạng
    /// thái có thật ("mục này không phải của mình"), khác hẳn trạng thái mù.
    #[test]
    fn label_mismatch_and_absent() {
        assert!(!label_matches(Some(&serde_json::json!(1235)), 1234));
        assert!(!label_matches(Some(&serde_json::json!("1235")), 1234));
        assert!(!label_matches(None, 1234));
        assert!(!label_matches(Some(&serde_json::json!(null)), 1234));
        assert!(!label_matches(Some(&serde_json::json!({"v": 1234})), 1234));
    }

    /// Số âm / số thực không phải nhãn hợp lệ — `as_u64()` trả `None`, không được
    /// rơi vào nhánh khớp.
    #[test]
    fn label_rejects_non_integer_numbers() {
        assert!(!label_matches(Some(&serde_json::json!(-1)), 1234));
        assert!(!label_matches(Some(&serde_json::json!(1234.5)), 1234));
    }
}
