//! Test ON-CHAIN Preview (bằng chứng thật) — chạy tường minh:
//! `cargo test --test onchain_preview -- --ignored --nocapture`
//!
//! Điều kiện: env `VEDATA_WALLET_MNEMONIC` + `BLOCKFROST_TOKEN_GREENSUN` (tự nạp từ
//! `/Users/ductiger/Projects/VeDataIO/.env`, fallback `/Users/ductiger/Projects/.env`)
//! và ví publisher còn tADA. Ví 0 ADA → DỪNG, báo blocker faucet (KHÔNG tự xin).
//!
//! Kịch bản (tiêu chí test S1 §8.1 — bản on-chain):
//! 1. Dựng StrataChain THẬT: genesis + 2 version → `publish_anchor()` (seq=2).
//! 2. `SettlementSink::publish` → tx metadata label 1234 thật trên Preview.
//! 3. Chờ confirm (poll Blockfrost) → ghi txid + phí thật.
//! 4. `resolve()` đọc ngược từ Blockfrost → khớp anchor gốc từng bit.
//! 5. Verify ngược §8.1c dưới root ĐÃ NEO (AnchoredLog) — kể cả sau khi local append.
//! 6. Cố publish anchor seq THẤP hơn → `RollbackAttempt` TRƯỚC khi build tx.
//! 7. Publish lại đúng anchor đã neo → idempotent `Ok(None)`.

use lampnet_anchor_sink::{
    AnchorError, AnchorPriority, AnchorSink, AnchoredLog, BlockfrostQuery, SettlementSink,
    SinkConfig, TsSubmitter, verify_anchored,
};
use lampnet_strata::{Hash32, Policy, StrataAnchor, StrataChain, StrataVersion, build_state_root};

use ed25519_dalek::SigningKey;
use rand::RngCore;
use rand::rngs::OsRng;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Ví bank/publisher Preview (TESTNET-FAUCET.md — thông tin CÔNG KHAI).
const PUBLISHER_ADDR: &str = "addr_test1qqh9u9qc4l2q9eyzx2c58pmpqn9vvxy2gjux0lah2wp33axx7cqq55f75fypagzqnelz3uzwxf764qzjx8kvaaw3q3yq8fyl7p";

const ENV_PATHS: [&str; 2] = [
    "/Users/ductiger/Projects/VeDataIO/.env",
    "/Users/ductiger/Projects/.env",
];

/// Nạp KEY=VAL từ .env vào env process (KHÔNG in giá trị). Bỏ qua dòng rác/comment.
/// Máy khác đặt `STRATA_ENV_FILE=/path/.env` để khỏi lệ thuộc path mặc định.
fn load_env() {
    let override_path = std::env::var("STRATA_ENV_FILE").ok();
    let paths: Vec<&str> = override_path
        .as_deref()
        .into_iter()
        .chain(ENV_PATHS)
        .collect();
    for p in paths {
        let Ok(text) = std::fs::read_to_string(p) else { continue };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let k = k.trim().trim_start_matches("export ").trim();
            if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                continue;
            }
            if std::env::var(k).is_err() {
                let v = v.trim().trim_matches('"').trim_matches('\'');
                // SAFETY: test đơn luồng tại điểm này (chạy đầu test, trước mọi thread).
                unsafe { std::env::set_var(k, v) };
            }
        }
    }
}

fn signed_version(
    seq: u64,
    prev: Hash32,
    ts: u64,
    did: [u8; 32],
    sk: &SigningKey,
    ph: Hash32,
) -> StrataVersion {
    let sr = build_state_root(&[(b"field".to_vec(), format!("gia-tri-{seq}").into_bytes())]);
    let mut v = StrataVersion::unsigned(seq, prev, b"cid-preview".to_vec(), sr, did, ph, ts);
    v.sign(sk);
    v
}

