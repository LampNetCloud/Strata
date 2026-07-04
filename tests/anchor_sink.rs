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
    MosaicAnchorSink, MosaicBackend, PlutusData, map_anchor_to_datum, parse_datum_to_anchor,
    verify_resolved,
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

// ── mock backend Mosaic (in-memory on-chain state) ───────────────────────────
struct MockMosaic {
    state: RefCell<HashMap<[u8; 32], PlutusData>>, // ref_id → datum mới nhất
    tx_count: RefCell<usize>,
    fail_submit: Option<AnchorError>, // nếu set → submit trả lỗi này
}
impl MockMosaic {
    fn new() -> Self {
        Self {
            state: RefCell::new(HashMap::new()),
            tx_count: RefCell::new(0),
            fail_submit: None,
        }
    }
    fn failing(e: AnchorError) -> Self {
        Self {
            fail_submit: Some(e),
            ..Self::new()
        }
    }
    fn tx_count(&self) -> usize {
        *self.tx_count.borrow()
    }
}
impl MosaicBackend for MockMosaic {
    fn on_chain_seq(&self, ref_id: &[u8; 32]) -> Result<Option<u64>, AnchorError> {
        Ok(self
            .state
            .borrow()
            .get(ref_id)
            .map(|d| parse_datum_to_anchor(d).unwrap().seq))
    }
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
    fn read_anchor(&self, ref_id: &[u8; 32]) -> Result<Option<PlutusData>, AnchorError> {
        Ok(self.state.borrow().get(ref_id).cloned())
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
    let sink = MosaicAnchorSink::new(MockMosaic::new());

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
    let sink = MosaicAnchorSink::new(MockMosaic::new());
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
    let sink = MosaicAnchorSink::new(MockMosaic::new());
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

// ── #5 resolve sau append: verify version cũ dưới root ĐÃ NEO ─────────────────
#[test]
fn s1_criteria_5_resolve_after_append_verifies_old_root() {
    let (mut chain, pol, a) = chain_of(2); // seq 0,1
    let ph = pol.policy_hash();
    let sink = MosaicAnchorSink::new(MockMosaic::new());
    let mut table = AnchoredTable::new();

    // Neo seq=1 + ghi bảng daemon TẠI thời điểm neo.
    let anchor1 = chain.publish_anchor().unwrap();
    assert_eq!(anchor1.seq, 1);
    sink.publish(&anchor1, AnchorPriority::Immediate).unwrap();
    assert!(table.record_anchor(&chain, &anchor1));

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
    assert!(table.get(5).is_none());
    assert_eq!(table.len(), 1);
}

// ── #6 DatumTooLarge / InsufficientAda: mock từ chối → đúng biến thể, KHÔNG panic ─
#[test]
fn s1_criteria_6_backend_errors_propagate() {
    let a1 = sample_anchor(1);

    let sink_big = MosaicAnchorSink::new(MockMosaic::failing(AnchorError::DatumTooLarge {
        bytes: 20_000,
    }));
    assert_eq!(
        sink_big
            .publish(&a1, AnchorPriority::Immediate)
            .unwrap_err(),
        AnchorError::DatumTooLarge { bytes: 20_000 }
    );

    let sink_ada = MosaicAnchorSink::new(MockMosaic::failing(AnchorError::InsufficientAda {
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
    let sink_net =
        MosaicAnchorSink::new(MockMosaic::failing(AnchorError::Network("timeout".into())));
    assert!(
        sink_net
            .publish(&a1, AnchorPriority::Immediate)
            .unwrap_err()
            .is_retryable()
    );
}
