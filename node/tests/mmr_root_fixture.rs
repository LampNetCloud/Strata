//! Khoá phía Rust cho test-vector chung `apis/mmr-root-vectors.json`.
//!
//! Phía kia là `scripts/verify_mmr_root.py` — bộ kiểm độc lập, không dùng mã Strata/Anchor.
//! Bước CI sinh lại + `diff` bắt bộ sinh đổi mà tệp không đổi; bài này bắt điều ngược lại —
//! MÃ đổi (chain chọn lá khác, crate MMR đổi luật) mà tệp đã commit vẫn nằm nguyên.
//!
//! Hai vế, tách riêng để biết vế nào gãy:
//! 1. `Mmr<Blake3Hasher>` của crate trên đúng các `version_hash` trong tệp ra đúng `mmr_root`;
//! 2. `StrataChain` dựng lại tất định ra đúng các `version_hash` đó và đúng `mmr_root` đó —
//!    tức lá MMR của chain là `version_hash`, không phải thứ khác.

use ed25519_dalek::SigningKey;
use lampnet_merkle_anchor::Blake3Hasher;
use lampnet_merkle_anchor::mmr::Mmr;
use lampnet_strata::state::build_state_root;
use lampnet_strata::version::StrataVersion;
use lampnet_strata::{Policy, StrataChain};
use serde_json::Value;

const FIXTURE: &str = include_str!("../../apis/mmr-root-vectors.json");

/// Số vector TỐI THIỂU — chặn việc "sửa cho xanh" bằng cách rút bớt ca.
const MIN_VECTORS: usize = 16;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn arr32(s: &str) -> [u8; 32] {
    assert_eq!(s.len(), 64, "hash phải 64 hex: {s}");
    let mut out = [0u8; 32];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).expect("hex hợp lệ");
    }
    out
}

fn vectors() -> Vec<Value> {
    let v: Value = serde_json::from_str(FIXTURE).expect("apis/mmr-root-vectors.json phải là JSON");
    v["vectors"].as_array().expect("vectors").clone()
}

/// Cùng tham số với `examples/dump_mmr_root_fixture.rs`.
fn chain(n: u64) -> StrataChain {
    let sk = SigningKey::from_bytes(&[7u8; 32]);
    let did = [0xA1u8; 32];
    let mut pol = Policy::new();
    pol.allow(did, sk.verifying_key());
    let ph = pol.policy_hash();
    let mk = |seq: u64, prev: [u8; 32]| {
        let sr = build_state_root(&[(b"giai_doan".to_vec(), format!("v{seq}").into_bytes())]);
        let cid = format!("cid-{seq}").into_bytes();
        let mut v = StrataVersion::unsigned(seq, prev, cid, sr, did, ph, 1000 + 10 * seq);
        v.sign(&sk);
        v
    };
    let mut c = StrataChain::genesis([0xAB; 32], mk(0, [0u8; 32]), &pol).unwrap();
    for seq in 1..n {
        let v = mk(seq, c.head_version_hash());
        c.append_version(v, &pol).unwrap();
    }
    c
}

#[test]
fn fixture_du_ca_va_co_day_nui_ba_dinh() {
    let vs = vectors();
    assert!(vs.len() >= MIN_VECTORS, "fixture còn {} vector", vs.len());
    for v in &vs {
        let n = v["n"].as_u64().unwrap();
        assert_eq!(
            v["peaks"].as_u64().unwrap(),
            u64::from(n.count_ones()),
            "{}",
            v["name"]
        );
        assert_eq!(
            v["version_hashes"].as_array().unwrap().len() as u64,
            n,
            "{}",
            v["name"]
        );
    }
    // n ≤ 3 (mọi thứ có trên chuỗi hôm nay) chỉ có ≤ 2 đỉnh — fold trái/phải trùng nhau ở đó.
    assert!(
        vs.iter().any(|v| v["peaks"].as_u64().unwrap() >= 3),
        "không có vector ≥ 3 đỉnh ⇒ chiều bag không được kiểm"
    );
}

#[test]
fn mmr_cua_crate_tren_la_trong_tep_ra_dung_root() {
    for v in vectors() {
        let mut m = Mmr::<Blake3Hasher>::new();
        for h in v["version_hashes"].as_array().unwrap() {
            m.append(&arr32(h.as_str().unwrap()));
        }
        assert_eq!(
            hex(&m.root()),
            v["mmr_root"].as_str().unwrap(),
            "{}",
            v["name"]
        );
    }
}

#[test]
fn chain_dung_lai_ra_dung_la_va_dung_root() {
    for v in vectors() {
        let n = v["n"].as_u64().unwrap();
        let c = chain(n);
        let leaves: Vec<String> = (0..n)
            .map(|s| hex(&c.version(s).unwrap().version_hash()))
            .collect();
        let want: Vec<&str> = v["version_hashes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h.as_str().unwrap())
            .collect();
        assert_eq!(leaves, want, "{}: version_hash", v["name"]);
        assert_eq!(
            hex(&c.mmr_root()),
            v["mmr_root"].as_str().unwrap(),
            "{}: mmr_root",
            v["name"]
        );
    }
}
