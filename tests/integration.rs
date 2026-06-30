//! Integration + red-team tests cho `lampnet-strata`. Map từng INV-E* qua API public.

use ed25519_dalek::SigningKey;
use lampnet_strata::{
    audit::{AuditAction, AuditEntry, AuditLog},
    chain::{StrataChain, StrataError, Policy},
    privacy::{decoy_sizes, pad_to_bucket, BUCKETS},
    refid::{decode_ref_id, gen_ref_id, gen_ref_id_raw},
    state::{build_state_root, prove_field, verify_field_proof},
    version::StrataVersion,
};
use rand::rngs::OsRng;

struct Author {
    did: [u8; 32],
    sk: SigningKey,
}
fn author(tag: u8) -> Author {
    Author { did: [tag; 32], sk: SigningKey::generate(&mut OsRng) }
}
fn policy(authors: &[&Author]) -> Policy {
    let mut p = Policy::new();
    for a in authors {
        p.allow(a.did, a.sk.verifying_key());
    }
    p
}
fn signed(
    seq: u64,
    prev: [u8; 32],
    ts: u64,
    a: &Author,
    ph: [u8; 32],
    fields: &[(Vec<u8>, Vec<u8>)],
) -> StrataVersion {
    let sr = build_state_root(fields);
    let mut v = StrataVersion::unsigned(seq, prev, b"content_cid_pure".to_vec(), sr, a.did, ph, ts);
    v.sign(&a.sk);
    v
}

/// Loại #1 Tĩnh (1 version) + #2 Chuỗi-thêm + #4 Hồ sơ cấu trúc, end-to-end.
#[test]
fn full_lifecycle_static_append_structured() {
    let alice = author(1);
    let pol = policy(&[&alice]);
    let ph = pol.policy_hash();
    let ref_id = gen_ref_id_raw(&alice.did, &[9u8; 32]);

    let f0 = vec![(b"name".to_vec(), b"cid_name".to_vec()), (b"dob".to_vec(), b"cid_dob".to_vec())];
    let v0 = signed(0, [0u8; 32], 100, &alice, ph, &f0);
    let mut chain = StrataChain::genesis(ref_id, v0, &pol).unwrap();

    // #2: thêm version (cập nhật field "name").
    let f1 = vec![(b"name".to_vec(), b"cid_name2".to_vec()), (b"dob".to_vec(), b"cid_dob".to_vec())];
    let v1 = signed(1, chain.head_version_hash(), 200, &alice, ph, &f1);
    chain.append_version(v1, &pol).unwrap();

    assert_eq!(chain.len(), 2);
    let anchor = chain.anchor();
    assert_eq!(anchor.seq, 1);
    assert_eq!(anchor.ref_id, ref_id);

    // #4: field-proof của "name" tại head, KHÔNG lộ "dob".
    let p = prove_field(&f1, b"name").unwrap();
    assert!(verify_field_proof(&p));
    assert_eq!(p.state_root, chain.head().state_root);
}

/// INV-E5: ref_id chỉ phụ thuộc hash(author,nonce) — không nhãn loại.
#[test]
fn inv_e5_refid_no_type_leak() {
    let did = [3u8; 32];
    let nonce = [4u8; 32];
    // "Vault Strata" và "Bulk Strata" cùng (author,nonce) → cùng ref_id.
    let vault_ref = gen_ref_id(&did, &nonce);
    let bulk_ref = gen_ref_id(&did, &nonce);
    assert_eq!(vault_ref, bulk_ref);
    assert!(vault_ref.starts_with("lnref1"));
    // Payload = 32B hash thuần (không class byte).
    assert_eq!(decode_ref_id(&vault_ref).unwrap().len(), 32);
}

