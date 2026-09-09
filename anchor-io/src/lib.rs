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
        let Some(items) = v.as_array() else {
            return Ok(None);
        };
        for it in items {
            if it.get("label").and_then(|l| l.as_str()) != Some(&label.to_string()) {
                continue;
            }
            // Blockfrost: field "metadata" (hex) hoặc "cbor_metadata" ("\x" + hex).
            let hex_str = it
                .get("metadata")
                .and_then(|m| m.as_str())
                .or_else(|| it.get("cbor_metadata").and_then(|m| m.as_str()));
            if let Some(h) = hex_str {
                let h = h.strip_prefix("\\x").unwrap_or(h);
                if let Ok(bytes) = hex::decode(h) {
                    return Ok(Some(bytes));
                }
            }
        }
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
}
