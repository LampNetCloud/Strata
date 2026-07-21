//! S1 backend Settlement — HỢP NHẤT `AnchoredTable`: anchor neo qua tx metadata label
//! 1234, `resolve()` trả `StrataAnchor` chuẩn, verify ngược dùng CHUNG
//! [`AnchoredTable`] + [`verify_resolved`] như backend Mosaic (KHÔNG bảng song song).
//!
//! Codec + logic sink đã có unit test trong `src/settlement.rs`; file này kiểm luồng
//! end-to-end với **chain ký thật** (ed25519) — thứ unit test in-module không dựng.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use ed25519_dalek::SigningKey;
use lampnet_strata::anchor_sink::{
    AnchorError, AnchorPriority, AnchorSink, AnchoredTable, verify_resolved,
};
use lampnet_strata::chain::{Policy, StrataChain};
use lampnet_strata::refid::gen_ref_id_raw;
use lampnet_strata::settlement::{
    ChainQuery, SettlementRecord, SettlementSink, SinkConfig, SubmitOutcome, Submitter,
    encode_records,
};
use lampnet_strata::state::build_state_root;
use lampnet_strata::version::StrataVersion;
use rand::rngs::OsRng;

// ── helper dựng chain ký thật ────────────────────────────────────────────────
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
fn chain_of(n: u64) -> StrataChain {
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
    chain
}

// ── mock on-chain (một ví publisher) ─────────────────────────────────────────
#[derive(Default)]
struct Store {
    publisher: String,
    txs: Vec<String>, // MỚI → CŨ
    inputs: HashMap<String, Vec<String>>,
    meta: HashMap<String, Vec<u8>>,
}
/// Newtype quanh store dùng chung (orphan-rule: không impl trait ngoài cho `Rc` ngoài).
#[derive(Clone)]
struct Q(Rc<RefCell<Store>>);
impl ChainQuery for Q {
    fn address_txs(&self, addr: &str, limit: usize) -> Result<Vec<String>, AnchorError> {
        let s = self.0.borrow();
        if addr != s.publisher {
            return Ok(Vec::new());
        }
        Ok(s.txs.iter().take(limit).cloned().collect())
    }
    fn tx_input_addresses(&self, txid: &str) -> Result<Vec<String>, AnchorError> {
        Ok(self
            .0
            .borrow()
            .inputs
            .get(txid)
            .cloned()
            .unwrap_or_default())
    }
    fn tx_metadata_cbor(&self, txid: &str, _label: u64) -> Result<Option<Vec<u8>>, AnchorError> {
        Ok(self.0.borrow().meta.get(txid).cloned())
    }
}
struct MockSubmitter {
    publisher: String,
    store: Rc<RefCell<Store>>,
}
impl Submitter for MockSubmitter {
    fn submit(&self, records: &[SettlementRecord]) -> Result<SubmitOutcome, AnchorError> {
        let cbor = encode_records(records);
        let mut s = self.store.borrow_mut();
        let txid = format!("tx{}", s.txs.len());
        s.txs.insert(0, txid.clone());
        s.inputs.insert(txid.clone(), vec![self.publisher.clone()]);
        s.meta.insert(txid.clone(), cbor);
        Ok(SubmitOutcome {
            txid,
            address: self.publisher.clone(),
        })
    }
}
fn sink(pub_addr: &str) -> SettlementSink<Q, MockSubmitter> {
    let store = Rc::new(RefCell::new(Store {
        publisher: pub_addr.into(),
        ..Default::default()
    }));
    let submitter = MockSubmitter {
        publisher: pub_addr.into(),
        store: store.clone(),
    };
    let cfg = SinkConfig {
        publisher_address: pub_addr.into(),
        ..Default::default()
    };
    SettlementSink::new(cfg, Q(store), submitter)
}

/// Neo head thật qua Settlement; resolve → StrataAnchor khớp; verify_resolved dưới bảng
/// AnchoredTable CHUẨN (dùng chung cả 2 backend) PASS.
#[test]
fn settlement_resolve_feeds_shared_verify_resolved() {
    let chain = chain_of(3); // head seq=2
    let real = chain.anchor();
    let s = sink("addr_pub");
    s.publish(&real, AnchorPriority::Immediate)
        .unwrap()
        .unwrap();

    // Daemon ghi bảng AnchoredTable CHUẨN.
    let mut table = AnchoredTable::new();
    table.record_anchor(&chain, &real).unwrap();

    let resolved = s.resolve(&real.ref_id).unwrap().unwrap();
    assert_eq!(resolved, real);
    assert!(verify_resolved(&chain, &resolved, &table).is_ok());
}

/// Neo ở seq CŨ (1), rồi chain tiến tới head 2; verify_resolved phải verify version cũ
/// dưới `mmr_size` LÚC NEO (tái dựng), không phải size hiện tại.
#[test]
fn settlement_resolve_old_seq_verifies_at_anchored_size() {
    let a = author(2);
    let pol = policy(&a);
    let ph = pol.policy_hash();
    let ref_id = gen_ref_id_raw(&a.did, &[7u8; 32]);
    let v0 = signed(0, [0u8; 32], 100, &a, ph);
    let mut chain = StrataChain::genesis(ref_id, v0, &pol).unwrap();
    chain
        .append_version(signed(1, chain.head_version_hash(), 101, &a, ph), &pol)
        .unwrap();
    let anchor_seq1 = chain.anchor(); // seq=1
    let mut table = AnchoredTable::new();
    table.record_anchor(&chain, &anchor_seq1).unwrap();

    // Chain tiến thêm 1 version SAU khi neo.
    chain
        .append_version(signed(2, chain.head_version_hash(), 102, &a, ph), &pol)
        .unwrap();

    let s = sink("addr_pub");
    s.publish(&anchor_seq1, AnchorPriority::Immediate)
        .unwrap()
        .unwrap();
    let resolved = s.resolve(&ref_id).unwrap().unwrap();
    assert_eq!(resolved.seq, 1);
    assert!(verify_resolved(&chain, &resolved, &table).is_ok());
}