/// INV-E6: red-team field-leak — proof không cho suy trường khác; tamper fail.
#[test]
fn inv_e6_field_privacy_redteam() {
    let fields = vec![
        (b"diagnosis".to_vec(), b"HIV_positive".to_vec()),
        (b"name".to_vec(), b"cid_name".to_vec()),
        (b"insurer".to_vec(), b"cid_ins".to_vec()),
    ];
    // Proof "name" verify; sibling không lộ "diagnosis".
    let p = prove_field(&fields, b"name").unwrap();
    assert!(verify_field_proof(&p));
    for (_, val) in &fields {
        for (sib, _) in &p.siblings {
            assert_ne!(sib.as_slice(), val.as_slice());
        }
    }
    // Đổi value → fail.
    let mut bad = p.clone();
    bad.value = b"cid_name_FAKE".to_vec();
    bad.fvh = lampnet_strata::state::fval_hash(&bad.value);
    assert!(!verify_field_proof(&bad));
}

/// INV-E1 red-team: tamper lịch sử → hash-link gãy.
#[test]
fn inv_e1_redteam_tamper_history() {
    let a = author(1);
    let pol = policy(&[&a]);
    let ph = pol.policy_hash();
    let f = vec![(b"k".to_vec(), b"v".to_vec())];
    let v0 = signed(0, [0u8; 32], 100, &a, ph, &f);
    let mut chain = StrataChain::genesis([0xAB; 32], v0, &pol).unwrap();
    // version mới phải nối đúng head; prev_hash bịa → reject.
    let bad = signed(1, [0x77; 32], 200, &a, ph, &f);
    assert!(matches!(chain.append_version(bad, &pol), Err(StrataError::HashLinkBroken { .. })));
}

/// INV-E7 red-team: rollback anchor bị từ chối.
#[test]
fn inv_e7_redteam_rollback() {
    let a = author(1);
    let pol = policy(&[&a]);
    let ph = pol.policy_hash();
    let f = vec![(b"k".to_vec(), b"v".to_vec())];
    let v0 = signed(0, [0u8; 32], 100, &a, ph, &f);
    let mut chain = StrataChain::genesis([0xAB; 32], v0, &pol).unwrap();
    let v1 = signed(1, chain.head_version_hash(), 200, &a, ph, &f);
    chain.append_version(v1, &pol).unwrap();
    chain.publish_anchor().unwrap(); // seq 1 đã neo
    assert!(matches!(chain.publish_anchor(), Err(StrataError::AnchorRollback { .. })));
}

/// INV-E4 red-team: type/sig — author ngoài policy bị chặn.
#[test]
fn inv_e4_redteam_unauthorized_author() {
    let owner = author(1);
    let attacker = author(2);
    let pol = policy(&[&owner]); // chỉ owner
    let ph = pol.policy_hash();
    let f = vec![(b"k".to_vec(), b"v".to_vec())];
    let v0 = signed(0, [0u8; 32], 100, &owner, ph, &f);
    let mut chain = StrataChain::genesis([0xAB; 32], v0, &pol).unwrap();
    let v1 = signed(1, chain.head_version_hash(), 200, &attacker, ph, &f);
    assert!(matches!(chain.append_version(v1, &pol), Err(StrataError::PolicyDenied)));
}

/// INV-E3 (kế thừa merkle-anchor): proof cũ verify dưới root mới.
#[test]
fn inv_e3_append_only_old_proof_valid() {
    let a = author(1);
    let pol = policy(&[&a]);
    let ph = pol.policy_hash();
    let f = vec![(b"k".to_vec(), b"v".to_vec())];
    let v0 = signed(0, [0u8; 32], 100, &a, ph, &f);
    let mut chain = StrataChain::genesis([0xAB; 32], v0, &pol).unwrap();
    let (p0, s0, vh0) = chain.prove_version(0).unwrap();
    assert!(StrataChain::verify_version(chain.mmr_root(), &vh0, 0, s0, &p0));

    for seq in 1..5u64 {
        let v = signed(seq, chain.head_version_hash(), 100 + seq * 10, &a, ph, &f);
        chain.append_version(v, &pol).unwrap();
    }
    let (p0b, s0b, vh0b) = chain.prove_version(0).unwrap();
    assert!(StrataChain::verify_version(chain.mmr_root(), &vh0b, 0, s0b, &p0b));
}

