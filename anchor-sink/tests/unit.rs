//! Unit test S1 AnchorSink (mock) — 6 tiêu chí §8.1 + trust filter + retry policy.
//!
//! KHÔNG chạm mạng: `ChainQuery`/`Submitter` mock chia sẻ một "on-chain state" giả
//! qua `Rc<RefCell<..>>` để publish → resolve nhìn thấy nhau như Blockfrost thật.

use lampnet_anchor_sink::{
    AnchorError, AnchorPriority, AnchorRecord, AnchorSink, AnchoredLog, ChainQuery,
    SettlementSink, SinkConfig, SubmitOutcome, Submitter, VerifyError, decode_records,
    encode_records, publish_with_retry, verify_anchored,
};
use lampnet_strata::{
    Hash32, Policy, StrataAnchor, StrataChain, StrataVersion, build_state_root,
};

use ed25519_dalek::SigningKey;
use rand::rngs::OsRng;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

const PUBLISHER: &str = "addr_test1_publisher_pinned";
const STRANGER: &str = "addr_test1_stranger_wallet";

// ---------------------------------------------------------------------------
// Mock on-chain state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct MockTx {
    txid: String,
    input_addrs: Vec<String>,
    /// (label, metadatum CBOR)
    metadata: Option<(u64, Vec<u8>)>,
}

#[derive(Default)]
struct MockChainState {
    txs: Vec<MockTx>, // cũ → mới; query trả đảo (mới → cũ)
    submit_count: usize,
}

impl MockChainState {
    fn push_tx(&mut self, from: &str, label: u64, cbor: Vec<u8>) -> String {
        let txid = format!("mock_tx_{}", self.txs.len());
        self.txs.push(MockTx {
            txid: txid.clone(),
            input_addrs: vec![from.to_string()],
            metadata: Some((label, cbor)),
        });
        txid
    }
}

#[derive(Clone)]
struct MockQuery(Rc<RefCell<MockChainState>>);

impl ChainQuery for MockQuery {
    fn address_txs(&self, addr: &str, limit: usize) -> Result<Vec<String>, AnchorError> {
        // Như Blockfrost: mọi tx LIÊN QUAN địa chỉ (input HOẶC output). Mock đơn
        // giản: trả tất cả tx (test stranger sẽ chứng minh filter input hoạt động).
        let _ = addr;
        Ok(self
            .0
            .borrow()
            .txs
            .iter()
            .rev()
            .take(limit)
            .map(|t| t.txid.clone())
            .collect())
    }

    fn tx_input_addresses(&self, txid: &str) -> Result<Vec<String>, AnchorError> {
        Ok(self
            .0
            .borrow()
            .txs
            .iter()
            .find(|t| t.txid == txid)
            .map(|t| t.input_addrs.clone())
            .unwrap_or_default())
    }

    fn tx_metadata_cbor(&self, txid: &str, label: u64) -> Result<Option<Vec<u8>>, AnchorError> {
        Ok(self.0.borrow().txs.iter().find(|t| t.txid == txid).and_then(|t| {
            t.metadata
                .as_ref()
                .filter(|(l, _)| *l == label)
                .map(|(_, c)| c.clone())
        }))
    }
}

/// Mock submitter: ghi tx vào state giả (ví = PUBLISHER), hoặc trả lỗi cấu hình sẵn.
struct MockSubmitter {
    state: Rc<RefCell<MockChainState>>,
    fail_with: RefCell<Vec<AnchorError>>, // pop lần lượt; hết → thành công
    wallet: String,
}

impl MockSubmitter {
    fn ok(state: Rc<RefCell<MockChainState>>) -> Self {
        Self {
            state,
            fail_with: RefCell::new(Vec::new()),
            wallet: PUBLISHER.to_string(),
        }
    }
    fn failing(state: Rc<RefCell<MockChainState>>, errs: Vec<AnchorError>) -> Self {
        Self {
            state,
            fail_with: RefCell::new(errs),
            wallet: PUBLISHER.to_string(),
        }
    }
}

