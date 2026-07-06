//! `SettlementSink` — backend Settlement (tx metadata label 1234) cho [`AnchorSink`].
//!
//! Kiến trúc tách để test được:
//! - [`ChainQuery`] — đọc on-chain (Blockfrost thật hoặc mock). `resolve()` CHỈ tin
//!   tx có địa chỉ publisher trong **INPUT** (tx do publisher ký/chi) — tx lạ mang
//!   label 1234, kể cả tx *gửi tiền tới* publisher, đều bị bỏ qua (chống đầu độc
//!   indexer).
//! - [`Submitter`] — build+sign+submit tx (child-process TS Lucid Evolution, hoặc mock).
//!
//! Idempotency §8.1b (enforce TRƯỚC khi build tx):
//! `on_chain_seq == anchor.seq` → `Ok(None)`; `on_chain_seq > anchor.seq` →
//! `Err(RollbackAttempt)`. Chỉ `Network` retryable — [`publish_with_retry`].

use crate::payload::{AnchorRecord, decode_records_lenient, encode_records};
use crate::{AnchorBackend, AnchorError, AnchorPriority, AnchorReceipt, AnchorSink};
use lampnet_strata::{Hash32, StrataAnchor};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

/// Đọc on-chain — trừu tượng hoá Blockfrost để mock được trong unit test.
pub trait ChainQuery {
    /// Tx hash liên quan `addr`, MỚI → CŨ, tối đa `limit` tx.
    fn address_txs(&self, addr: &str, limit: usize) -> Result<Vec<String>, AnchorError>;
    /// Địa chỉ các INPUT (không tính collateral) của tx.
    fn tx_input_addresses(&self, txid: &str) -> Result<Vec<String>, AnchorError>;
    /// CBOR metadatum (raw bytes) của `label` trong tx; `None` nếu tx không có label.
    fn tx_metadata_cbor(&self, txid: &str, label: u64) -> Result<Option<Vec<u8>>, AnchorError>;
}

/// Kết quả submit từ backend build tx.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitOutcome {
    /// Tx hash.
    pub txid: String,
    /// Địa chỉ ví đã ký (đối chiếu publisher pin trong config).
    pub address: String,
}

/// Build + sign + submit tx metadata — trừu tượng hoá submitter TS để mock được.
pub trait Submitter {
    /// Submit tx với metadatum label 1234 = các record đã cho. Trả txid + địa chỉ ví ký.
    fn submit(&self, records: &[AnchorRecord]) -> Result<SubmitOutcome, AnchorError>;
}

/// Cấu hình sink Settlement.
#[derive(Debug, Clone)]
pub struct SinkConfig {
    /// Ví dịch vụ công bố (publisher) — TRUST PIN v1: anchor chỉ hợp lệ nếu tx
    /// phát (input) từ ví này.
    pub publisher_address: String,
    /// Nhãn metadata (mặc định [`crate::METADATA_LABEL`] = 1234).
    pub label: u64,
    /// Trần kích thước metadatum (byte) — vượt → [`AnchorError::DatumTooLarge`].
    /// Cardano maxTxSize ~16384; để dư địa cho phần tx còn lại.
    pub max_metadatum_bytes: usize,
    /// Trần số tx quét khi `resolve` (ví publisher dùng chung có nhiều tx).
    pub resolve_scan_limit: usize,
}

impl Default for SinkConfig {
    fn default() -> Self {
        Self {
            publisher_address: String::new(),
            label: crate::METADATA_LABEL,
            max_metadatum_bytes: 8 * 1024,
            resolve_scan_limit: 500,
        }
    }
}

/// Sink Settlement: metadata label 1234, generic theo query + submitter.
pub struct SettlementSink<Q: ChainQuery, S: Submitter> {
    cfg: SinkConfig,
    query: Q,
    submitter: S,
}

impl<Q: ChainQuery, S: Submitter> SettlementSink<Q, S> {
    /// Tạo sink; publisher_address rỗng → coi như chưa cấu hình (mọi call trả
    /// [`AnchorError::NotConfigured`]).
    pub fn new(cfg: SinkConfig, query: Q, submitter: S) -> Self {
        Self { cfg, query, submitter }
    }

    fn ensure_configured(&self) -> Result<(), AnchorError> {
        if self.cfg.publisher_address.is_empty() {
            return Err(AnchorError::NotConfigured);
        }
        Ok(())
    }

