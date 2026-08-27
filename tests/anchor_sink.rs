//! S1 — `AnchorSink → Mosaic` (CIP-68): 6 tiêu chí test §8.1 + CBOR round-trip.
//!
//! Boundary Mosaic = [`MockMosaic`] (in-memory) — thay cho tx-builder Lucid/CSL phía
//! VeData (Phase 2). Test #2 "Preview tx thật" là DoD Phase 2; ở đây test luồng resolve
//! qua mock (datum khớp `mmr_root`/`head_version_hash`).

use std::cell::RefCell;
use std::collections::HashMap;

use ed25519_dalek::SigningKey;
use lampnet_strata::anchor_sink::{
    AnchorBackend, AnchorError, AnchorPriority, AnchorReceipt, AnchorSink, AnchoredTable,
    AssetClass, MosaicAnchorSink, MosaicBackend, PlutusData, ResolvedAnchor, TableError,
    map_anchor_to_datum, parse_datum_to_anchor, verify_resolved,
};
use lampnet_strata::chain::{Policy, StrataAnchor, StrataChain};
use lampnet_strata::refid::gen_ref_id_raw;
use lampnet_strata::state::build_state_root;
use lampnet_strata::version::StrataVersion;
use rand::rngs::OsRng;

// ── helper dựng chain ────────────────────────────────────────────────────────
struct Author {
    did: [u8; 32],
    sk: SigningKey,
}
fn author(tag: u8) -> Author {
    Author {
        did: [tag; 32],
        sk: SigningKey::generate(&mut OsRng),
    }
}
fn policy(a: &Author) -> Policy {
    let mut p = Policy::new();
    p.allow(a.did, a.sk.verifying_key());
    p
}
fn signed(seq: u64, prev: [u8; 32], ts: u64, a: &Author, ph: [u8; 32]) -> StrataVersion {
    let sr = build_state_root(&[(b"name".to_vec(), format!("v{seq}").into_bytes())]);
    let mut v = StrataVersion::unsigned(seq, prev, b"cid".to_vec(), sr, a.did, ph, ts);
    v.sign(&a.sk);
    v
}
/// Chain với `n` version (seq 0..n-1).
fn chain_of(n: u64) -> (StrataChain, Policy, Author) {
    let a = author(1);
    let pol = policy(&a);
    let ph = pol.policy_hash();
    let ref_id = gen_ref_id_raw(&a.did, &[9u8; 32]);
    let v0 = signed(0, [0u8; 32], 100, &a, ph);
    let mut chain = StrataChain::genesis(ref_id, v0, &pol).unwrap();
    for seq in 1..n {
        let v = signed(seq, chain.head_version_hash(), 100 + seq, &a, ph);
        chain.append_version(v, &pol).unwrap();
    }
    (chain, pol, a)
}

/// Thread-token NFT one-shot mẫu (test).
fn tok(tag: u8) -> AssetClass {
    AssetClass {
        policy_id: [tag; 28],
        asset_name: b"LN-STRATA-THREAD".to_vec(),
    }
}

/// Token mặc định cho các test đường GHI.
///
/// `publish` nay **fail-đóng** khi sink không pin thread-token: không pin thì `resolve()`
/// không phân biệt được UTxO thật với UTxO kẻ lạ cấy vào địa chỉ script, nên đường ghi mù.
/// Mọi test từng dựng sink bằng `new_unverified_for_tests` rồi gọi `publish` nay phải pin —
/// đúng ràng buộc mà production phải theo.
fn dtok() -> AssetClass {
    tok(0xAA)
}

/// Sink pin token + mock phát UTxO authentic mang đúng token đó.
fn pinned_sink(mock: MockMosaic) -> MosaicAnchorSink<MockMosaic> {
    MosaicAnchorSink::with_thread_token(mock.pin(dtok()), dtok())
}