#[test]
#[ignore = "on-chain Preview: cần env secret + tADA; chạy với -- --ignored"]
fn s1_settlement_anchor_on_preview() {
    load_env();
    let token = std::env::var("BLOCKFROST_TOKEN_GREENSUN")
        .expect("BLOCKER: thiếu env BLOCKFROST_TOKEN_GREENSUN (nguồn: VeDataIO/.env)");
    assert!(
        std::env::var("VEDATA_WALLET_MNEMONIC").is_ok(),
        "BLOCKER: thiếu env VEDATA_WALLET_MNEMONIC (nguồn: VeDataIO/.env)"
    );

    let query = BlockfrostQuery::preview(token);

    // BƯỚC 0 — số dư ví trước. 0 ADA → DỪNG, báo blocker faucet.
    let balance = query.lovelace_balance(PUBLISHER_ADDR).expect("đọc số dư ví publisher");
    println!("[0] Số dư ví publisher: {} lovelace (~{} tADA)", balance, balance / 1_000_000);
    assert!(
        balance > 3_000_000,
        "BLOCKER: ví publisher {PUBLISHER_ADDR} chỉ còn {balance} lovelace — cần nạp \
         faucet Preview (https://docs.cardano.org/cardano-testnets/tools/faucet) trước khi test."
    );

    // BƯỚC 1 — StrataChain THẬT: genesis + 2 version; ref_id ngẫu nhiên mỗi lần chạy
    // (tránh nhiễu anchor của lần chạy trước trên cùng ví).
    let mut ref_id = [0u8; 32];
    OsRng.fill_bytes(&mut ref_id);
    let sk = SigningKey::generate(&mut OsRng);
    let did = [0x51; 32];
    let mut policy = Policy::new();
    policy.allow(did, sk.verifying_key());
    let ph = policy.policy_hash();
    let v0 = signed_version(0, [0u8; 32], 1000, did, &sk, ph);
    let mut chain = StrataChain::genesis(ref_id, v0, &policy).unwrap();
    chain
        .append_version(signed_version(1, chain.head_version_hash(), 1010, did, &sk, ph), &policy)
        .unwrap();
    chain
        .append_version(signed_version(2, chain.head_version_hash(), 1020, did, &sk, ph), &policy)
        .unwrap();
    let anchor = chain.publish_anchor().unwrap(); // INV-E7 lớp core
    assert_eq!(anchor.seq, 2);
    println!("[1] Chain thật: ref_id={} seq_head=2", hex::encode(ref_id));

    // Bảng anchored §8.1c — daemon lưu (seq → mmr_root, mmr_size) TẠI LÚC neo.
    let mut anchored_log = AnchoredLog::new();
    assert!(anchored_log.record(anchor.ref_id, anchor.seq, anchor.mmr_root, chain.len() as u64));

    // BƯỚC 2 — publish qua sink Settlement thật.
    let submitter_dir: PathBuf = [env!("CARGO_MANIFEST_DIR"), "submitter"].iter().collect();
    let sink = SettlementSink::new(
        SinkConfig {
            publisher_address: PUBLISHER_ADDR.to_string(),
            resolve_scan_limit: 120,
            ..SinkConfig::default()
        },
        BlockfrostQuery::preview(
            std::env::var("BLOCKFROST_TOKEN_GREENSUN").unwrap(),
        ),
        TsSubmitter { submitter_dir, label: 1234, timeout_secs: 180 },
    );

    let receipt = sink
        .publish(&anchor, AnchorPriority::Milestone)
        .expect("publish on-chain")
        .expect("anchor mới → phải có receipt");
    println!("[2] Tx submit: {}", receipt.txid);
    println!("    https://preview.cardanoscan.io/transaction/{}", receipt.txid);

    // BƯỚC 3 — chờ confirm (Blockfrost thấy tx).
    let deadline = Instant::now() + Duration::from_secs(600);
    let fee = loop {
        match query.tx_fee_if_confirmed(&receipt.txid) {
            Ok(Some(fee)) => break fee,
            Ok(None) => {
                assert!(Instant::now() < deadline, "tx chưa confirm sau 600s");
                std::thread::sleep(Duration::from_secs(10));
            }
            Err(AnchorError::Network(e)) => {
                println!("    (poll lỗi mạng tạm: {e})");
                std::thread::sleep(Duration::from_secs(10));
            }
            Err(e) => panic!("poll confirm: {e}"),
        }
    };
    println!("[3] CONFIRMED. Phí thật: {} lovelace (~{:.6} ADA)", fee, fee as f64 / 1e6);

    // BƯỚC 4 — resolve đọc ngược từ Blockfrost → khớp anchor gốc từng bit.
    let on_chain = sink
        .resolve(&ref_id)
        .expect("resolve")
        .expect("anchor phải đọc lại được sau confirm");
    assert_eq!(on_chain, anchor, "anchor on-chain phải khớp anchor gốc từng bit");
    assert_eq!(on_chain.mmr_root, chain.mmr_root());
    assert_eq!(on_chain.head_version_hash, chain.head_version_hash());
    println!(
        "[4] resolve() khớp: seq={} mmr_root={} head={}",
        on_chain.seq,
        hex::encode(on_chain.mmr_root),
        hex::encode(on_chain.head_version_hash)
    );

    // BƯỚC 5 — verify ngược §8.1c: local append tiếp 2 version (seq=4) rồi verify
    // proof version seq=2 dưới root ĐÃ NEO với size CŨ.
    chain
        .append_version(signed_version(3, chain.head_version_hash(), 1030, did, &sk, ph), &policy)
        .unwrap();
    chain
        .append_version(signed_version(4, chain.head_version_hash(), 1040, did, &sk, ph), &policy)
        .unwrap();
    assert_ne!(chain.mmr_root(), on_chain.mmr_root, "root local đã tiến xa root neo");
    verify_anchored(&chain, &on_chain, &anchored_log).expect("verify ngược dưới root ĐÃ NEO");
    // resolve vẫn trả seq đã neo (local seq=4 chưa neo).
    let still = sink.resolve(&ref_id).unwrap().unwrap();
    assert_eq!(still.seq, 2);
    println!("[5] Verify ngược §8.1c PASS (local head seq=4, on-chain vẫn seq=2)");

    // BƯỚC 6 — INV-E7 lớp adapter: cố neo anchor seq THẤP hơn → RollbackAttempt,
    // bị chặn TRƯỚC khi build tx (không tốn phí).
    let stale = StrataAnchor {
        ref_id,
        head_version_hash: chain.version(1).unwrap().version_hash(),
        mmr_root: [0u8; 32],
        seq: 1,
    };
    let err = sink.publish(&stale, AnchorPriority::Milestone).unwrap_err();
    assert_eq!(err, AnchorError::RollbackAttempt { on_chain_seq: 2, attempted: 1 });
    println!("[6] RollbackAttempt chặn TRƯỚC build tx: {err}");

    // BƯỚC 7 — idempotent: publish lại đúng anchor đã neo → Ok(None), KHÔNG tx mới.
    let again = sink.publish(&anchor, AnchorPriority::Milestone).unwrap();
    assert_eq!(again, None, "publish lại anchor đã neo phải là no-op");
    println!("[7] Idempotent republish → Ok(None). HOÀN TẤT.");

    println!("\n=== BẰNG CHỨNG ===");
    println!("txid : {}", receipt.txid);
    println!("link : https://preview.cardanoscan.io/transaction/{}", receipt.txid);
    println!("phí  : {} lovelace", fee);
    println!("ref  : {}", hex::encode(ref_id));
}