impl Submitter for MockSubmitter {
    fn submit(&self, records: &[AnchorRecord]) -> Result<SubmitOutcome, AnchorError> {
        if let Some(e) = self.fail_with.borrow_mut().pop() {
            return Err(e);
        }
        let cbor = encode_records(records);
        let mut st = self.state.borrow_mut();
        st.submit_count += 1;
        let txid = st.push_tx(&self.wallet, 1234, cbor);
        Ok(SubmitOutcome {
            txid,
            address: self.wallet.clone(),
        })
    }
}

fn mk_sink(
    state: &Rc<RefCell<MockChainState>>,
) -> SettlementSink<MockQuery, MockSubmitter> {
    let cfg = SinkConfig {
        publisher_address: PUBLISHER.to_string(),
        ..SinkConfig::default()
    };
    SettlementSink::new(cfg, MockQuery(state.clone()), MockSubmitter::ok(state.clone()))
}

// ---------------------------------------------------------------------------
// Chain local thật (lampnet-strata) cho test verify ngược
// ---------------------------------------------------------------------------

struct TestChain {
    chain: StrataChain,
    policy: Policy,
    sk: SigningKey,
    did: [u8; 32],
}

fn signed_version(
    seq: u64,
    prev: Hash32,
    ts: u64,
    did: [u8; 32],
    sk: &SigningKey,
    ph: Hash32,
) -> StrataVersion {
    let sr = build_state_root(&[(b"k".to_vec(), format!("v{seq}").into_bytes())]);
    let mut v = StrataVersion::unsigned(seq, prev, b"cid".to_vec(), sr, did, ph, ts);
    v.sign(sk);
    v
}

/// Chain thật: genesis + `extra` version append.
fn build_chain(ref_id: Hash32, extra: u64) -> TestChain {
    let sk = SigningKey::generate(&mut OsRng);
    let did = [0x77; 32];
    let mut policy = Policy::new();
    policy.allow(did, sk.verifying_key());
    let ph = policy.policy_hash();
    let v0 = signed_version(0, [0u8; 32], 100, did, &sk, ph);
    let mut chain = StrataChain::genesis(ref_id, v0, &policy).unwrap();
    for i in 1..=extra {
        let v = signed_version(i, chain.head_version_hash(), 100 + i * 10, did, &sk, ph);
        chain.append_version(v, &policy).unwrap();
    }
    TestChain { chain, policy, sk, did }
}

impl TestChain {
    fn append_more(&mut self, n: u64) {
        let ph = self.policy.policy_hash();
        for _ in 0..n {
            let next = self.chain.head().seq + 1;
            let v = signed_version(
                next,
                self.chain.head_version_hash(),
                100 + next * 10,
                self.did,
                &self.sk,
                ph,
            );
            self.chain.append_version(v, &self.policy).unwrap();
        }
    }
}

// ---------------------------------------------------------------------------
// Tiêu chí 1 — round-trip anchor → payload → parse khớp bit
// ---------------------------------------------------------------------------

#[test]
fn t1_round_trip_payload_bit_exact() {
    let tc = build_chain([0xA1; 32], 2);
    let anchor = tc.chain.anchor();
    let cbor = encode_records(&[AnchorRecord::Anchor(anchor.clone())]);
    let parsed = decode_records(&cbor).unwrap();
    assert_eq!(parsed, vec![AnchorRecord::Anchor(anchor.clone())]);
    let AnchorRecord::Anchor(a2) = &parsed[0] else { unreachable!() };
    // 4 trường khớp bit.
    assert_eq!(a2.ref_id, anchor.ref_id);
    assert_eq!(a2.head_version_hash, anchor.head_version_hash);
    assert_eq!(a2.mmr_root, anchor.mmr_root);
    assert_eq!(a2.seq, anchor.seq);
    // encode lại → cùng bytes (deterministic).
    assert_eq!(cbor, encode_records(&parsed));
}

// ---------------------------------------------------------------------------
// Tiêu chí 2 — idempotent: publish 2 lần → lần 2 Ok(None), KHÔNG tx mới
// ---------------------------------------------------------------------------

