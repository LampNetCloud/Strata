//! `lampnet-anchor-io` — cài đặt I/O cho S1 AnchorSink backend **Settlement**
//! ([`lampnet_strata::settlement`]). Tách khỏi crate lõi `lampnet-strata` để lõi giữ
//! **no-I/O** (không kéo `reqwest`/process):
//!
//! - [`BlockfrostQuery`] — đọc on-chain Preview qua Blockfrost, impl
//!   [`ChainQuery`](lampnet_strata::settlement::ChainQuery). `resolve` lọc theo địa chỉ
//!   INPUT của tx (chỉ tin tx do publisher CHI — chống đầu độc indexer).
//! - [`TsSubmitter`] — build+sign+submit tx metadata label 1234 qua child-process
//!   `submitter/submit.ts` (Lucid Evolution). impl
//!   [`Submitter`](lampnet_strata::settlement::Submitter).
//!
//! **Bí mật (mnemonic/token) chỉ đi qua ENV của process cha**, KHÔNG qua argv, KHÔNG in
//! ra log/error — mọi type cầm secret redact trong `Debug`; submit.ts tự lọc trước khi in.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use lampnet_strata::anchor_sink::AnchorError;
use lampnet_strata::settlement::{ChainQuery, SettlementRecord, SubmitOutcome, Submitter};

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

    fn asset_latest_tx(&self, unit: &str) -> Result<Option<String>, AnchorError> {
        // /assets/{unit}/transactions?order=desc → tx đụng asset, MỚI→CŨ. Phần tử đầu =
        // lần di chuyển beacon gần nhất. 404 = asset chưa từng tồn tại → chưa neo.
        let Some(v) =
            self.get_json(&format!("/assets/{unit}/transactions?order=desc&count=1&page=1"))?
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

// ───────────────────────────────────────────────────────────────────────────
// TsSubmitter — child process Node/tsx (Lucid Evolution + Blockfrost)
// ───────────────────────────────────────────────────────────────────────────

/// Submitter gọi `submitter/submit.ts` qua stdin/stdout JSON. Secret (mnemonic/token)
/// đi bằng ENV của process cha — KHÔNG qua argv, KHÔNG log.
pub struct TsSubmitter {
    /// Thư mục chứa `submit.ts` + `node_modules`.
    pub submitter_dir: PathBuf,
    /// Nhãn metadata.
    pub label: u64,
    /// Timeout chờ child (giây).
    pub timeout_secs: u64,
}

impl std::fmt::Debug for TsSubmitter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TsSubmitter")
            .field("submitter_dir", &self.submitter_dir)
            .field("label", &self.label)
            .finish()
    }
}

