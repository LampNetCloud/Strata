//! Regression test cho issue #14 — `SettlementSink::resolve()` bị làm mù bằng flood
//! tx rẻ tiền gửi TỚI publisher: anchor thật bị đẩy ra ngoài cửa sổ quét
//! `resolve_scan_limit` → resolve trả `None` dù anchor còn nguyên on-chain, vô hiệu
//! guard chống-rollback INV-E7 cross-process.
//!
//! Nguồn: audit hội đồng 2026-07-22 (Strata agent), đã xác nhận bằng PoC thực thi.
//!
//! Bản vá (issue #14, hướng A-opt): `beacon_mode` — `resolve` xác định latest theo
//! ASSET (beacon NFT `unit = policy ++ ref_id`, native policy `sig(publisher)`) thay vì
//! quét cửa sổ địa chỉ. Kẻ lạ không mint/di chuyển được beacon ⇒ flood KHÔNG chạm tới.
//!
//! - `resolve_must_not_be_blinded_by_flood` — BẤT BIẾN issue #14, nay chạy ở BEACON mode
//!   → PASS (trước khi có beacon thì FAIL, đó là lý do bản vá tồn tại).
//! - `resolve_control_no_flood` — beacon mode, không flood → resolve đúng.
//! - `legacy_mode_is_blinded_by_flood_documented` — tài-liệu-hoá GIỚI HẠN của đường
//!   legacy (`beacon_policy = None`): flood VẪN làm mù (trả `None`). Đây là đánh đổi đã
//!   ghi rõ, KHÔNG phải hồi quy — nó chốt "vì sao cần beacon_mode".
//!
//! ## issue #78 — nhánh fail-open nằm TRONG chính bản vá trên
//!
//! `resolve_via_beacon` tìm được beacon, xác nhận tx mới nhất do publisher chi, rồi trả
//! `Ok(None)` khi tx đó không đọc ra anchor. Người gọi đọc `None` là **"chưa neo bao
//! giờ"** ⇒ không có mốc so `seq` ⇒ gác INV-E7 không chạy. Tức bản vá của #14 mở lại
//! đúng cái cổng nó đóng, chỉ qua một cửa khác.
//!
//! Hai ca dưới canh hai nhánh ĐỘC LẬP dẫn tới đó — nhánh thứ hai không được issue nêu:
//!
//! - `beacon_tx_without_label_must_fail_closed` — beacon bị cuốn theo một tx không mang
//!   metadata label (coin-selection trả phí, gộp UTxO).
//! - `beacon_tx_with_other_refs_must_fail_closed` — tx CÓ label nhưng chỉ chứa anchor
//!   của ref khác. `fold_best_anchor` trả `None` ở cả hai nghĩa, nên nhánh này đi lọt
//!   kể cả sau khi vá riêng nhánh thứ nhất.
//!
//! Cả hai đo cùng một điều: **"không đọc được" phải kêu to hơn "chưa neo"**, không được
//! mang cùng một giá trị trả về.

use std::collections::HashMap;

use lampnet_strata::{
    AnchorError, AnchorSink, ChainQuery, SettlementRecord, SettlementSink, SinkConfig,
    StrataAnchor, SubmitOutcome, Submitter, encode_records,
};

const PUBLISHER: &str = "addr_publisher";
const ATTACKER: &str = "addr_attacker";
const REF_ID: [u8; 32] = [0x11; 32];
const REAL_SEQ: u64 = 5;
/// PolicyId beacon (28 byte = 56 hex) — khoá native `sig(publisher)`.
const BEACON_POLICY: &str = "beac04beac04beac04beac04beac04beac04beac04beac04beac04be";

struct MockQuery {
    txs: Vec<String>, // MỚI → CŨ (đúng order=desc của Blockfrost)
    inputs: HashMap<String, Vec<String>>,
    meta: HashMap<String, Vec<u8>>,
    asset_latest: HashMap<String, String>, // unit → tx mới nhất đụng asset (beacon)
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
    fn asset_latest_tx(&self, unit: &str) -> Result<Option<String>, AnchorError> {
        Ok(self.asset_latest.get(unit).cloned())
    }
}
struct NoSubmit;
impl Submitter for NoSubmit {
    fn submit(&self, _r: &[SettlementRecord]) -> Result<SubmitOutcome, AnchorError> {
        Err(AnchorError::Network("không dùng trong test resolve".into()))
    }
}