#[test]
fn t2_idempotent_publish_twice_second_is_noop() {
    let state = Rc::new(RefCell::new(MockChainState::default()));
    let sink = mk_sink(&state);
    let mut tc = build_chain([0xB2; 32], 1);
    let anchor = tc.chain.publish_anchor().unwrap(); // seq=1, INV-E7 lớp core

    let r1 = sink.publish(&anchor, AnchorPriority::Milestone).unwrap();
    assert!(r1.is_some(), "lần 1 phải tạo tx");
    assert_eq!(state.borrow().submit_count, 1);

    // Retry cùng anchor (giả lập sau lỗi Network mà tx đã lên).
    let r2 = sink.publish(&anchor, AnchorPriority::Milestone).unwrap();
    assert_eq!(r2, None, "lần 2 phải là idempotent no-op");
    assert_eq!(state.borrow().submit_count, 1, "KHÔNG được tạo tx thứ hai");
}

// ---------------------------------------------------------------------------
// Tiêu chí 3 — rollback: publish seq THẤP hơn on-chain → RollbackAttempt,
//              bị chặn TRƯỚC khi build tx
// ---------------------------------------------------------------------------

#[test]
fn t3_rollback_lower_seq_rejected_before_build() {
    let state = Rc::new(RefCell::new(MockChainState::default()));
    let sink = mk_sink(&state);
    let mut tc = build_chain([0xC3; 32], 3);
    let anchor3 = tc.chain.publish_anchor().unwrap(); // seq=3
    sink.publish(&anchor3, AnchorPriority::Milestone).unwrap();
    assert_eq!(state.borrow().submit_count, 1);

    // Kẻ tấn công / process khác cố neo lại anchor CŨ (seq=1).
    let stale = StrataAnchor {
        ref_id: [0xC3; 32],
        head_version_hash: tc.chain.version(1).unwrap().version_hash(),
        mmr_root: [0xEE; 32], // root cũ nào đó
        seq: 1,
    };
    let err = sink.publish(&stale, AnchorPriority::Milestone).unwrap_err();
    assert_eq!(
        err,
        AnchorError::RollbackAttempt { on_chain_seq: 3, attempted: 1 }
    );
    assert_eq!(
        state.borrow().submit_count,
        1,
        "rollback phải bị chặn TRƯỚC khi build/submit tx"
    );
}

// ---------------------------------------------------------------------------
// Tiêu chí 4 — resolve sau khi local append tiếp: vẫn trả seq ĐÃ NEO;
//              verify ngược §8.1c dưới root ĐÃ NEO với size CŨ
// ---------------------------------------------------------------------------

#[test]
fn t4_resolve_returns_anchored_seq_after_local_appends_and_back_verify() {
    let state = Rc::new(RefCell::new(MockChainState::default()));
    let sink = mk_sink(&state);
    let mut tc = build_chain([0xD4; 32], 1);
    let anchor1 = tc.chain.publish_anchor().unwrap(); // seq=1, size=2

    // Daemon lưu bảng anchored (§8.1c) lúc publish.
    let mut log = AnchoredLog::new();
    assert!(log.record(anchor1.ref_id, anchor1.seq, anchor1.mmr_root, tc.chain.len() as u64));

    sink.publish(&anchor1, AnchorPriority::BatchDaily).unwrap();

    // Local tiến tới seq=5 (CHƯA neo).
    tc.append_more(4);
    assert_eq!(tc.chain.head().seq, 5);

    // resolve vẫn trả anchor đã neo (seq=1) — on-chain không tự tiến.
    let on_chain = sink.resolve(&anchor1.ref_id).unwrap().unwrap();
    assert_eq!(on_chain, anchor1);
    assert_eq!(on_chain.seq, 1);

    // Verify ngược: proof version seq=1 dưới mmr_root ĐÃ NEO (size cũ = 2) PASS,
    // dù local MMR giờ size 6, root khác.
    assert_ne!(tc.chain.mmr_root(), on_chain.mmr_root, "root local đã tiến");
    verify_anchored(&tc.chain, &on_chain, &log).unwrap();

    // ref_id chưa từng neo → None.
    assert_eq!(sink.resolve(&[0x00; 32]).unwrap(), None);
}