// ── mock backend Mosaic (in-memory on-chain state) ───────────────────────────
struct MockMosaic {
    state: RefCell<HashMap<[u8; 32], PlutusData>>, // ref_id → datum authentic mới nhất
    tx_count: RefCell<usize>,
    fail_submit: Option<AnchorError>, // nếu set → submit trả lỗi này
    fail_read: Option<AnchorError>,   // nếu set → read_anchor trả lỗi này
    token: Option<AssetClass>,        // thread-token UTxO authentic mang (None = không mang)
    forged: RefCell<Vec<ResolvedAnchor>>, // UTxO giả kẻ lạ gửi vào script address
}
impl MockMosaic {
    fn new() -> Self {
        Self {
            state: RefCell::new(HashMap::new()),
            tx_count: RefCell::new(0),
            fail_submit: None,
            fail_read: None,
            token: None,
            forged: RefCell::new(Vec::new()),
        }
    }
    fn failing(e: AnchorError) -> Self {
        Self {
            fail_submit: Some(e),
            ..Self::new()
        }
    }
    /// Gắn thread-token authentic cho một mock đã dựng (giữ nguyên các trường khác).
    fn pin(mut self, token: AssetClass) -> Self {
        self.token = Some(token);
        self
    }
    /// Mock có thread-token authentic (production-mode).
    fn with_token(token: AssetClass) -> Self {
        Self {
            token: Some(token),
            ..Self::new()
        }
    }
    /// Kẻ lạ gửi một UTxO giả (datum + token tuỳ ý) vào địa chỉ script.
    fn inject_forged(&self, datum: PlutusData, thread_token: Option<AssetClass>) {
        self.forged.borrow_mut().push(ResolvedAnchor {
            datum,
            thread_token,
        });
    }
    fn tx_count(&self) -> usize {
        *self.tx_count.borrow()
    }
}
impl MosaicBackend for MockMosaic {
    fn submit_anchor(&self, datum: &PlutusData) -> Result<AnchorReceipt, AnchorError> {
        if let Some(e) = &self.fail_submit {
            return Err(e.clone());
        }
        let a = parse_datum_to_anchor(datum).unwrap();
        self.state.borrow_mut().insert(a.ref_id, datum.clone());
        *self.tx_count.borrow_mut() += 1;
        Ok(AnchorReceipt {
            txid: format!("mocktx{}", self.tx_count()),
            backend: AnchorBackend::Mosaic,
            slot: Some(1000),
        })
    }
    /// Trả MỌI UTxO ứng viên (authentic + forged) — sink tự lọc theo thread-token.
    fn read_anchor(&self, ref_id: &[u8; 32]) -> Result<Vec<ResolvedAnchor>, AnchorError> {
        if let Some(e) = &self.fail_read {
            return Err(e.clone());
        }
        let mut out = Vec::new();
        if let Some(datum) = self.state.borrow().get(ref_id).cloned() {
            out.push(ResolvedAnchor {
                datum,
                thread_token: self.token.clone(),
            });
        }
        out.extend(self.forged.borrow().iter().cloned());
        Ok(out)
    }
}

fn sample_anchor(seq: u64) -> StrataAnchor {
    StrataAnchor {
        ref_id: [7u8; 32],
        head_version_hash: [8u8; 32],
        mmr_root: [9u8; 32],
        seq,
    }
}

// ── #1 map round-trip (datum + CBOR) ─────────────────────────────────────────
#[test]
fn s1_criteria_1_datum_roundtrip_bit_exact() {
    let a = StrataAnchor {
        ref_id: [1u8; 32],
        head_version_hash: [2u8; 32],
        mmr_root: [3u8; 32],
        seq: 42,
    };
    let datum = map_anchor_to_datum(&a);
    // datum → parse → anchor' khớp bit 4 trường.
    assert_eq!(parse_datum_to_anchor(&datum).unwrap(), a);
    // CBOR round-trip nội bộ: from_cbor(to_cbor(datum)) == datum.
    let cbor = datum.to_cbor();
    assert_eq!(PlutusData::from_cbor(&cbor).unwrap(), datum);
    // datum bọc đúng CIP-68 Constr 0 [meta, version=1, extra].
    match &datum {
        PlutusData::Constr(0, f) if f.len() == 3 => {
            assert!(matches!(f[1], PlutusData::Int(1)));
        }
        _ => panic!("datum không đúng shape CIP-68"),
    }
    // seq lớn (u64) round-trip.
    let big = StrataAnchor { seq: u64::MAX, ..a };
    assert_eq!(
        parse_datum_to_anchor(&map_anchor_to_datum(&big))
            .unwrap()
            .seq,
        u64::MAX
    );
}