/// Dựng dữ liệu on-chain giả: `n_garbage` tx rác MỚI nhất (attacker chi, gửi tới
/// publisher) đứng trước anchor THẬT (publisher chi) cũ hơn; đăng ký beacon unit trỏ
/// tới đúng tx anchor thật. `beacon` = bật beacon_mode, `scan_limit` = cửa sổ legacy.
fn build(beacon: bool, scan_limit: usize, n_garbage: usize) -> SettlementSink<MockQuery, NoSubmit> {
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

    // Beacon: unit = policy ++ ref_id, tx mới nhất của asset = tx anchor thật. Kẻ tấn công
    // KHÔNG chạm được asset này (không mint/di chuyển được) → flood ở `txs` vô hại.
    let mut asset_latest = HashMap::new();
    // assetName = hex(REF_ID); REF_ID = [0x11; 32] → "11" lặp 32 lần (64 hex).
    let unit = format!("{BEACON_POLICY}{}", "11".repeat(32));
    asset_latest.insert(unit, real_tx);

    let cfg = SinkConfig {
        publisher_address: PUBLISHER.to_string(),
        resolve_scan_limit: scan_limit,
        beacon_policy: beacon.then(|| BEACON_POLICY.to_string()),
        ..Default::default()
    };
    SettlementSink::new(
        cfg,
        MockQuery {
            txs,
            inputs,
            meta,
            asset_latest,
        },
        NoSubmit,
    )
}

/// BẤT BIẾN issue #14: anchor đã neo phải resolve được kể cả khi kẻ tấn công bơm đủ tx
/// rác lấp đầy cửa sổ. Ở BEACON mode điều này ĐẠT (beacon miễn nhiễm flood).
#[test]
fn resolve_must_not_be_blinded_by_flood() {
    // Cửa sổ legacy chỉ 3 (sẽ bị 3 tx rác lấp đầy) — nhưng beacon KHÔNG dùng cửa sổ này.
    let sink = build(
        /* beacon */ true, /* scan_limit */ 3, /* n_garbage */ 3,
    );
    let got = sink.resolve(&REF_ID).unwrap();
    assert_eq!(
        got.map(|a| a.seq),
        Some(REAL_SEQ),
        "beacon_mode: anchor đã neo phải resolve được dù bị flood"
    );
}

/// Canh hồi quy đường thường (beacon, không flood): resolve đúng anchor.
#[test]
fn resolve_control_no_flood() {
    let sink = build(
        /* beacon */ true, /* scan_limit */ 10, /* n_garbage */ 0,
    );
    let got = sink.resolve(&REF_ID).unwrap();
    assert_eq!(got.map(|a| a.seq), Some(REAL_SEQ));
}

