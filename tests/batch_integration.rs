//! Integration + red-team tests cho S3 BatchPolicy/checkpoint sub-MMR
//! (Strata-API §8.3 — 7 tiêu chí test).

use ed25519_dalek::SigningKey;
use lampnet_strata::{
    BatchEntry, BatchError, BatchPolicy, EpochAccumulator, entry_leaf_data, verify_entry,
    verify_entry_two_tier,
    chain::{Policy, StrataChain},
    version::StrataVersion,
};
use rand::rngs::OsRng;

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

/// Version ký sẵn với `state_root` TÙY Ý (checkpoint dùng sub-MMR root làm state_root).
fn signed_with_root(
    seq: u64,
    prev: [u8; 32],
    ts: u64,
    a: &Author,
    ph: [u8; 32],
    state_root: [u8; 32],
    content_cid: &[u8],
) -> StrataVersion {
    let mut v = StrataVersion::unsigned(
        seq,
        prev,
        content_cid.to_vec(),
        state_root,
        a.did,
        ph,
        ts,
    );
    v.sign(&a.sk);
    v
}

/// Gom `n` entry (ts = now = 1_000) rồi đóng epoch. entry_seq bắt đầu từ 0.
fn closed_epoch_of(n: u64) -> lampnet_strata::ClosedEpoch {
    let mut acc = EpochAccumulator::new(BatchPolicy::default());
    for i in 0..n {
        acc.push(i, 1_000, format!("payload-{i}").into_bytes(), 1_000)
            .unwrap();
    }
    acc.close().unwrap()
}

/// Tiêu chí #1: 1000 entry tần suất cao → MỘT version checkpoint duy nhất
/// (không phải 1000 version) → MỘT anchor.
#[test]
fn checkpoint_1000_versions() {
    let a = author(1);
    let pol = policy(&a);
    let ph = pol.policy_hash();

    let epoch = closed_epoch_of(1000);
    assert_eq!(epoch.sub_size, 1000);

    // Genesis rồi MỘT append_version checkpoint (state_root = sub-MMR root,
    // content_cid = CID blob lô do caller cấp — core no-I/O).
    let v0 = signed_with_root(0, [0u8; 32], 100, &a, ph, [0u8; 32], b"genesis_cid");
    let mut chain = StrataChain::genesis([0xAB; 32], v0, &pol).unwrap();
    let v1 = signed_with_root(
        1,
        chain.head_version_hash(),
        1_000,
        &a,
        ph,
        epoch.sub_mmr_root,
        b"batch_blob_cid",
    );
    chain.append_version(v1, &pol).unwrap();

    assert_eq!(chain.len(), 2, "1000 entry → chỉ 1 checkpoint version (+ genesis)");
    let anchor = chain.publish_anchor().unwrap();
    assert_eq!(anchor.seq, 1, "một anchor duy nhất cho cả lô 1000 entry");
    assert_eq!(chain.head().state_root, epoch.sub_mmr_root);
}

/// Tiêu chí #2: prove một entry giữa lô — proof ~O(log N), verify pass.
#[test]
fn prove_entry_in_checkpoint() {
    let epoch = closed_epoch_of(1000);
    let idx = 517usize;
    let (proof, sub_size, leaf) = epoch.prove_entry(idx).unwrap();
    assert_eq!(sub_size, 1000);
    assert_eq!(leaf, entry_leaf_data(517, b"payload-517"));

    // O(log N): siblings ≤ ceil(log2(1000)) = 10; báo số thật.
    let sibling_bytes = proof.siblings.len() * 32;
    assert!(
        proof.siblings.len() <= 10,
        "siblings = {} (> log2(1000))",
        proof.siblings.len()
    );
    println!(
        "sub-proof entry {idx}/1000: {} siblings = {} byte hash + {} peaks",
        proof.siblings.len(),
        sibling_bytes,
        proof.peaks.len()
    );

    assert!(verify_entry(
        epoch.sub_mmr_root,
        517,
        b"payload-517",
        idx,
        sub_size,
        &proof
    ));
    // Sai index → fail (bind chặt index ↔ hash).
    assert!(!verify_entry(
        epoch.sub_mmr_root,
        517,
        b"payload-517",
        idx + 1,
        sub_size,
        &proof
    ));
}