// ── #2 publish → resolve → datum khớp mmr_root/head_version_hash (mock) ───────
#[test]
fn s1_criteria_2_publish_resolve_matches_chain() {
    let (chain, _pol, _a) = chain_of(2);
    let anchor = chain.anchor(); // seq=1
    let sink = pinned_sink(MockMosaic::new());

    let receipt = sink.publish(&anchor, AnchorPriority::Immediate).unwrap();
    assert!(receipt.is_some());

    let resolved = sink.resolve(&anchor.ref_id).unwrap().unwrap();
    assert_eq!(resolved.mmr_root, chain.mmr_root());
    assert_eq!(resolved.head_version_hash, chain.head_version_hash());
    assert_eq!(resolved, anchor);
}

// ── #3 INV-E7 rollback: neo lại seq cũ bị chặn ───────────────────────────────
#[test]
fn s1_criteria_3_rollback_rejected() {
    let sink = pinned_sink(MockMosaic::new());
    let a1 = sample_anchor(1);
    sink.publish(&a1, AnchorPriority::Immediate)
        .unwrap()
        .unwrap();
    assert_eq!(sink.backend().tx_count(), 1);

    // neo lại seq=0 (cũ hơn) → RollbackAttempt, KHÔNG đẻ tx.
    let a0 = sample_anchor(0);
    let err = sink.publish(&a0, AnchorPriority::Immediate).unwrap_err();
    assert_eq!(
        err,
        AnchorError::RollbackAttempt {
            on_chain_seq: 1,
            attempted: 0
        }
    );
    assert!(!err.is_retryable());
    assert_eq!(sink.backend().tx_count(), 1);
}

// ── #4 idempotency: publish cùng seq hai lần → tx = 1 ─────────────────────────
#[test]
fn s1_criteria_4_idempotent_no_double_tx() {
    let sink = pinned_sink(MockMosaic::new());
    let a1 = sample_anchor(1);
    assert!(
        sink.publish(&a1, AnchorPriority::Immediate)
            .unwrap()
            .is_some()
    );
    // lần hai cùng seq → Ok(None), KHÔNG tạo tx mới.
    assert!(
        sink.publish(&a1, AnchorPriority::Immediate)
            .unwrap()
            .is_none()
    );
    assert_eq!(sink.backend().tx_count(), 1);

    // NoAnchor → luôn Ok(None), không đụng backend.
    assert!(
        sink.publish(&sample_anchor(2), AnchorPriority::NoAnchor)
            .unwrap()
            .is_none()
    );
    assert_eq!(sink.backend().tx_count(), 1);
}

// ── #4b Mosaic-A: nhảy bậc seq bị chặn TẠI SINK, không đẩy lên chuỗi ─────────
//
// Validator Plutus đang chạy ép `datum_out.seq == datum_in.seq + 1`
// (`VeDataIO/Code: mosaic/aiken/lib/strata/anchor.ak:55-57`). Trước bản vá này, sink đẩy
// thẳng một seq nhảy bậc lên chuỗi: tx bị từ chối, nhưng head local đã tiến ⇒ mọi lần neo
// sau đều nhảy bậc y hệt ⇒ lineage kẹt vĩnh viễn. Anh Đức chốt hướng B (2026-08-07): giữ
// luật on-chain, sửa tầng đẩy — nên sink phải fail cứng TRƯỚC khi dựng tx.
#[test]
fn seq_gap_rejected_before_building_tx() {
    let sink = pinned_sink(MockMosaic::new());

    // Neo bậc đầu (lineage chưa có gì on-chain) — hợp lệ, validator không guard CREATE.
    sink.publish(&sample_anchor(1), AnchorPriority::Immediate)
        .unwrap()
        .unwrap();
    assert_eq!(sink.backend().tx_count(), 1);

    // Nhảy từ 1 sang 3 → SeqGap, KHÔNG đẻ tx.
    let err = sink
        .publish(&sample_anchor(3), AnchorPriority::Immediate)
        .unwrap_err();
    assert_eq!(
        err,
        AnchorError::SeqGap {
            on_chain_seq: Some(1),
            expected: 2,
            attempted: 3,
        }
    );
    // Fail CỨNG: retry cùng seq vẫn hỏng y hệt, không được xếp vào diện retryable.
    assert!(!err.is_retryable());
    assert_eq!(sink.backend().tx_count(), 1);

    // Neo đúng bậc kế tiếp thì đi tiếp bình thường — bản vá không chặn đường hợp lệ.
    sink.publish(&sample_anchor(2), AnchorPriority::Immediate)
        .unwrap()
        .unwrap();
    assert_eq!(sink.backend().tx_count(), 2);
    sink.publish(&sample_anchor(3), AnchorPriority::Immediate)
        .unwrap()
        .unwrap();
    assert_eq!(sink.backend().tx_count(), 3);

    // NoAnchor vẫn thoát sớm, không đụng backend dù seq nhảy bậc.
    assert!(
        sink.publish(&sample_anchor(99), AnchorPriority::NoAnchor)
            .unwrap()
            .is_none()
    );
    assert_eq!(sink.backend().tx_count(), 3);
}