/// INV-E8 (kế thừa): dup-leaf / determinism của state tree — second-preimage guard.
#[test]
fn inv_e8_state_tree_dup_leaf_and_determinism() {
    // Hai trường giá trị giống nhau nhưng key khác → leaf khác (key trộn vào leaf).
    let f = vec![(b"a".to_vec(), b"same".to_vec()), (b"b".to_vec(), b"same".to_vec())];
    let r1 = build_state_root(&f);
    // Đảo thứ tự nhập → cùng root (sort theo key).
    let f2 = vec![(b"b".to_vec(), b"same".to_vec()), (b"a".to_vec(), b"same".to_vec())];
    assert_eq!(r1, build_state_root(&f2));
    // Số trường lẻ (carry, không copy lá cuối) vẫn cho proof verify.
    let f3 = vec![
        (b"a".to_vec(), b"1".to_vec()),
        (b"b".to_vec(), b"2".to_vec()),
        (b"c".to_vec(), b"3".to_vec()),
    ];
    for (k, _) in &f3 {
        assert!(verify_field_proof(&prove_field(&f3, k).unwrap()));
    }
}

/// INV-E9 mức code: padding/decoy + audit-log append-only.
#[test]
fn inv_e9_padding_and_audit_append_only() {
    assert_eq!(pad_to_bucket(3000), pad_to_bucket(3500)); // không phân biệt loại theo size
    let d = decoy_sizes(7, 10);
    assert!(d.iter().all(|s| BUCKETS.contains(s)));

    let mut log = AuditLog::new();
    log.append_access(AuditEntry {
        created_ts: 100,
        actor_did: [1u8; 32],
        action: AuditAction::Create,
        signed_hash: [2u8; 32],
        location: [3u8; 32],
    })
    .unwrap();
    let (proof, size, leaf) = log.prove(0).unwrap();
    assert!(AuditLog::verify(log.root(), &leaf, 0, size, &proof));
    log.append_access(AuditEntry {
        created_ts: 200,
        actor_did: [9u8; 32],
        action: AuditAction::Read,
        signed_hash: [2u8; 32],
        location: [3u8; 32],
    })
    .unwrap();
    // proof cũ vẫn đúng (append-only).
    let (p0, s0, l0) = log.prove(0).unwrap();
    assert!(AuditLog::verify(log.root(), &l0, 0, s0, &p0));
}

/// Temporal: version_at(t) trả version có ts lớn nhất ≤ t + proof verify.
#[test]
fn temporal_version_at() {
    let a = author(1);
    let pol = policy(&[&a]);
    let ph = pol.policy_hash();
    let f = vec![(b"k".to_vec(), b"v".to_vec())];
    let v0 = signed(0, [0u8; 32], 100, &a, ph, &f);
    let mut chain = StrataChain::genesis([0xAB; 32], v0, &pol).unwrap();
    let v1 = signed(1, chain.head_version_hash(), 200, &a, ph, &f);
    chain.append_version(v1, &pol).unwrap();
    let v2 = signed(2, chain.head_version_hash(), 300, &a, ph, &f);
    chain.append_version(v2, &pol).unwrap();

    assert!(chain.version_at(50).is_none());
    assert_eq!(chain.version_at(100).unwrap().0.seq, 0);
    assert_eq!(chain.version_at(299).unwrap().0.seq, 1);
    assert_eq!(chain.version_at(1000).unwrap().0.seq, 2);
    let (v, proof) = chain.version_at(299).unwrap();
    assert!(StrataChain::verify_version(
        chain.mmr_root(),
        &v.version_hash(),
        v.seq,
        chain.len() as u64,
        &proof
    ));
}

/// Determinism: cùng input → cùng mmr_root + state_root (version_hash độc lập sig).
#[test]
fn determinism_roots_independent_of_sig_nonce() {
    let a = author(5);
    let pol = policy(&[&a]);
    let ph = pol.policy_hash();
    let f = vec![(b"x".to_vec(), b"y".to_vec())];
    let build = || {
        let v0 = signed(0, [0u8; 32], 100, &a, ph, &f);
        let mut c = StrataChain::genesis([0xCD; 32], v0, &pol).unwrap();
        let v1 = signed(1, c.head_version_hash(), 200, &a, ph, &f);
        c.append_version(v1, &pol).unwrap();
        (c.mmr_root(), build_state_root(&f))
    };
    assert_eq!(build(), build());
}