/// Tiêu chí #3: ghép sub-proof + version-proof → verify về mmr_root ĐÃ NEO.
#[test]
fn two_tier_inclusion() {
    let a = author(1);
    let pol = policy(&a);
    let ph = pol.policy_hash();

    let epoch = closed_epoch_of(100);
    let v0 = signed_with_root(0, [0u8; 32], 100, &a, ph, [0u8; 32], b"genesis_cid");
    let mut chain = StrataChain::genesis([0xAB; 32], v0, &pol).unwrap();
    let v1 = signed_with_root(
        1,
        chain.head_version_hash(),
        1_000,
        &a,
        ph,
        epoch.sub_mmr_root,
        b"batch_blob_cid",
    );
    chain.append_version(v1, &pol).unwrap();
    let anchor = chain.publish_anchor().unwrap();

    // Tầng dưới: entry 42 trong sub-MMR. Tầng trên: checkpoint seq=1 trong chain-MMR.
    let (sub_proof, sub_size, _) = epoch.prove_entry(42).unwrap();
    let (ver_proof, chain_size, _) = chain.prove_version(1).unwrap();
    let checkpoint = chain.version(1).unwrap();

    assert!(verify_entry_two_tier(
        anchor.mmr_root,
        checkpoint,
        chain_size,
        &ver_proof,
        42,
        b"payload-42",
        42,
        sub_size,
        &sub_proof
    ));

    // Ghép SAI: sub-proof đúng nhưng checkpoint khác (genesis, state_root ≠ sub root) → fail.
    let (ver_proof0, size0, _) = chain.prove_version(0).unwrap();
    assert!(!verify_entry_two_tier(
        anchor.mmr_root,
        chain.version(0).unwrap(),
        size0,
        &ver_proof0,
        42,
        b"payload-42",
        42,
        sub_size,
        &sub_proof
    ));
    // Root neo giả → fail tầng trên.
    assert!(!verify_entry_two_tier(
        [0xEE; 32],
        checkpoint,
        chain_size,
        &ver_proof,
        42,
        b"payload-42",
        42,
        sub_size,
        &sub_proof
    ));
}

/// Tiêu chí #4: bơm max_entries+1 → epoch đóng TẠI max_entries (không chờ hết
/// epoch_secs); entry dư thuộc epoch SAU.
#[test]
fn close_on_max_entries() {
    let polcy = BatchPolicy {
        epoch_secs: 3600,
        max_entries: 5,
        flush_max_age: 300,
    };
    let mut acc = EpochAccumulator::new(polcy);
    let now = 1_000u64;
    for i in 0..5u64 {
        acc.push(i, now, b"e".to_vec(), now).unwrap();
    }
    // Van (b) chạm ngay khi đầy — KHÔNG phụ thuộc thời gian.
    assert!(acc.should_close(now));
    // Entry thứ 6 đến trong lúc epoch đầy/đang đóng → bị đẩy sang epoch SAU.
    assert_eq!(
        acc.push(5, now, b"e".to_vec(), now),
        Err(BatchError::EpochFull { max_entries: 5 })
    );
    let epoch = acc.close().unwrap();
    assert_eq!(epoch.sub_size, 5, "epoch đóng tại đúng max_entries");
    // Entry dư push lại → vào epoch mới, index 0.
    assert_eq!(acc.push(5, now, b"e".to_vec(), now), Ok(0));
    assert_eq!(acc.len(), 1);
    assert!(!acc.should_close(now), "epoch mới chưa chạm van nào");
}

/// Tiêu chí #5: van (c) là TUỔI ENTRY CŨ NHẤT, KHÔNG phải "im lặng" — entry mới
/// rả rích mỗi 60s (< flush_max_age) KHÔNG reset van; đóng khi entry đầu già 180s.
#[test]
fn close_on_flush_max_age() {
    let mut acc = EpochAccumulator::new(BatchPolicy::proofchat()); // flush_max_age=180
    // Entry đầu lúc t=1000; entry mới mỗi 60 giây (chuỗi tin nhịp chậm).
    acc.push(0, 1_000, b"m0".to_vec(), 1_000).unwrap();
    acc.push(1, 1_060, b"m1".to_vec(), 1_060).unwrap();
    acc.push(2, 1_120, b"m2".to_vec(), 1_120).unwrap();
    assert!(
        !acc.should_close(1_179),
        "tuổi entry đầu 179 < 180 → chưa đóng (dù đã có 3 entry)"
    );
    // t=1180: vẫn có entry mới đến ĐÚNG LÚC NÀY (không hề idle) — vẫn phải đóng
    // vì entry ĐẦU đã chờ đủ 180s. Ngữ nghĩa oldest-age ≠ idle: nếu là idle,
    // khoảng lặng chỉ 60s < 180s thì không bao giờ đóng.
    acc.push(3, 1_180, b"m3".to_vec(), 1_180).unwrap();
    assert!(
        acc.should_close(1_180),
        "oldest_entry_age = 180 ≥ flush_max_age → đóng dù entry mới vừa đến"
    );
    let epoch = acc.close().unwrap();
    assert_eq!(epoch.sub_size, 4, "cả 4 tin nhịp chậm gom vào MỘT checkpoint");
}

