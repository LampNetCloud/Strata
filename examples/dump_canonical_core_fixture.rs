//! Sinh test-vector CHUNG cho `canonical_core` → `apis/canonical-core-vectors.json`.
//!
//! **Vì sao Rust ra đề:** Rust giữ cả encoder ([`StrataVersion::canonical_core`]) lẫn decoder
//! ([`parse_canonical_core`]); bên phải-khớp không nên tự ra đề cho chính mình. Cùng khuôn
//! `dump_settlement_fixture` đã dùng cho metadatum label 1234 (PR #33).
//!
//! **Bên tiêu thụ đầu tiên:** `OriLifeTrace/OriLife-Core` — `strata_client.py` cài lại
//! layout này bằng Python để ký version. Hai bản cài độc lập cùng một byte-layout mà không
//! có vector chung thì không gì bắt được lúc chúng trôi khỏi nhau; lỗi chỉ lộ khi chữ ký
//! verify hỏng trên máy chủ thật (`OriLife-Core#161`).
//!
//! Chạy: `cargo run --example dump_canonical_core_fixture > apis/canonical-core-vectors.json`

use lampnet_strata::hash::h_dom;
use lampnet_strata::version::{StrataVersion, TAG_VER};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn vector(name: &str, why: &str, v: &StrataVersion) -> String {
    let c = v.canonical_core();
    format!(
        r#"    {{
      "name": {name:?},
      "why": {why:?},
      "seq": {},
      "prev_hash": "{}",
      "content_cid": "{}",
      "state_root": "{}",
      "author_did": "{}",
      "policy_hash": "{}",
      "ts": {},
      "canonical_core_len": {},
      "canonical_core": "{}",
      "version_hash": "{}"
    }}"#,
        v.seq,
        hex(&v.prev_hash),
        hex(&v.content_cid),
        hex(&v.state_root),
        hex(&v.author_did),
        hex(&v.policy_hash),
        v.ts,
        c.len(),
        hex(&c),
        hex(&h_dom(TAG_VER, &c)),
    )
}

fn main() {
    let sr = [0x11u8; 32];
    let did = [0x22u8; 32];
    let ph = [0x33u8; 32];

    let vs = [
        (
            "V1-genesis-empty-cid",
            "seq=0, prev_hash toàn 0, content_cid RỖNG — biên dưới của trường biến độ dài",
            StrataVersion::unsigned(0, [0u8; 32], vec![], sr, did, ph, 1_700_000_000),
        ),
        (
            "V2-cid-32B",
            "content_cid đúng 32 byte — độ dài hay gặp nhất, dễ nhầm là trường cố định",
            StrataVersion::unsigned(1, [0xaa; 32], vec![0xcd; 32], sr, did, ph, 1_700_000_001),
        ),
        (
            "V3-cid-5B",
            "content_cid độ dài LẺ và ngắn — bắt lỗi cài đặt đệm hoặc bỏ length-prefix",
            StrataVersion::unsigned(2, [0xbb; 32], b"hello".to_vec(), sr, did, ph, 1_700_000_002),
        ),
        (
            "V4-ts-trung-V3",
            "ts TRÙNG V3: hai version cùng một giây là HỢP LỆ (ràng buộc là >=, không phải >) — xem OriLife-Core#168",
            StrataVersion::unsigned(3, [0xcc; 32], b"world".to_vec(), sr, did, ph, 1_700_000_002),
        ),
        (
            "V5-seq-multibyte",
            "seq vượt 1 byte để khoá thứ tự byte u64 big-endian — cài little-endian sẽ đỏ ở đây",
            StrataVersion::unsigned(
                0x0102_0304_0506_0708,
                [0xde; 32],
                b"x".to_vec(),
                sr,
                did,
                ph,
                0xfeed_face,
            ),
        ),
    ];

    println!("{{");
    println!(
        r#"  "_note": "Test-vector CHUNG cho canonical_core (Strata-Math §3.1). SINH TỰ ĐỘNG bởi `cargo run --example dump_canonical_core_fixture` — đừng sửa tay. Bên Rust khoá bằng tests/canonical_core_fixture.rs.","#
    );
    println!(r#"  "hash": {{"#);
    println!(r#"    "algo": "blake3","#);
    println!(r#"    "h_dom": "BLAKE3(tag || 0x00 || x)","#);
    println!(r#"    "tag_ver": "{TAG_VER}","#);
    println!(r#"    "version_hash": "H_dom(tag_ver, canonical_core)""#);
    println!(r#"  }},"#);
    println!(
        r#"  "layout_note": "TLV length-prefix, KHÔNG phải CBOR. sig KHÔNG nằm trong canonical_core (CHỐT-1). Tổng = 148 + len(content_cid).","#
    );
    println!(r#"  "layout": ["#);
    for (i, (f, e, n)) in [
        ("seq", "u64_be", "8"),
        ("prev_hash", "raw", "32"),
        ("len(content_cid)", "u32_be", "4"),
        ("content_cid", "raw", "n"),
        ("state_root", "raw", "32"),
        ("author_did", "raw", "32"),
        ("policy_hash", "raw", "32"),
        ("ts", "u64_be", "8"),
    ]
    .iter()
    .enumerate()
    {
        let comma = if i == 7 { "" } else { "," };
        println!(
            r#"    {{"order": {}, "field": "{f}", "enc": "{e}", "bytes": "{n}"}}{comma}"#,
            i + 1
        );
    }
    println!("  ],");

    println!(r#"  "vectors": ["#);
    let body: Vec<String> = vs.iter().map(|(n, w, v)| vector(n, w, v)).collect();
    println!("{}", body.join(",\n"));
    println!("  ],");

    // ── Negative control ─────────────────────────────────────────────────────
    // KHÔNG dùng ca "bỏ length-prefix thì hai cid đụng nhau" — ca đó KHÔNG tồn tại với layout
    // hiện tại: `content_cid` là trường biến độ dài DUY NHẤT, kẹp giữa 40 byte đầu và 104 byte
    // đuôi đều cố định, nên phần giữa luôn khôi phục được từ tổng độ dài. Ba ca dưới đây là
    // thứ `parse_canonical_core` THẬT SỰ từ chối.
    let base = vs[2].2.canonical_core(); // V3, 153 byte

    let mut truncated = base.clone();
    truncated.truncate(base.len() - 3);

    let mut trailing = base.clone();
    trailing.extend_from_slice(&[0xff, 0xff]);

    let mut overflow = base.clone();
    overflow[40..44].copy_from_slice(&u32::MAX.to_be_bytes());

    println!(r#"  "negative_controls": ["#);
    println!(
        r#"    {{
      "name": "NC1-truncated",
      "why": "cắt cụt 3 byte cuối — decoder phải báo Truncated, KHÔNG được đọc bừa",
      "bytes": "{}",
      "expect_error": "Truncated"
    }},
    {{
      "name": "NC2-trailing-bytes",
      "why": "thừa 2 byte ở đuôi — decoder phải báo TrailingBytes, KHÔNG được bỏ qua im lặng",
      "bytes": "{}",
      "expect_error": "TrailingBytes"
    }},
    {{
      "name": "NC3-length-overflow",
      "why": "len(content_cid) khai u32::MAX — decoder phải báo LengthOverflow, KHÔNG được cấp phát theo số khai",
      "bytes": "{}",
      "expect_error": "LengthOverflow"
    }}"#,
        hex(&truncated),
        hex(&trailing),
        hex(&overflow),
    );
    println!("  ]");
    println!("}}");
}