// ── #5 resolve sau append: verify version cũ dưới root ĐÃ NEO ─────────────────
#[test]
fn s1_criteria_5_resolve_after_append_verifies_old_root() {
    let (mut chain, pol, a) = chain_of(2); // seq 0,1
    let ph = pol.policy_hash();
    let sink = pinned_sink(MockMosaic::new());
    let mut table = AnchoredTable::new();

    // Neo seq=1 + ghi bảng daemon TẠI thời điểm neo.
    let anchor1 = chain.publish_anchor().unwrap();
    assert_eq!(anchor1.seq, 1);
    sink.publish(&anchor1, AnchorPriority::Immediate).unwrap();
    table.record_anchor(&chain, &anchor1).unwrap();

    // Append tới seq=5 (CHƯA neo) → MMR đổi.
    for seq in 2..=5 {
        let v = signed(seq, chain.head_version_hash(), 100 + seq, &a, ph);
        chain.append_version(v, &pol).unwrap();
    }
    assert_eq!(chain.head().seq, 5);

    // resolve vẫn trả seq=1; verify dưới root ĐÃ NEO (size cũ) PASS.
    let resolved = sink.resolve(&anchor1.ref_id).unwrap().unwrap();
    assert_eq!(resolved.seq, 1);
    assert_eq!(resolved.mmr_root, anchor1.mmr_root); // root cũ, KHÁC root hiện tại
    assert_ne!(resolved.mmr_root, chain.mmr_root());
    verify_resolved(&chain, &resolved, &table).expect("verify dưới root đã neo phải PASS");

    // seq=5 chưa neo → không có dòng trong bảng.
    assert!(table.get(&anchor1.ref_id, 5).is_none());
    assert_eq!(table.len(), 1);
}

// ── #6 DatumTooLarge / InsufficientAda: mock từ chối → đúng biến thể, KHÔNG panic ─
#[test]
fn s1_criteria_6_backend_errors_propagate() {
    let a1 = sample_anchor(1);

    let sink_big = pinned_sink(MockMosaic::failing(AnchorError::DatumTooLarge {
        bytes: 20_000,
    }));
    assert_eq!(
        sink_big
            .publish(&a1, AnchorPriority::Immediate)
            .unwrap_err(),
        AnchorError::DatumTooLarge { bytes: 20_000 }
    );

    let sink_ada = pinned_sink(MockMosaic::failing(AnchorError::InsufficientAda {
        need: 1_500_000,
        have: 900_000,
    }));
    let err = sink_ada
        .publish(&a1, AnchorPriority::Immediate)
        .unwrap_err();
    assert_eq!(
        err,
        AnchorError::InsufficientAda {
            need: 1_500_000,
            have: 900_000
        }
    );
    assert!(!err.is_retryable());

    // Network là retryable (phân tầng §8.1b).
    let sink_net = pinned_sink(MockMosaic::failing(AnchorError::Network("timeout".into())));
    assert!(
        sink_net
            .publish(&a1, AnchorPriority::Immediate)
            .unwrap_err()
            .is_retryable()
    );
}

