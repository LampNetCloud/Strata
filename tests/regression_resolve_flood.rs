//! Regression test cho issue #14 — `SettlementSink::resolve()` bị làm mù bằng flood
//! tx rẻ tiền gửi TỚI publisher: anchor thật bị đẩy ra ngoài cửa sổ quét
//! `resolve_scan_limit` → resolve trả `None` dù anchor còn nguyên on-chain, vô hiệu
//! guard chống-rollback INV-E7 cross-process.
//!
//! Nguồn: audit hội đồng 2026-07-22 (Strata agent), đã xác nhận bằng PoC thực thi.
//!
//! - `resolve_must_not_be_blinded_by_flood` — BẤT BIẾN MONG MUỐN, hiện FAIL (bug) nên
//!   `#[ignore]`. Người vá #14 (beacon NFT / con-trỏ-latest xác thực) BỎ `#[ignore]`
//!   để test xanh — đó là tiêu chí "xong" đo được của bản vá.
//! - `resolve_control_no_flood` — đường hạnh phúc (không flood) resolve đúng; KHÔNG
//!   ignore, canh hồi quy đường thường.

use std::collections::HashMap;

use lampnet_strata::{
    AnchorError, AnchorSink, ChainQuery, SettlementRecord, SettlementSink, SinkConfig, StrataAnchor,
    SubmitOutcome, Submitter, encode_records,
};

const PUBLISHER: &str = "addr_publisher";
const ATTACKER: &str = "addr_attacker";
const REF_ID: [u8; 32] = [0x11; 32];
const REAL_SEQ: u64 = 5;

struct MockQuery {
    txs: Vec<String>, // MỚI → CŨ (đúng order=desc của Blockfrost)
    inputs: HashMap<String, Vec<String>>,
    meta: HashMap<String, Vec<u8>>,
}
impl ChainQuery for MockQuery {
    fn address_txs(&self, addr: &str, limit: usize) -> Result<Vec<String>, AnchorError> {
        if addr != PUBLISHER {
            return Ok(Vec::new());
        }
        Ok(self.txs.iter().take(limit).cloned().collect()) // take(limit) = cửa sổ hữu hạn
    }
    fn tx_input_addresses(&self, txid: &str) -> Result<Vec<String>, AnchorError> {
        Ok(self.inputs.get(txid).cloned().unwrap_or_default())
    }
    fn tx_metadata_cbor(&self, txid: &str, _label: u64) -> Result<Option<Vec<u8>>, AnchorError> {
        Ok(self.meta.get(txid).cloned())
    }
}
struct NoSubmit;
impl Submitter for NoSubmit {
    fn submit(&self, _r: &[SettlementRecord]) -> Result<SubmitOutcome, AnchorError> {
        Err(AnchorError::Network("không dùng trong test resolve".into()))
    }
}

/// Dựng sink: `n_garbage` tx rác MỚI nhất (attacker chi, gửi tới publisher) đứng trước
/// anchor THẬT (publisher chi) cũ hơn. `scan_limit` = cửa sổ quét.
fn build(scan_limit: usize, n_garbage: usize) -> SettlementSink<MockQuery, NoSubmit> {
    let real = StrataAnchor {
        ref_id: REF_ID,
        head_version_hash: [0x22; 32],
        mmr_root: [0x33; 32],
        seq: REAL_SEQ,
    };
    let real_cbor = encode_records(&[SettlementRecord::Anchor(real)]);

    let mut txs = Vec::new();
    let mut inputs = HashMap::new();
    let mut meta = HashMap::new();
    for i in 0..n_garbage {
        let t = format!("garbage{i}");
        txs.push(t.clone());
        inputs.insert(t.clone(), vec![ATTACKER.to_string()]);
        meta.insert(t.clone(), encode_records(&[])); // rác cũng mang label 1234
    }
    let real_tx = "real_anchor_tx".to_string();
    txs.push(real_tx.clone());
    inputs.insert(real_tx.clone(), vec![PUBLISHER.to_string()]);
    meta.insert(real_tx.clone(), real_cbor);

    let cfg = SinkConfig {
        publisher_address: PUBLISHER.to_string(),
        resolve_scan_limit: scan_limit,
        ..Default::default()
    };
    SettlementSink::new(cfg, MockQuery { txs, inputs, meta }, NoSubmit)
}

/// BẤT BIẾN MONG MUỐN (issue #14): anchor đã neo phải resolve được kể cả khi kẻ tấn
/// công bơm đủ tx rác lấp đầy cửa sổ quét. Hiện FAIL → `#[ignore]`. Bỏ ignore khi vá.
#[test]
#[ignore = "KNOWN BUG #14: resolve bị flood làm mù (trả None). Bỏ #[ignore] khi vá xong."]
fn resolve_must_not_be_blinded_by_flood() {
    // Cửa sổ 3, nhồi 3 tx rác → anchor thật (thứ 4) rơi ngoài.
    let sink = build(3, 3);
    let got = sink.resolve(&REF_ID).unwrap();
    assert_eq!(
        got.map(|a| a.seq),
        Some(REAL_SEQ),
        "anchor đã neo phải resolve được dù bị flood — hiện trả None (bug #14)"
    );
}

/// Canh hồi quy đường thường: không flood thì resolve đúng anchor.
#[test]
fn resolve_control_no_flood() {
    let sink = build(10, 3); // cửa sổ đủ rộng, anchor thật còn trong tầm
    let got = sink.resolve(&REF_ID).unwrap();
    assert_eq!(got.map(|a| a.seq), Some(REAL_SEQ));
}