#[test]
fn t4b_back_verify_negative_cases() {
    let mut tc = build_chain([0xD5; 32], 2);
    let anchor = tc.chain.publish_anchor().unwrap(); // seq=2, size=3
    let mut log = AnchoredLog::new();
    log.record(anchor.ref_id, anchor.seq, anchor.mmr_root, 3);
    tc.append_more(2);

    // PASS chuẩn.
    verify_anchored(&tc.chain, &anchor, &log).unwrap();

    // ref_id lệch.
    let mut bad = anchor.clone();
    bad.ref_id = [0x00; 32];
    assert_eq!(verify_anchored(&tc.chain, &bad, &log), Err(VerifyError::RefIdMismatch));

    // on-chain đi trước local → LocalBehind (local stale).
    let mut ahead = anchor.clone();
    ahead.seq = 99;
    assert!(matches!(
        verify_anchored(&tc.chain, &ahead, &log),
        Err(VerifyError::LocalBehind { on_chain_seq: 99, .. })
    ));

    // root on-chain giả (khác log) → AnchoredRootMismatch.
    let mut forged = anchor.clone();
    forged.mmr_root = [0xFF; 32];
    assert_eq!(
        verify_anchored(&tc.chain, &forged, &log),
        Err(VerifyError::AnchoredRootMismatch)
    );

    // head_version_hash giả → HeadHashMismatch.
    let mut wrong_head = anchor.clone();
    wrong_head.head_version_hash = [0xAB; 32];
    assert_eq!(
        verify_anchored(&tc.chain, &wrong_head, &log),
        Err(VerifyError::HeadHashMismatch)
    );

    // Thiếu dòng AnchoredLog → NotInAnchoredLog.
    let empty = AnchoredLog::new();
    assert_eq!(
        verify_anchored(&tc.chain, &anchor, &empty),
        Err(VerifyError::NotInAnchoredLog { seq: 2 })
    );

    // Log bị ghi size sai kèm root sai-mà-khớp-nhau → ProofInvalid (proof không dựng lại được).
    let mut bad_log = AnchoredLog::new();
    bad_log.record(anchor.ref_id, anchor.seq, anchor.mmr_root, 2); // size thật là 3
    assert_eq!(
        verify_anchored(&tc.chain, &anchor, &bad_log),
        Err(VerifyError::ProofInvalid)
    );
}

// ---------------------------------------------------------------------------
// Tiêu chí 5 — DatumTooLarge / InsufficientAda mock → đúng biến thể, không panic
// ---------------------------------------------------------------------------

#[test]
fn t5_backend_error_variants_propagate() {
    let state = Rc::new(RefCell::new(MockChainState::default()));
    let tc = build_chain([0xE5; 32], 1);
    let anchor = tc.chain.anchor();

    // InsufficientAda từ backend.
    let sink = SettlementSink::new(
        SinkConfig { publisher_address: PUBLISHER.into(), ..SinkConfig::default() },
        MockQuery(state.clone()),
        MockSubmitter::failing(
            state.clone(),
            vec![AnchorError::InsufficientAda { need: 2_000_000, have: 500_000 }],
        ),
    );
    assert_eq!(
        sink.publish(&anchor, AnchorPriority::Milestone).unwrap_err(),
        AnchorError::InsufficientAda { need: 2_000_000, have: 500_000 }
    );

    // DatumTooLarge từ backend.
    let sink = SettlementSink::new(
        SinkConfig { publisher_address: PUBLISHER.into(), ..SinkConfig::default() },
        MockQuery(state.clone()),
        MockSubmitter::failing(state.clone(), vec![AnchorError::DatumTooLarge { bytes: 20_000 }]),
    );
    assert_eq!(
        sink.publish(&anchor, AnchorPriority::Milestone).unwrap_err(),
        AnchorError::DatumTooLarge { bytes: 20_000 }
    );

    // DatumTooLarge phát hiện LOCAL (trước submit) khi metadatum vượt trần config.
    let sink = SettlementSink::new(
        SinkConfig {
            publisher_address: PUBLISHER.into(),
            max_metadatum_bytes: 10, // trần bé giả lập
            ..SinkConfig::default()
        },
        MockQuery(state.clone()),
        MockSubmitter::ok(state.clone()),
    );
    let err = sink.publish(&anchor, AnchorPriority::Milestone).unwrap_err();
    assert!(matches!(err, AnchorError::DatumTooLarge { bytes } if bytes > 10));
    assert_eq!(state.borrow().submit_count, 0, "không được submit khi quá trần");
}