// ── review #1: trust-model resolve — thread-token NFT one-shot chống đầu độc ───
#[test]
fn resolve_thread_token_rejects_poisoning() {
    let t = tok(0xAA);
    // Sink production: pin thread-token one-shot của lineage.
    let sink = MosaicAnchorSink::with_thread_token(MockMosaic::with_token(t.clone()), t.clone());
    let authentic = sample_anchor(1); // ref_id [7;32]
    sink.publish(&authentic, AnchorPriority::Immediate)
        .unwrap()
        .unwrap();

    // Kẻ lạ gửi UTxO datum GIẢ seq cao hơn (99) vào script address nhưng KHÔNG mint được
    // NFT one-shot → thread_token None (hoặc token sai).
    let forged_hi = map_anchor_to_datum(&sample_anchor(99));
    sink.backend().inject_forged(forged_hi.clone(), None);
    sink.backend().inject_forged(forged_hi, Some(tok(0xBB))); // token sai

    // resolve PHẢI trả seq=1 (authentic), KHÔNG bị đầu độc bởi seq=99.
    let got = sink.resolve(&authentic.ref_id).unwrap().unwrap();
    assert_eq!(got.seq, 1, "chỉ tin UTxO mang đúng thread-token one-shot");
}

// ── review #2: datum rác → BỎ QUA quét tiếp (không Err → chống DoS) ───────────
#[test]
fn resolve_skips_garbage_datum_no_error() {
    // Chế độ không pin token (round-trip) — cô lập hành vi bỏ-qua-rác.
    let sink = pinned_sink(MockMosaic::new());
    let authentic = sample_anchor(3);
    sink.publish(&authentic, AnchorPriority::Immediate)
        .unwrap()
        .unwrap();

    // Kẻ lạ chèn datum RÁC (shape sai) — resolve phải BỎ QUA, không Err(Rejected).
    let garbage = PlutusData::Int(12345);
    sink.backend().inject_forged(garbage.clone(), None);
    let got = sink.resolve(&authentic.ref_id).unwrap();
    assert_eq!(got.unwrap().seq, 3, "bỏ qua datum rác, lấy anchor hợp lệ");

    // Chỉ có datum rác (không authentic) → Ok(None), KHÔNG Err (chống DoS 1-tx).
    let sink2 = pinned_sink(MockMosaic::new());
    sink2.backend().inject_forged(garbage, None);
    assert_eq!(sink2.resolve(&[7u8; 32]).unwrap(), None);
}

// ── review #3 (vòng 2): anchor THẬT (đúng thread-token) datum hỏng → WARN + bỏ qua; kẻ lạ
//    (token sai/None) vẫn im lặng. resolve KHÔNG Err; anchor hợp lệ khác vẫn lấy được. ──────
#[test]
fn resolve_warns_on_authenticated_corrupt_datum() {
    let t = tok(0xAA);
    let sink = MosaicAnchorSink::with_thread_token(MockMosaic::with_token(t.clone()), t.clone());
    let authentic = sample_anchor(1);
    sink.publish(&authentic, AnchorPriority::Immediate)
        .unwrap()
        .unwrap();

    // UTxO mang ĐÚNG thread-token nhưng datum hỏng (shape sai) → nhánh WARN (authenticated).
    sink.backend()
        .inject_forged(PlutusData::Int(999), Some(t.clone()));
    // Kẻ lạ: datum hỏng + token None → bỏ qua IM LẶNG (không WARN).
    sink.backend().inject_forged(PlutusData::Int(111), None);

    // resolve vẫn trả anchor hợp lệ seq=1, KHÔNG Err (datum hỏng bỏ qua dù đã WARN).
    let got = sink.resolve(&authentic.ref_id).unwrap().unwrap();
    assert_eq!(got.seq, 1);

    // Chỉ có anchor-thật-hỏng (đúng token), không có datum hợp lệ → Ok(None), vẫn KHÔNG Err.
    let sink2 = MosaicAnchorSink::with_thread_token(MockMosaic::with_token(t.clone()), t.clone());
    sink2.backend().inject_forged(PlutusData::Int(999), Some(t));
    assert_eq!(sink2.resolve(&[7u8; 32]).unwrap(), None);
}