    /// Gộp NHIỀU anchor (nhiều chain) vào MỘT tx. Idempotency kiểm từng ref_id;
    /// anchor đã neo rồi bị loại khỏi lô; nếu lô rỗng sau lọc → `Ok(None)`.
    /// Bất kỳ anchor nào bị rollback → fail cả lô TRƯỚC khi build tx.
    pub fn publish_batch(
        &self,
        anchors: &[StrataAnchor],
    ) -> Result<Option<AnchorReceipt>, AnchorError> {
        self.ensure_configured()?;
        let mut fresh: Vec<AnchorRecord> = Vec::new();
        for a in anchors {
            match self.resolve(&a.ref_id)? {
                Some(on_chain) if on_chain.seq > a.seq => {
                    return Err(AnchorError::RollbackAttempt {
                        on_chain_seq: on_chain.seq,
                        attempted: a.seq,
                    });
                }
                Some(on_chain) if on_chain.seq == a.seq => {
                    // idempotent no-op cho anchor này.
                }
                _ => fresh.push(AnchorRecord::Anchor(a.clone())),
            }
        }
        if fresh.is_empty() {
            return Ok(None);
        }
        let cbor = encode_records(&fresh);
        if cbor.len() > self.cfg.max_metadatum_bytes {
            return Err(AnchorError::DatumTooLarge { bytes: cbor.len() });
        }
        let outcome = self.submitter.submit(&fresh)?;
        if outcome.address != self.cfg.publisher_address {
            // Ví submitter KHÔNG phải publisher đã pin → anchor vừa đẩy sẽ bị chính
            // resolve() bỏ qua. Fail to hơn im lặng.
            return Err(AnchorError::Rejected(format!(
                "ví submitter ({}) != publisher pin trong config ({})",
                outcome.address, self.cfg.publisher_address
            )));
        }
        Ok(Some(AnchorReceipt {
            txid: outcome.txid,
            backend: AnchorBackend::Settlement,
            slot: None,
        }))
    }
}

impl<Q: ChainQuery, S: Submitter> AnchorSink for SettlementSink<Q, S> {
    fn publish(
        &self,
        anchor: &StrataAnchor,
        priority: AnchorPriority,
    ) -> Result<Option<AnchorReceipt>, AnchorError> {
        if priority == AnchorPriority::NoAnchor {
            return Ok(None);
        }
        self.publish_batch(std::slice::from_ref(anchor))
    }

    fn resolve(&self, ref_id: &Hash32) -> Result<Option<StrataAnchor>, AnchorError> {
        self.ensure_configured()?;
        let txs = self
            .query
            .address_txs(&self.cfg.publisher_address, self.cfg.resolve_scan_limit)?;
        let mut best: Option<StrataAnchor> = None;
        for txid in txs {
            let Some(cbor) = self.query.tx_metadata_cbor(&txid, self.cfg.label)? else {
                continue;
            };
            // TRUST: chỉ tin tx do publisher CHI (địa chỉ publisher trong input).
            // address_txs trả cả tx GỬI TỚI publisher (VD faucet) → phải lọc input.
            let inputs = self.query.tx_input_addresses(&txid)?;
            if !inputs.iter().any(|a| a == &self.cfg.publisher_address) {
                continue; // tx từ ví lạ mang label 1234 → bỏ qua
            }
            for rec in decode_records_lenient(&cbor) {
                if let AnchorRecord::Anchor(a) = rec
                    && a.ref_id == *ref_id
                    && best.as_ref().is_none_or(|b| a.seq > b.seq)
                {
                    best = Some(a);
                }
            }
        }
        Ok(best)
    }
}

/// Retry CHỈ với [`AnchorError::Network`] (backoff mũ). Mọi lỗi khác fail-hard ngay.
pub fn publish_with_retry<K: AnchorSink>(
    sink: &K,
    anchor: &StrataAnchor,
    priority: AnchorPriority,
    max_attempts: u32,
    base_backoff: Duration,
) -> Result<Option<AnchorReceipt>, AnchorError> {
    let mut attempt = 0u32;
    loop {
        match sink.publish(anchor, priority) {
            Err(e) if e.is_retryable() && attempt + 1 < max_attempts => {
                std::thread::sleep(base_backoff * 2u32.saturating_pow(attempt));
                attempt += 1;
            }
            other => return other,
        }
    }
}

// ---------------------------------------------------------------------------
// Blockfrost ChainQuery (Preview) — reqwest blocking.
// ---------------------------------------------------------------------------

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
        Self::new("https://cardano-preview.blockfrost.io/api/v0".into(), project_id)
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

// ---------------------------------------------------------------------------
// TsSubmitter — child process Node/tsx (Lucid Evolution + Blockfrost).
// ---------------------------------------------------------------------------

/// Submitter gọi `submitter/submit.ts` qua stdin/stdout JSON.
/// Secret (mnemonic/token) đi bằng ENV của process cha — KHÔNG qua argv, KHÔNG log.
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
    fn submit(&self, records: &[AnchorRecord]) -> Result<SubmitOutcome, AnchorError> {
        // JS Number an toàn tới 2^53-1; seq vượt → từ chối trước khi sang TS.
        const JS_MAX_SAFE: u64 = (1u64 << 53) - 1;
        let mut recs = Vec::with_capacity(records.len());
        for r in records {
            match r {
                AnchorRecord::Anchor(a) => {
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
                AnchorRecord::KeyRotation(p) => {
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
            let kind = v.get("error_kind").and_then(|k| k.as_str()).unwrap_or("Rejected");
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