// ---------------------------------------------------------------------------
// Tiêu chí 6 — anchor giả từ ví lạ bị resolve BỎ QUA (trust pin publisher)
// ---------------------------------------------------------------------------

#[test]
fn t6_forged_anchor_from_stranger_wallet_ignored() {
    let state = Rc::new(RefCell::new(MockChainState::default()));
    let sink = mk_sink(&state);
    let mut tc = build_chain([0xF6; 32], 1);
    let real = tc.chain.publish_anchor().unwrap(); // seq=1
    sink.publish(&real, AnchorPriority::Milestone).unwrap();

    // Kẻ lạ đúc anchor GIẢ seq=999 cùng ref_id, label 1234, từ VÍ LẠ.
    let forged = StrataAnchor {
        ref_id: real.ref_id,
        head_version_hash: [0xBA; 32],
        mmr_root: [0xDD; 32],
        seq: 999,
    };
    let cbor = encode_records(&[AnchorRecord::Anchor(forged)]);
    state.borrow_mut().push_tx(STRANGER, 1234, cbor);

    // resolve CHỈ tin publisher → vẫn trả anchor thật seq=1.
    let got = sink.resolve(&real.ref_id).unwrap().unwrap();
    assert_eq!(got, real, "anchor giả từ ví lạ phải bị bỏ qua");

    // Và publish tiếp seq=2 KHÔNG bị anchor giả seq=999 chặn nhầm (RollbackAttempt sai).
    tc.append_more(1);
    let a2 = tc.chain.publish_anchor().unwrap(); // seq=2
    let r = sink.publish(&a2, AnchorPriority::Milestone).unwrap();
    assert!(r.is_some(), "anchor giả không được đầu độc idempotency check");
}

#[test]
fn t6b_stranger_tx_paying_to_publisher_still_ignored() {
    // Tinh vi hơn: tx của kẻ lạ GỬI TIỀN TỚI publisher (nên lọt address_txs của
    // Blockfrost) nhưng input là ví lạ → vẫn phải bị bỏ qua.
    let state = Rc::new(RefCell::new(MockChainState::default()));
    let sink = mk_sink(&state);

    let ref_id = [0xF7; 32];
    let forged = StrataAnchor {
        ref_id,
        head_version_hash: [0x01; 32],
        mmr_root: [0x02; 32],
        seq: 42,
    };
    let cbor = encode_records(&[AnchorRecord::Anchor(forged)]);
    state.borrow_mut().push_tx(STRANGER, 1234, cbor); // mock trả mọi tx qua address_txs

    assert_eq!(sink.resolve(&ref_id).unwrap(), None, "chưa từng có anchor thật");
}

// ---------------------------------------------------------------------------
// Retry policy §8.1b — chỉ Network retry; NoAnchor → Ok(None)
// ---------------------------------------------------------------------------

#[test]
fn retry_only_network_errors() {
    let state = Rc::new(RefCell::new(MockChainState::default()));
    let tc = build_chain([0xAA; 32], 1);
    let anchor = tc.chain.anchor();

    // 2 lần Network rồi thành công → publish_with_retry pass ở lần 3.
    let sink = SettlementSink::new(
        SinkConfig { publisher_address: PUBLISHER.into(), ..SinkConfig::default() },
        MockQuery(state.clone()),
        MockSubmitter::failing(
            state.clone(),
            vec![
                AnchorError::Network("timeout 2".into()),
                AnchorError::Network("timeout 1".into()),
            ],
        ),
    );
    let r = publish_with_retry(
        &sink,
        &anchor,
        AnchorPriority::Milestone,
        5,
        Duration::from_millis(1),
    )
    .unwrap();
    assert!(r.is_some());
    assert_eq!(state.borrow().submit_count, 1);

    // Rejected → KHÔNG retry (fail ngay lần 1).
    let state2 = Rc::new(RefCell::new(MockChainState::default()));
    let sink2 = SettlementSink::new(
        SinkConfig { publisher_address: PUBLISHER.into(), ..SinkConfig::default() },
        MockQuery(state2.clone()),
        MockSubmitter::failing(
            state2.clone(),
            vec![
                AnchorError::Network("không được chạm tới".into()),
                AnchorError::Rejected("validator từ chối".into()),
            ],
        ),
    );
    let err = publish_with_retry(
        &sink2,
        &anchor,
        AnchorPriority::Milestone,
        5,
        Duration::from_millis(1),
    )
    .unwrap_err();
    assert_eq!(err, AnchorError::Rejected("validator từ chối".into()));
    assert_eq!(state2.borrow().submit_count, 0);
}