// ── review #3: publish_with_retry — chỉ Network, backoff mũ, max_attempts ─────
#[test]
fn publish_with_retry_only_network_exp_backoff() {
    let a1 = sample_anchor(1);

    // Network liên tục → retry đủ max_attempts=3 rồi trả Err; backoff mũ 10,20.
    let sink = pinned_sink(MockMosaic::failing(AnchorError::Network("t".into())));
    let mut sleeps = Vec::new();
    let err = sink
        .publish_with_retry(&a1, AnchorPriority::Immediate, 3, 10, |ms| sleeps.push(ms))
        .unwrap_err();
    assert!(err.is_retryable());
    assert_eq!(sleeps, vec![10, 20], "3 lần gọi = 2 lần chờ, backoff mũ");

    // Lỗi KHÔNG retryable → trả ngay, 0 lần chờ.
    let sink_rej = pinned_sink(MockMosaic::failing(AnchorError::DatumTooLarge {
        bytes: 99,
    }));
    let mut s2 = Vec::new();
    assert!(
        sink_rej
            .publish_with_retry(&a1, AnchorPriority::Immediate, 5, 10, |ms| s2.push(ms))
            .is_err()
    );
    assert!(s2.is_empty(), "lỗi cứng không retry");

    // Thành công ngay → không chờ.
    let sink_ok = pinned_sink(MockMosaic::new());
    let mut s3 = Vec::new();
    assert!(
        sink_ok
            .publish_with_retry(&a1, AnchorPriority::Immediate, 5, 10, |ms| s3.push(ms))
            .unwrap()
            .is_some()
    );
    assert!(s3.is_empty());
}

// ── review #4: AnchoredTable — key (ref_id,seq), reject overwrite, save/load ──
#[test]
fn anchored_table_multichain_key_and_persist() {
    let (chain, _pol, _a) = chain_of(2); // seq 0,1
    let r1 = chain.anchor().ref_id;
    let r2 = [0x55u8; 32]; // ref_id thứ hai (đa-chain)
    let mut table = AnchoredTable::new();

    // Ghi (r1,1) và (r2,1) — cùng seq, khác ref_id → hai dòng riêng.
    let a_r1 = chain.anchor(); // ref_id=r1, seq=1
    let a_r2 = StrataAnchor {
        ref_id: r2,
        ..chain.anchor()
    };
    table.record_anchor(&chain, &a_r1).unwrap();
    table.record_anchor(&chain, &a_r2).unwrap();
    assert_eq!(table.len(), 2);
    assert!(table.get(&r1, 1).is_some());
    assert!(table.get(&r2, 1).is_some());

    // Idempotent: ghi lại y hệt = OK, không nhân dòng.
    table.record_anchor(&chain, &a_r1).unwrap();
    assert_eq!(table.len(), 2);

    // Ghi đè (r1,1) bằng mmr_root KHÁC → ConflictingOverwrite.
    let conflicting = StrataAnchor {
        mmr_root: [0xEEu8; 32],
        ..a_r1.clone()
    };
    assert_eq!(
        table.record_anchor(&chain, &conflicting),
        Err(TableError::ConflictingOverwrite { seq: 1 })
    );
    assert_eq!(table.len(), 2, "conflict không ghi");

    // save/load round-trip byte-chính-xác.
    let bytes = table.to_bytes();
    assert_eq!(AnchoredTable::from_bytes(&bytes).unwrap(), table);

    // parse strict: cụt / thừa byte → None.
    assert!(AnchoredTable::from_bytes(&bytes[..bytes.len() - 1]).is_none());
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(AnchoredTable::from_bytes(&extra).is_none());
    assert!(AnchoredTable::from_bytes(&[0, 0]).is_none());
}