impl Submitter for TsSubmitter {
    fn submit(&self, records: &[SettlementRecord]) -> Result<SubmitOutcome, AnchorError> {
        // JS Number an toàn tới 2^53-1; seq vượt → từ chối trước khi sang TS.
        const JS_MAX_SAFE: u64 = (1u64 << 53) - 1;
        let mut recs = Vec::with_capacity(records.len());
        for r in records {
            match r {
                SettlementRecord::Anchor(a) => {
                    if a.seq > JS_MAX_SAFE {
                        return Err(AnchorError::Rejected(format!(
                            "seq {} vượt Number.MAX_SAFE_INTEGER của submitter JS",
                            a.seq
                        )));
                    }
                    recs.push(serde_json::json!({
                        "t": 1,
                        "ref_id": hex::encode(a.ref_id),
                        "head_version_hash": hex::encode(a.head_version_hash),
                        "mmr_root": hex::encode(a.mmr_root),
                        "seq": a.seq,
                    }));
                }
                SettlementRecord::KeyRotation(p) => {
                    recs.push(serde_json::json!({ "t": 2, "payload": hex::encode(p) }));
                }
            }
        }
        let req = serde_json::json!({ "label": self.label, "records": recs });

        let mut child = std::process::Command::new("npx")
            .args(["tsx", "submit.ts"])
            .current_dir(&self.submitter_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(spawn_err)?;

        child
            .stdin
            .take()
            .expect("stdin piped")
            .write_all(req.to_string().as_bytes())
            .map_err(|e| AnchorError::Network(format!("ghi stdin submitter: {e}")))?;
        // stdin drop → EOF.

        // Chờ có timeout (poll try_wait).
        let deadline = std::time::Instant::now() + Duration::from_secs(self.timeout_secs);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        return Err(AnchorError::Network("submitter timeout".into()));
                    }
                    std::thread::sleep(Duration::from_millis(300));
                }
                Err(e) => return Err(AnchorError::Network(format!("wait submitter: {e}"))),
            }
        }
        let mut stdout = String::new();
        let mut stderr = String::new();
        if let Some(mut o) = child.stdout.take() {
            let _ = o.read_to_string(&mut stdout);
        }
        if let Some(mut e) = child.stderr.take() {
            let _ = e.read_to_string(&mut stderr);
        }

        // stdout: dòng JSON cuối cùng là kết quả.
        let last_json = stdout
            .lines()
            .rev()
            .find(|l| l.trim_start().starts_with('{'))
            .ok_or_else(|| {
                AnchorError::Rejected(format!(
                    "submitter không trả JSON; stderr: {}",
                    truncate(&stderr, 800)
                ))
            })?;
        let v: serde_json::Value = serde_json::from_str(last_json)
            .map_err(|e| AnchorError::Rejected(format!("submitter JSON hỏng: {e}")))?;

        if v.get("ok").and_then(|b| b.as_bool()) == Some(true) {
            let txid = v
                .get("txid")
                .and_then(|t| t.as_str())
                .ok_or_else(|| AnchorError::Rejected("submitter thiếu txid".into()))?
                .to_string();
            let address = v
                .get("address")
                .and_then(|a| a.as_str())
                .unwrap_or_default()
                .to_string();
            Ok(SubmitOutcome { txid, address })
        } else {
            let kind = v
                .get("error_kind")
                .and_then(|k| k.as_str())
                .unwrap_or("Rejected");
            let msg = v
                .get("error")
                .and_then(|m| m.as_str())
                .unwrap_or("submitter lỗi không rõ")
                .to_string();
            Err(match kind {
                "NotConfigured" => AnchorError::NotConfigured,
                "Network" => AnchorError::Network(msg),
                "InsufficientAda" => AnchorError::InsufficientAda {
                    need: v.get("need").and_then(|n| n.as_u64()).unwrap_or(0),
                    have: v.get("have").and_then(|n| n.as_u64()).unwrap_or(0),
                },
                "DatumTooLarge" => AnchorError::DatumTooLarge {
                    bytes: v.get("bytes").and_then(|n| n.as_u64()).unwrap_or(0) as usize,
                },
                _ => AnchorError::Rejected(msg),
            })
        }
    }
}

/// spawn fail: binary thiếu (npx không có) → NotConfigured; khác → Network.
fn spawn_err(e: std::io::Error) -> AnchorError {
    if e.kind() == std::io::ErrorKind::NotFound {
        AnchorError::NotConfigured
    } else {
        AnchorError::Network(format!("spawn submitter: {e}"))
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

    #[test]
    fn ts_submitter_debug_hides_nothing_secret() {
        let s = TsSubmitter {
            submitter_dir: PathBuf::from("/x/submitter"),
            label: 1234,
            timeout_secs: 60,
        };
        let dbg = format!("{s:?}");
        assert!(dbg.contains("1234"));
    }

    #[test]
    fn ts_submitter_missing_npx_is_not_configured() {
        // Thư mục trống + PATH giữ nguyên: nếu npx KHÔNG có → NotConfigured; nếu CÓ npx
        // nhưng submit.ts thiếu → child chạy fail → Rejected/Network. Test chỉ khẳng định
        // KHÔNG panic và trả Err (không phụ thuộc máy CI có npx hay không).
        let s = TsSubmitter {
            submitter_dir: PathBuf::from("/nonexistent-dir-xyz"),
            label: 1234,
            timeout_secs: 5,
        };
        let a = lampnet_strata::chain::StrataAnchor {
            ref_id: [1; 32],
            head_version_hash: [2; 32],
            mmr_root: [3; 32],
            seq: 0,
        };
        let r = s.submit(&[SettlementRecord::Anchor(a)]);
        assert!(r.is_err());
    }

    #[test]
    fn seq_over_js_safe_rejected() {
        let s = TsSubmitter {
            submitter_dir: PathBuf::from("/x"),
            label: 1234,
            timeout_secs: 5,
        };
        let a = lampnet_strata::chain::StrataAnchor {
            ref_id: [1; 32],
            head_version_hash: [2; 32],
            mmr_root: [3; 32],
            seq: (1u64 << 53) + 1, // vượt Number.MAX_SAFE_INTEGER
        };
        let r = s.submit(&[SettlementRecord::Anchor(a)]);
        assert!(matches!(r, Err(AnchorError::Rejected(_))));
    }
}
