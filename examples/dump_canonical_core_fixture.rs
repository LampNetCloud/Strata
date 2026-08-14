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
use lampnet_strata::state::{
    TAG_STATE_FVAL, TAG_STATE_LEAF, TAG_STATE_NODE, build_state_root, fval_hash, leaf_hash,
};
use lampnet_strata::version::{StrataVersion, TAG_VER};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Một vector `state_root`: in cả trung gian (`fvh`, `leaf`) chứ không chỉ root.
///
/// Lý do in trung gian: nếu bên phải-khớp chỉ có root thì lúc lệch họ không biết lệch ở
/// tầng nào — `fval` sai, `leaf` sai, hay thứ tự sort sai — và phải đoán. Có `fvh`/`leaf`
/// thì chỗ lệch đầu tiên chỉ thẳng ra tầng hỏng.
fn state_vector(name: &str, why: &str, fields: &[(Vec<u8>, Vec<u8>)]) -> String {
    let mut per_field = Vec::new();
    for (k, v) in fields {
        let fvh = fval_hash(v);
        per_field.push(format!(
            r#"        {{"key": "{}", "key_utf8": {:?}, "key_len": {}, "field_value_bytes": "{}", "fvh": "{}", "leaf": "{}"}}"#,
            hex(k),
            String::from_utf8_lossy(k),
            k.len(),
            hex(v),
            hex(&fvh),
            hex(&leaf_hash(k, &fvh)),
        ));
    }
    format!(
        r#"    {{
      "name": {name:?},
      "why": {why:?},
      "fields_in_given_order": [
{}
      ],
      "state_root": "{}"
    }}"#,
        per_field.join(",\n"),
        hex(&build_state_root(fields)),
    )
}

fn f(k: &str, v: &[u8]) -> (Vec<u8>, Vec<u8>) {
    (k.as_bytes().to_vec(), v.to_vec())
}

/// `(tên, vì sao, các cặp (key, field_value_bytes) theo thứ tự TRUYỀN VÀO)`.
type StateSet = (&'static str, &'static str, Vec<(Vec<u8>, Vec<u8>)>);

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
    println!("  ],");

    // ── state_root (INV-E6, CHỐT-4) ──────────────────────────────────────────
    // Xin bởi `OriLife-Core#161`: `build_state_root` bên đó viết TỪ SPEC, không có vector
    // đối chiếu — khác hẳn `canonical_core` vốn đã có 5 vector khoá. Mà `state_root` là
    // trường #5 của `canonical_core`, tức nó nằm trong `version_hash`, tức nó ĐƯỢC KÝ.
    let cid32: Vec<u8> = (0..32u8)
        .map(|i| i.wrapping_mul(7).wrapping_add(3))
        .collect();

    // Khoá dài đúng 28 byte: tiền ảnh lá = u32_be(4) + key(28) + fvh(32) = 64 byte,
    // bằng ĐÚNG tiền ảnh nút (left 32 + right 32). Khi đó thứ DUY NHẤT ngăn một nút
    // trong bị khai là một lá là hai domain-tag khác nhau. Vector này khoá chỗ đó.
    let key28 = "k".repeat(28);

    let sets: Vec<StateSet> = vec![
        (
            "S1-empty",
            "KHÔNG field nào — root là 32 byte 0, KHÔNG phải hash của chuỗi rỗng",
            vec![],
        ),
        (
            "S2-single",
            "đúng 1 field — root BẰNG leaf, không có tầng nút nào chạy",
            vec![f("age", b"7")],
        ),
        (
            "S3-three-odd-carry",
            "3 lá: số lẻ ⇒ lá cuối CARRY NGUYÊN lên tầng trên, KHÔNG nhân đôi (CVE-2012-2459). Cài sai thành nhân đôi sẽ ra root khác",
            vec![f("a", b"1"), f("b", b"2"), f("c", b"3")],
        ),
        (
            "S4-four-unsorted-input",
            "4 field truyền vào theo thứ tự ĐẢO (d,c,b,a). Sort theo key là bắt buộc trước khi dựng cây, nên bên phải-khớp phải HOÁN VỊ danh sách này theo thứ tự bất kỳ và vẫn ra ĐÚNG root này — cài thiếu bước sort sẽ đỏ ngay",
            vec![f("d", b"4"), f("c", b"3"), f("b", b"2"), f("a", b"1")],
        ),
        (
            "S5-key-28B-leaf-preimage-64B",
            "khoá dài 28 byte ⇒ tiền ảnh lá = 4+28+32 = 64 byte = ĐÚNG tiền ảnh nút. Hai domain-tag khác nhau là thứ DUY NHẤT chặn việc khai một nút trong thành một lá — dùng chung tag ở hai chỗ vẫn cho root hợp lệ mà mất hẳn lớp phòng vệ này",
            vec![f(&key28, b"v"), f("z", b"w")],
        ),
        (
            "S6-cid-value-32B",
            "field mang CID: `field_value_bytes` là CID THUẦN 32 byte ĐÃ GIẢI MÃ, KHÔNG phải 64 ký tự hex ASCII (CHỐT-4 — không class byte, để field-proof không lộ loại). Đây là câu trả lời cho chỗ mập mờ ở OriLife-Core#161",
            vec![f("content", &cid32), f("a", b"1")],
        ),
    ];

    println!(
        r#"  "state_root_note": "INV-E6 / CHỐT-4. fvh = H_dom(tag_fval, field_value_bytes); leaf = H_dom(tag_leaf, u32_be(len(key)) || key || fvh); node = H_dom(tag_node, left || right); root = fold trên các leaf ĐÃ SORT theo key tăng dần, lá lẻ CARRY nguyên (không nhân đôi). Field RỖNG ⇒ root = 32 byte 0. field_value_bytes là giá trị inline HOẶC content_cid THUẦN (đã giải mã), KHÔNG phải chuỗi hex.","#
    );
    println!(r#"  "state_root_hash": {{"#);
    println!(r#"    "tag_fval": "{TAG_STATE_FVAL}","#);
    println!(r#"    "tag_leaf": "{TAG_STATE_LEAF}","#);
    println!(r#"    "tag_node": "{TAG_STATE_NODE}","#);
    println!(
        r#"    "duplicate_key": "KHÔNG HỢP LỆ — bên gọi PHẢI từ chối trước khi vào build_state_root. sort_by của Rust là sort ỔN ĐỊNH nên hai mục trùng key giữ nguyên thứ tự đầu vào ⇒ root ĐỔI THEO THỨ TỰ truyền vào. Xem Strata#39 điểm 2 (DuplicateFieldKey/E6) và OriLife-Core#324 mục B.""#
    );
    println!(r#"  }},"#);
    println!(r#"  "state_root_vectors": ["#);
    let sbody: Vec<String> = sets
        .iter()
        .map(|(n, w, fs)| state_vector(n, w, fs))
        .collect();
    println!("{}", sbody.join(",\n"));
    println!("  ]");
    println!("}}");
}
