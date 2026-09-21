//! Sinh test-vector CHUNG cho `mmr_root` → `apis/mmr-root-vectors.json`.
//!
//! **Đi qua đường sản xuất, không qua `Mmr` trần.** Mỗi vector là một `StrataChain` thật n
//! version (ký bằng khoá tất định), và `mmr_root` lấy từ [`StrataChain::mmr_root`] — thứ daemon
//! trả ở `GET /head` và thứ neo lên label 1234. Sinh bằng `Mmr` trực tiếp thì fixture không bắt
//! được chuyện chain đổi thứ làm lá (vd lá = `canonical_core` thay vì `version_hash`).
//!
//! **n = 1..16** để phủ mọi hình dạng dãy núi tới 4 đỉnh. Dữ liệu chuỗi hôm nay chỉ có n ≤ 3 —
//! tối đa 2 đỉnh — mà ở đó bag fold-left và fold-right **ra cùng kết quả**; chiều bag chỉ kiểm
//! được khi có ≥ 3 đỉnh (n = 7, 11, 13, 14, 15).
//!
//! Chạy: `cargo run -p lampnet-strata-node --example dump_mmr_root_fixture > apis/mmr-root-vectors.json`

use ed25519_dalek::SigningKey;
use lampnet_strata::state::build_state_root;
use lampnet_strata::version::StrataVersion;
use lampnet_strata::{Policy, StrataChain};
use serde_json::{Value, json};

const MAX_N: u64 = 16;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Chain n version, tất định: khoá `[7; 32]`, `author_did = [0xA1; 32]`, ts 1000 + 10·seq, mỗi
/// version một giá trị trường khác nhau (lá MMR khác nhau ⇒ đảo thứ tự lá là phép thử có nghĩa).
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
    let mut c = StrataChain::genesis([0xAB; 32], mk(0, [0u8; 32]), &pol).expect("genesis hợp lệ");
    for seq in 1..n {
        let v = mk(seq, c.head_version_hash());
        c.append_version(v, &pol).expect("append hợp lệ");
    }
    c
}

fn main() {
    let vectors: Vec<Value> = (1..=MAX_N)
        .map(|n| {
            let c = chain(n);
            let leaves: Vec<String> = (0..n)
                .map(|s| hex(&c.version(s).expect("seq có").version_hash()))
                .collect();
            json!({
                "name": format!("n{n}"),
                "n": n,
                "peaks": n.count_ones(),
                "version_hashes": leaves,
                "mmr_root": hex(&c.mmr_root()),
            })
        })
        .collect();
    let out = json!({
        "about": "mmr_root của StrataChain n version (n = 1..16). Sinh bởi node/examples/dump_mmr_root_fixture.rs; bên kiểm độc lập: scripts/verify_mmr_root.py.",
        "rule": [
            "H_dom(tag, x) = BLAKE3(UTF-8(tag) ‖ 0x00 ‖ x)",
            "lá   = H_dom(\"LN/STRATA/mmr/leaf/v1\", 0x00 ‖ version_hash)   — theo seq 0..n-1",
            "nút  = H_dom(\"LN/STRATA/mmr/node/v1\", 0x01 ‖ trái ‖ phải)",
            "đỉnh = gốc cây con hoàn hảo theo khai triển nhị phân của n, lớn→nhỏ; không nhân đôi lá lẻ",
            "bag  = fold-right: bag = p_m; j = m-1..1: bag = nút(p_j, bag)",
            "root = H_dom(\"LN/STRATA/mmr/root/v1\", u64_be(n) ‖ bag)"
        ],
        "vectors": vectors,
    });
    println!("{}", serde_json::to_string_pretty(&out).expect("JSON"));
}
