//! Sinh `apis/settlement-metadata.json` — test-vector CHUNG cho codec label 1234.
//!
//! Vì sao cần: `submit.ts` (TS) dựng metadatum label 1234 **độc lập** với
//! `settlement.rs` (Rust) — cùng một layout `[{t, a}]` + luật chunk 64B viết hai lần
//! bằng hai ngôn ngữ. Không có gì bắt được khi hai bên trôi khỏi nhau: TS ghi lên chain
//! một hình dạng mà `decode_records` của Rust từ chối, và lỗi chỉ lộ lúc `resolve` thật.
//! (Cùng lớp gap đã vá ở VeDataIO/Core#64 cho anchor metadata label 7368.)
//!
//! Chạy lại khi đổi codec: `cargo run --example dump_settlement_fixture > apis/settlement-metadata.json`
//! Rust là nguồn sinh vì nó giữ **decoder** — bên nào phải khớp thì bên đó không nên tự ra đề.

use lampnet_strata::chain::StrataAnchor;
use lampnet_strata::settlement::{SettlementRecord, encode_records};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Payload tất định — byte thứ i = (7i + 3) mod 256.
fn payload(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 7 + 3) & 0xff) as u8).collect()
}

fn h32(fill: u8) -> [u8; 32] {
    [fill; 32]
}

/// Kỳ vọng cho phía TS, GIỮ NGUYÊN phân biệt giữa hai hình dạng:
///   - `<= 64B` → **một bytestring trần** → JSON là chuỗi `"hex"`;
///   - `> 64B`  → **mảng chunk** → JSON là mảng `["hex", ...]`.
///
/// Không được gộp hai dạng này làm một: decoder Rust từ chối `<=64B mà lại chunk`
/// (chống malleability), nên vector phải nói được sự khác nhau đó.
fn field_json(b: &[u8]) -> String {
    if b.len() <= 64 {
        format!("\"{}\"", hex(b))
    } else {
        let parts: Vec<String> = b.chunks(64).map(|c| format!("\"{}\"", hex(c))).collect();
        format!("[{}]", parts.join(", "))
    }
}

fn anchor_case(name: &str, ref_b: u8, hvh_b: u8, mmr_b: u8, seq: u64) -> String {
    let a = StrataAnchor {
        ref_id: h32(ref_b),
        head_version_hash: h32(hvh_b),
        mmr_root: h32(mmr_b),
        seq,
    };
    let cbor = encode_records(&[SettlementRecord::Anchor(a.clone())]);
    format!(
        r#"    {{
      "name": "{name}",
      "records": [
        {{ "t": 1, "ref_id": "{r}", "head_version_hash": "{h}", "mmr_root": "{m}", "seq": {seq} }}
      ],
      "expected_structure": [
        {{ "t": 1, "a": ["{r}", "{h}", "{m}", {seq}] }}
      ],
      "expected_cbor_hex": "{c}"
    }}"#,
        r = hex(&a.ref_id),
        h = hex(&a.head_version_hash),
        m = hex(&a.mmr_root),
        c = hex(&cbor)
    )
}

fn rotation_case(name: &str, n: usize) -> String {
    let p = payload(n);
    let cbor = encode_records(&[SettlementRecord::KeyRotation(p.clone())]);
    let field = field_json(&p);
    format!(
        r#"    {{
      "name": "{name}",
      "records": [ {{ "t": 2, "payload": "{p}" }} ],
      "expected_structure": [ {{ "t": 2, "a": [{field}] }} ],
      "expected_cbor_hex": "{c}"
    }}"#,
        p = hex(&p),
        c = hex(&cbor)
    )
}

fn multi_case() -> String {
    let a = StrataAnchor {
        ref_id: h32(0xaa),
        head_version_hash: h32(0xbb),
        mmr_root: h32(0xcc),
        seq: 9,
    };
    let p = payload(100);
    let cbor = encode_records(&[
        SettlementRecord::Anchor(a.clone()),
        SettlementRecord::KeyRotation(p.clone()),
    ]);
    let field = field_json(&p);
    format!(
        r#"    {{
      "name": "multi_anchor_plus_rotation",
      "records": [
        {{ "t": 1, "ref_id": "{r}", "head_version_hash": "{h}", "mmr_root": "{m}", "seq": 9 }},
        {{ "t": 2, "payload": "{pl}" }}
      ],
      "expected_structure": [
        {{ "t": 1, "a": ["{r}", "{h}", "{m}", 9] }},
        {{ "t": 2, "a": [{field}] }}
      ],
      "expected_cbor_hex": "{c}"
    }}"#,
        r = hex(&a.ref_id),
        h = hex(&a.head_version_hash),
        m = hex(&a.mmr_root),
        pl = hex(&p),
        c = hex(&cbor)
    )
}

fn main() {
    let cases = [
        anchor_case("anchor_seq_zero", 0x11, 0x22, 0x33, 0),
        anchor_case("anchor_seq_multibyte", 0x44, 0x55, 0x66, 4_294_967_303),
        rotation_case("rotation_63B_single", 63),
        rotation_case("rotation_64B_boundary_single", 64),
        rotation_case("rotation_65B_chunked_64_1", 65),
        rotation_case("rotation_128B_chunked_64_64", 128),
        rotation_case("rotation_129B_chunked_64_64_1", 129),
        multi_case(),
    ];
    println!("{{");
    println!(
        "  \"$schema_note\": \"Test-vector CHUNG cho metadatum label 1234 (Strata Settlement). Sinh bởi `cargo run --example dump_settlement_fixture`. Rust giữ decoder nên Rust ra đề; TS phải khớp `expected_structure`.\","
    );
    println!("  \"label\": 1234,");
    println!(
        "  \"chunk_rule\": \"bytes <= 64 => MỘT bytestring (cấm chunk); > 64 => mảng chunk, mọi chunk trừ cuối đúng 64B\","
    );
    println!("  \"cases\": [");
    println!("{}", cases.join(",\n"));
    println!("  ]");
    println!("}}");
}