// ── review #4b: verify_resolved tái dựng proof ở size cũ (không lưu proof) ────
#[test]
fn verify_resolved_rebuilds_proof_no_stored_proof() {
    let (mut chain, pol, a) = chain_of(2);
    let ph = pol.policy_hash();
    let sink = pinned_sink(MockMosaic::new());
    let mut table = AnchoredTable::new();

    let anchor1 = chain.publish_anchor().unwrap();
    sink.publish(&anchor1, AnchorPriority::Immediate).unwrap();
    // record SAU khi append thêm nhiều version — không còn ràng buộc "record TRƯỚC append".
    for seq in 2..=6 {
        let v = signed(seq, chain.head_version_hash(), 100 + seq, &a, ph);
        chain.append_version(v, &pol).unwrap();
    }
    table.record_anchor(&chain, &anchor1).unwrap(); // ghi muộn vẫn đúng

    let resolved = sink.resolve(&anchor1.ref_id).unwrap().unwrap();
    verify_resolved(&chain, &resolved, &table)
        .expect("verify tái dựng proof ở size cũ phải PASS dù record muộn");
}

// ── Ba mục review PR #42 — bài kiểm cho từng mục ─────────────────────────────

/// **Mục 1.** Đường GHI đi qua cửa đã xác thực: một UTxO cấy vào địa chỉ script mang
/// `seq = 2^63` KHÔNG còn kẹt được lineage.
///
/// Trước bản vá, `publish` hỏi `on_chain_seq()` — một số vô hướng, sink không còn gì để lọc
/// — nên `seq` của kẻ lạ trở thành `on_chain_seq` và mọi lần neo sau đều ăn `RollbackAttempt`
/// vĩnh viễn. Nay `publish` đọc qua `resolve()`, vốn loại UTxO không mang thread-token.
#[test]
fn utxo_cay_seq_khong_lo_khong_con_ket_duong_ghi() {
    let t = tok(7);
    let mock = MockMosaic::with_token(t.clone());
    // Kẻ lạ gửi UTxO datum giả: cùng ref_id, seq = 2^63, KHÔNG mang thread-token.
    mock.inject_forged(map_anchor_to_datum(&sample_anchor(u64::MAX / 2)), None);
    let sink = MosaicAnchorSink::with_thread_token(mock, t);

    // Lineage chưa neo lần nào (không UTxO nào qua được cửa xác thực) ⇒ neo đầu đi lọt.
    let r = sink.publish(&sample_anchor(0), AnchorPriority::Immediate);
    assert!(
        r.is_ok(),
        "UTxO cấy vào phải bị loại, không được kẹt đường ghi: {r:?}"
    );
}

/// **Mục 2.** Chế độ không-xác-thực **fail-đóng ở đường GHI**, nhưng vẫn ĐỌC được.
///
/// Không pin token thì `resolve()` nhận mọi ứng viên, tức không có gác nào — trả một kết quả
/// trông-như-đúng là chỗ hỏng im lặng. Đọc thì vẫn cho, vì đọc không thay đổi trạng thái.
#[test]
fn khong_pin_token_thi_chan_ghi_nhung_van_cho_doc() {
    let sink = MosaicAnchorSink::new_unverified_for_tests(MockMosaic::new());
    let err = sink
        .publish(&sample_anchor(0), AnchorPriority::Immediate)
        .expect_err("đường ghi phải fail-đóng khi không pin thread-token");
    assert!(
        matches!(&err, AnchorError::Rejected(m) if m.contains("with_thread_token")),
        "thông điệp phải chỉ thẳng cách sửa, nhận được: {err:?}"
    );
    assert!(sink.resolve(&[1u8; 32]).is_ok(), "đường đọc không bị chặn");
}

/// **Mục 3.** `AnchorPriority` thưa + Mosaic bị chặn NGAY, kèm lý do đúng.
///
/// Không chặn thì nó không hỏng im lặng — nó hỏng *ồn ào nhưng sai hướng*: `SeqGap` kèm câu
/// "on-chain đòi neo seq=N trước", đọc như gọi sai thứ tự trong khi thực chất là sai cấu hình.
#[test]
fn priority_thua_bi_chan_voi_ly_do_dung() {
    let t = tok(9);
    let sink = MosaicAnchorSink::with_thread_token(MockMosaic::with_token(t.clone()), t);
    for p in [AnchorPriority::Milestone, AnchorPriority::BatchDaily] {
        let err = sink
            .publish(&sample_anchor(0), p)
            .expect_err("priority thưa phải bị chặn ở Mosaic-A");
        assert!(
            matches!(&err, AnchorError::Rejected(m) if m.contains("Settlement")),
            "phải chỉ sang backend đúng, nhận được: {err:?}"
        );
    }
}
