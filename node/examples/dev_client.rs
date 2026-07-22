//! Sinh sẵn vật liệu để thử daemon bằng `curl`: một cặp khoá dev, dòng
//! `STRATA_NODE_KEYS`, và body `POST /v1/strata/create` **đã ký đúng**.
//!
//! Khoá ở đây sinh từ seed CỐ ĐỊNH — chỉ để dev/smoke-test, không dùng ở đâu khác.
//!
//! ```bash
//! cargo run -p lampnet-strata-node --example dev_client
//! ```

use ed25519_dalek::SigningKey;
use lampnet_strata::state::build_state_root;
use lampnet_strata::version::StrataVersion;
use lampnet_strata::{Policy, refid};

fn main() {
    let sk = SigningKey::from_bytes(&[0x5a; 32]);
    let pk = sk.verifying_key();
    let did = [0x11u8; 32];
    let nonce = [0x33u8; 32];
    let ts = 1_700_000_000u64;

    // Policy một-thành-viên = mặc định của `create` khi không gửi `policy_authors`.
    let mut policy = Policy::new();
    policy.allow(did, pk);
    let policy_hash = policy.policy_hash();

    let value = hex::decode("aa00000000000000000000000000000000000000000000000000000000000001").unwrap();
    let fields = vec![(b"diagnosis".to_vec(), value.clone())];
    let content_cid = hex::decode("cafe").unwrap();

    let mut v0 = StrataVersion::unsigned(
        0,
        [0u8; 32],
        content_cid.clone(),
        build_state_root(&fields),
        did,
        policy_hash,
        ts,
    );
    v0.sign(&sk);

    println!("# 1) chạy daemon với khoá dev này:");
    println!(
        "export STRATA_NODE_KEYS={}:{}",
        hex::encode(did),
        hex::encode(pk.to_bytes())
    );
    println!("cargo run -p lampnet-strata-node --bin strata-node\n");
    println!("# 2) ref_id sẽ là: {}", refid::gen_ref_id(&did, &nonce));
    println!("\n# 3) body create (đã ký):");
    println!(
        r#"{{"author_did":"{}","genesis_nonce":"{}","content_cid":"{}","state_fields":[{{"key":"diagnosis","value":"{}"}}],"policy_hash":"{}","ts":{},"sig":"{}"}}"#,
        hex::encode(did),
        hex::encode(nonce),
        hex::encode(&content_cid),
        hex::encode(&value),
        hex::encode(policy_hash),
        ts,
        hex::encode(v0.sig)
    );
}