/// Tiêu chí #6: entry_bytes canonical — khác payload cùng độ dài → leaf khác;
/// cùng payload khác entry_seq → leaf khác; length-prefix chống nhập nhằng nối chuỗi.
#[test]
fn entry_bytes_canonical() {
    // Hai payload khác nhau CÙNG độ dài → leaf khác.
    assert_ne!(entry_leaf_data(7, b"aaaa"), entry_leaf_data(7, b"aaab"));
    // Cùng payload, khác entry_seq → leaf khác (entry_seq nằm trong leaf).
    assert_ne!(entry_leaf_data(7, b"aaaa"), entry_leaf_data(8, b"aaaa"));
    // Length-prefix: "ab" ≠ "a" + đuôi trùng — không nhập nhằng nối chuỗi.
    assert_ne!(entry_leaf_data(7, b"ab"), entry_leaf_data(7, b"a"));
    // entry_bytes khớp layout chốt: u64_be(seq) ‖ u32_be(len) ‖ payload.
    let e = BatchEntry {
        entry_seq: 0x0102030405060708,
        ts: 999, // ts KHÔNG vào entry_bytes
        payload: b"xy".to_vec(),
    };
    let mut expect = vec![1u8, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 2];
    expect.extend_from_slice(b"xy");
    assert_eq!(e.entry_bytes(), expect);
    assert_eq!(e.leaf_data(), entry_leaf_data(e.entry_seq, &e.payload));
}

/// Tiêu chí #7 (red-team): tamper 1 entry trong lô sau khi đóng → proof fail;
/// replay entry_seq cũ → bị từ chối.
#[test]
fn redteam_tamper_entry_and_replay_seq() {
    let a = author(1);
    let pol = policy(&a);
    let ph = pol.policy_hash();

    let mut acc = EpochAccumulator::new(BatchPolicy::default());
    for i in 0..8u64 {
        acc.push(i, 1_000, format!("msg-{i}").into_bytes(), 1_000)
            .unwrap();
    }
    let epoch = acc.close().unwrap();

    let v0 = signed_with_root(0, [0u8; 32], 100, &a, ph, [0u8; 32], b"g");
    let mut chain = StrataChain::genesis([0xAB; 32], v0, &pol).unwrap();
    let v1 = signed_with_root(
        1,
        chain.head_version_hash(),
        1_000,
        &a,
        ph,
        epoch.sub_mmr_root,
        b"blob",
    );
    chain.append_version(v1, &pol).unwrap();
    let anchor = chain.publish_anchor().unwrap();

    // (1) Tamper NỘI DUNG entry 3 sau khi đóng: proof gốc + payload giả → fail.
    let (sub_proof, sub_size, _) = epoch.prove_entry(3).unwrap();
    assert!(!verify_entry(
        epoch.sub_mmr_root,
        3,
        b"msg-3-TAMPERED",
        3,
        sub_size,
        &sub_proof
    ));
    // Tamper entry_seq của entry (giữ payload) → fail.
    assert!(!verify_entry(
        epoch.sub_mmr_root,
        7,
        b"msg-3",
        3,
        sub_size,
        &sub_proof
    ));
    // Hai tầng với entry giả → fail; entry thật → pass (đối chứng).
    let (ver_proof, chain_size, _) = chain.prove_version(1).unwrap();
    let checkpoint = chain.version(1).unwrap();
    assert!(!verify_entry_two_tier(
        anchor.mmr_root, checkpoint, chain_size, &ver_proof,
        3, b"msg-3-TAMPERED", 3, sub_size, &sub_proof
    ));
    assert!(verify_entry_two_tier(
        anchor.mmr_root, checkpoint, chain_size, &ver_proof,
        3, b"msg-3", 3, sub_size, &sub_proof
    ));

    // (2) Kẻ tấn công dựng LẠI lô với entry 3 bị sửa → root khác → không khớp
    // state_root checkpoint đã ký/neo.
    let mut forged = lampnet_strata::mmr::Mmr::<lampnet_strata::Blake3Hasher>::new();
    for i in 0..8u64 {
        let payload = if i == 3 {
            b"msg-3-TAMPERED".to_vec()
        } else {
            format!("msg-{i}").into_bytes()
        };
        forged.append(&entry_leaf_data(i, &payload));
    }
    assert_ne!(forged.root(), epoch.sub_mmr_root);
    assert_ne!(forged.root(), checkpoint.state_root);

    // (3) Replay entry_seq cũ (trong epoch mới, xuyên epoch) → từ chối, không ghi.
    assert_eq!(
        acc.push(3, 2_000, b"replay-old".to_vec(), 2_000),
        Err(BatchError::EntrySeqReplay { last: 7, got: 3 })
    );
    assert_eq!(
        acc.push(7, 2_000, b"replay-last".to_vec(), 2_000),
        Err(BatchError::EntrySeqReplay { last: 7, got: 7 })
    );
    assert!(acc.is_empty(), "replay KHÔNG được ghi vào epoch mới");
    assert_eq!(acc.push(8, 2_000, b"next".to_vec(), 2_000), Ok(0));
}