#[test]
fn no_anchor_priority_is_noop() {
    let state = Rc::new(RefCell::new(MockChainState::default()));
    let sink = mk_sink(&state);
    let tc = build_chain([0xAB; 32], 0);
    let r = sink.publish(&tc.chain.anchor(), AnchorPriority::NoAnchor).unwrap();
    assert_eq!(r, None);
    assert_eq!(state.borrow().submit_count, 0);
}

#[test]
fn not_configured_when_publisher_missing() {
    let state = Rc::new(RefCell::new(MockChainState::default()));
    let sink = SettlementSink::new(
        SinkConfig::default(), // publisher rỗng
        MockQuery(state.clone()),
        MockSubmitter::ok(state.clone()),
    );
    let tc = build_chain([0xAC; 32], 0);
    assert_eq!(
        sink.publish(&tc.chain.anchor(), AnchorPriority::Milestone).unwrap_err(),
        AnchorError::NotConfigured
    );
    assert_eq!(sink.resolve(&[0xAC; 32]).unwrap_err(), AnchorError::NotConfigured);
}

#[test]
fn submitter_wallet_mismatch_rejected() {
    // Ví submitter KHÔNG khớp publisher pin → Rejected (anchor sẽ vô hình với resolve).
    let state = Rc::new(RefCell::new(MockChainState::default()));
    let mut wrong = MockSubmitter::ok(state.clone());
    wrong.wallet = STRANGER.to_string();
    let sink = SettlementSink::new(
        SinkConfig { publisher_address: PUBLISHER.into(), ..SinkConfig::default() },
        MockQuery(state.clone()),
        wrong,
    );
    let tc = build_chain([0xAD; 32], 0);
    assert!(matches!(
        sink.publish(&tc.chain.anchor(), AnchorPriority::Milestone),
        Err(AnchorError::Rejected(_))
    ));
}

// ---------------------------------------------------------------------------
// Batch nhiều chain trong 1 tx
// ---------------------------------------------------------------------------

#[test]
fn batch_multiple_chains_one_tx() {
    let state = Rc::new(RefCell::new(MockChainState::default()));
    let sink = mk_sink(&state);
    let mut tc1 = build_chain([0x01; 32], 1);
    let mut tc2 = build_chain([0x02; 32], 2);
    let a1 = tc1.chain.publish_anchor().unwrap();
    let a2 = tc2.chain.publish_anchor().unwrap();

    let r = sink.publish_batch(&[a1.clone(), a2.clone()]).unwrap();
    assert!(r.is_some());
    assert_eq!(state.borrow().submit_count, 1, "2 anchor phải gộp 1 tx");

    assert_eq!(sink.resolve(&a1.ref_id).unwrap().unwrap(), a1);
    assert_eq!(sink.resolve(&a2.ref_id).unwrap().unwrap(), a2);

    // Batch lại: a1 đã neo (loại), tc2 tiến thêm → chỉ a2' vào tx mới.
    tc2.append_more(1);
    let a2b = tc2.chain.publish_anchor().unwrap();
    let r2 = sink.publish_batch(&[a1.clone(), a2b.clone()]).unwrap();
    assert!(r2.is_some());
    assert_eq!(state.borrow().submit_count, 2);
    assert_eq!(sink.resolve(&a2.ref_id).unwrap().unwrap(), a2b);
    // Batch toàn anchor đã neo → Ok(None), không tx.
    let r3 = sink.publish_batch(&[a1, a2b]).unwrap();
    assert_eq!(r3, None);
    assert_eq!(state.borrow().submit_count, 2);
}
