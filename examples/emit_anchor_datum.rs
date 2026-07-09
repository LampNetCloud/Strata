//! Emit datum CIP-68 (detailed-JSON cardano-cli) cho anchor của một chain thật —
//! dùng cho neo Preview thật (S1 test #2). In datum + 4 trường kỳ vọng để script assert.
//!
//! `cargo run --example emit_anchor_datum` → stdout:
//!   DATUM_JSON=<detailed json>
//!   REF_ID=<hex> / HVH=<hex> / MMR_ROOT=<hex> / SEQ=<n>
//!
//! Keys deterministic (seed cố định) → anchor tái lập giữa các lần chạy.

use ed25519_dalek::SigningKey;
use lampnet_strata::anchor_sink::map_anchor_to_datum;
use lampnet_strata::chain::{Policy, StrataChain};
use lampnet_strata::refid::gen_ref_id_raw;
use lampnet_strata::state::build_state_root;
use lampnet_strata::version::StrataVersion;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn signed(
    seq: u64,
    prev: [u8; 32],
    ts: u64,
    did: [u8; 32],
    sk: &SigningKey,
    ph: [u8; 32],
) -> StrataVersion {
    let sr = build_state_root(&[(b"name".to_vec(), format!("v{seq}").into_bytes())]);
    let mut v = StrataVersion::unsigned(seq, prev, b"cid".to_vec(), sr, did, ph, ts);
    v.sign(sk);
    v
}

fn main() {
    // Author deterministic.
    let sk = SigningKey::from_bytes(&[7u8; 32]);
    let did = [1u8; 32];
    let mut pol = Policy::new();
    pol.allow(did, sk.verifying_key());
    let ph = pol.policy_hash();
    let ref_id = gen_ref_id_raw(&did, &[9u8; 32]);

    // Chain 2 version → head seq=1.
    let v0 = signed(0, [0u8; 32], 100, did, &sk, ph);
    let mut chain = StrataChain::genesis(ref_id, v0, &pol).unwrap();
    let v1 = signed(1, chain.head_version_hash(), 200, did, &sk, ph);
    chain.append_version(v1, &pol).unwrap();

    let anchor = chain.publish_anchor().unwrap();
    let datum = map_anchor_to_datum(&anchor);

    println!("DATUM_JSON={}", datum.to_detailed_json());
    println!("REF_ID={}", hex(&anchor.ref_id));
    println!("HVH={}", hex(&anchor.head_version_hash));
    println!("MMR_ROOT={}", hex(&anchor.mmr_root));
    println!("SEQ={}", anchor.seq);
}