/// Tài-liệu-hoá GIỚI HẠN của đường legacy (`beacon_policy = None`): flood lấp cửa sổ VẪN
/// làm `resolve` trả `None`. Đây là đánh đổi đã ghi rõ (publisher-1-ref_id / reader tin
/// daemon vẫn an toàn) và là lý do beacon_mode tồn tại — KHÔNG phải bug hồi quy.
#[test]
fn legacy_mode_is_blinded_by_flood_documented() {
    let sink = build(
        /* beacon */ false, /* scan_limit */ 3, /* n_garbage */ 3,
    );
    let got = sink.resolve(&REF_ID).unwrap();
    assert_eq!(
        got.map(|a| a.seq),
        None,
        "legacy mode: flood đẩy anchor ra ngoài cửa sổ → None (đánh đổi đã ghi, dùng beacon_mode để chống)"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// issue #78 — "không đọc được" KHÔNG được trả về cùng giá trị với "chưa neo"
// ────────────────────────────────────────────────────────────────────────────

/// Dựng sink beacon-mode với beacon trỏ vào `beacon_tx`, và `beacon_tx` mang đúng
/// `meta_of_beacon_tx` (None = tx không có metadata label 1234). Anchor THẬT của
/// `REF_ID` vẫn nằm nguyên trên chuỗi ở một tx cũ hơn — nên mọi kết luận "chưa neo"
/// trong hai ca dưới đều là kết luận SAI về trạng thái thật.
fn build_beacon_pointing_at(
    beacon_tx: &str,
    meta_of_beacon_tx: Option<Vec<u8>>,
) -> SettlementSink<MockQuery, NoSubmit> {
    let real = StrataAnchor {
        ref_id: REF_ID,
        head_version_hash: [0x22; 32],
        mmr_root: [0x33; 32],
        seq: REAL_SEQ,
    };
    let mut txs = vec![beacon_tx.to_string(), "real_anchor_tx".to_string()];
    let mut inputs = HashMap::new();
    let mut meta = HashMap::new();
    // Cả hai tx đều do publisher CHI — phép kiểm defense-in-depth ở trên phải ĐẠT,
    // để ca này đo đúng nhánh metadata chứ không vô tình đo nhánh input.
    inputs.insert(beacon_tx.to_string(), vec![PUBLISHER.to_string()]);
    inputs.insert("real_anchor_tx".to_string(), vec![PUBLISHER.to_string()]);
    meta.insert(
        "real_anchor_tx".to_string(),
        encode_records(&[SettlementRecord::Anchor(real)]),
    );
    if let Some(m) = meta_of_beacon_tx {
        meta.insert(beacon_tx.to_string(), m);
    }
    txs.dedup();

    let mut asset_latest = HashMap::new();
    asset_latest.insert(
        format!("{BEACON_POLICY}{}", "11".repeat(32)),
        beacon_tx.to_string(),
    );

    let cfg = SinkConfig {
        publisher_address: PUBLISHER.to_string(),
        resolve_scan_limit: 10,
        beacon_policy: Some(BEACON_POLICY.to_string()),
        ..Default::default()
    };
    SettlementSink::new(
        cfg,
        MockQuery {
            txs,
            inputs,
            meta,
            asset_latest,
        },
        NoSubmit,
    )
}

/// Nhánh #78 nêu: beacon bị cuốn theo một tx KHÔNG mang metadata label 1234 (ví
/// publisher trả phí / gộp UTxO). Trước bản vá: `Ok(None)` ⇒ người gọi hiểu "chưa neo"
/// ⇒ mất mốc so `seq` ⇒ INV-E7 không được gác. Sau vá: `Err`, và thông điệp phải tự
/// nói nó là trạng thái "không đọc được".
#[test]
fn beacon_tx_without_label_must_fail_closed() {
    let sink = build_beacon_pointing_at("fee_sweep_tx", None);
    let got = sink.resolve(&REF_ID);
    let Err(AnchorError::Rejected(msg)) = got else {
        panic!("beacon tồn tại nhưng tx không đọc được anchor: phải fail-closed, nhận {got:?}");
    };
    assert!(
        msg.contains("KHÔNG mang metadata"),
        "thông điệp phải nói rõ nhánh nào, nhận: {msg}"
    );
}

/// Nhánh issue KHÔNG nêu, nằm thấp hơn đúng một dòng: tx CÓ metadata label 1234 nhưng
/// chỉ chứa anchor của **ref khác** (beacon đi kèm một lô của lineage khác — chuyện
/// thường, vì publisher gộp nhiều ref vào một tx). `fold_best_anchor` trả `None` ở đây
/// với cùng một nghĩa mơ hồ, nên vá riêng nhánh trên là chưa đủ.
#[test]
fn beacon_tx_with_other_refs_must_fail_closed() {
    let other = StrataAnchor {
        ref_id: [0x99; 32],
        head_version_hash: [0x22; 32],
        mmr_root: [0x33; 32],
        seq: 1,
    };
    let sink = build_beacon_pointing_at(
        "batch_of_other_refs_tx",
        Some(encode_records(&[SettlementRecord::Anchor(other)])),
    );
    let got = sink.resolve(&REF_ID);
    let Err(AnchorError::Rejected(msg)) = got else {
        panic!("beacon trỏ vào lô của ref khác: phải fail-closed, nhận {got:?}");
    };
    assert!(
        msg.contains("KHÔNG chứa"),
        "thông điệp phải phân biệt được với nhánh thiếu label, nhận: {msg}"
    );
}
