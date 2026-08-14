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

use ciborium::value::{Integer, Value};
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

// ─────────────────────────────────────────────────────────────────────────────
// Khối `must_reject` + `must_skip` — anh Đức chốt hướng (b) ở PR #40.
//
// Vì sao cần: tám ca ở trên toàn là **ca dương**. Một bản cài TS pass 8/8 vẫn có thể
// **nhận** một encoding không canonical, vì không ca nào ép nó phải TỪ CHỐI thứ gì.
// Chừng nào chưa có khối này thì gọi fixture là "nguồn sự thật DUY NHẤT" là nói quá —
// nó mới là nguồn sự thật cho **nửa dương**.
//
// Cùng hình dạng với `apis/canonical-core-vectors.json` (PR #47): ca dương khoá encoder,
// ca âm khoá decoder. Thiếu nửa nào thì nửa đó trôi tự do.
// ─────────────────────────────────────────────────────────────────────────────

fn cbor_hex(v: &Value) -> String {
    let mut out = Vec::new();
    ciborium::ser::into_writer(v, &mut out).expect("Vec<u8> writer không fail");
    hex(&out)
}

fn t_a(t: u8, a: Vec<Value>) -> Value {
    Value::Map(vec![
        (Value::Text("t".into()), Value::Integer(Integer::from(t))),
        (Value::Text("a".into()), Value::Array(a)),
    ])
}

fn bytes32(fill: u8) -> Value {
    Value::Bytes(vec![fill; 32])
}

fn reject_case(name: &str, why: &str, err: &str, top: Value) -> String {
    format!(
        r#"    {{
      "name": "{name}",
      "why": "{why}",
      "expect_error": "{err}",
      "cbor_hex": "{}"
    }}"#,
        cbor_hex(&top)
    )
}

fn malformed() -> Vec<String> {
    vec![
        // Luật: bytes ≤64B phải là MỘT bytestring trần. Bọc mảng 1 chunk = không canonical.
        reject_case(
            "le64_wrapped_in_array",
            "ref_id 32B bọc trong mảng 1 chunk — <=64B thì CẤM chunk, nếu không thì một giá trị có hai encoding",
            "BadChunking",
            Value::Array(vec![t_a(
                1,
                vec![
                    Value::Array(vec![bytes32(0x11)]),
                    bytes32(0x22),
                    bytes32(0x33),
                    Value::Integer(Integer::from(0u8)),
                ],
            )]),
        ),
        // Luật: mọi chunk TRỪ chunk cuối phải đúng 64B.
        reject_case(
            "middle_chunk_not_64",
            "payload 65B chia [32,33] thay vì [64,1] — chunk giữa khác 64B thì cùng một bytes có nhiều cách chia",
            "BadChunking",
            Value::Array(vec![t_a(
                2,
                vec![Value::Array(vec![
                    Value::Bytes(payload(32)),
                    Value::Bytes(payload(33)),
                ])],
            )]),
        ),
        // Luật: chunk cuối 1..=64B, KHÔNG được rỗng.
        reject_case(
            "last_chunk_empty",
            "chia [64,0] — chunk cuối rỗng là chunk thừa, cùng bytes lại có thêm một encoding",
            "BadChunking",
            Value::Array(vec![t_a(
                2,
                vec![Value::Array(vec![
                    Value::Bytes(payload(64)),
                    Value::Bytes(vec![]),
                ])],
            )]),
        ),
        // Luật: record map phải có ĐÚNG 2 entry (t, a) — chống malleability duplicate-key.
        reject_case(
            "map_three_entries",
            "map có entry thứ ba — parser khác nhau thấy giá trị khác nhau khi map có key lạ/trùng",
            "BadShape",
            Value::Array(vec![Value::Map(vec![
                (Value::Text("t".into()), Value::Integer(Integer::from(1u8))),
                (
                    Value::Text("a".into()),
                    Value::Array(vec![
                        bytes32(0x11),
                        bytes32(0x22),
                        bytes32(0x33),
                        Value::Integer(Integer::from(0u8)),
                    ]),
                ),
                (Value::Text("x".into()), Value::Integer(Integer::from(9u8))),
            ])]),
        ),
        reject_case(
            "record_not_a_map",
            "record là mảng thay vì map — hình dạng sai ngay tầng ngoài",
            "BadShape",
            Value::Array(vec![Value::Array(vec![Value::Integer(Integer::from(1u8))])]),
        ),
        reject_case(
            "bytestring_over_64",
            "một bytestring 65B không chunk — vượt trần bytestring của metadata Cardano",
            "BadChunking",
            Value::Array(vec![t_a(2, vec![Value::Bytes(payload(65))])]),
        ),
    ]
}

/// `t` lạ **KHÔNG** phải lỗi — bỏ qua để giữ forward-compat. Ca này khoá đúng chiều ngược
/// lại của `must_reject`: một bản cài quá nghiêm, ném lỗi khi gặp `t` chưa biết, sẽ làm
/// mọi reader cũ chết ngay ngày thêm loại record mới.
fn skip_case() -> String {
    let top = Value::Array(vec![
        t_a(99, vec![Value::Bytes(payload(4))]),
        t_a(
            1,
            vec![
                bytes32(0x11),
                bytes32(0x22),
                bytes32(0x33),
                Value::Integer(Integer::from(7u8)),
            ],
        ),
    ]);
    format!(
        r#"    {{
      "name": "unknown_t_is_skipped_not_error",
      "why": "t=99 chưa biết phải BỎ QUA, không được ném lỗi (forward-compat); record t=1 sau nó vẫn phải decode ra",
      "expect_records": 1,
      "cbor_hex": "{}"
    }}"#,
        cbor_hex(&top)
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
    println!("  ],");
    println!(
        "  \"must_reject_note\": \"Ca ÂM. Tám ca `cases` ở trên toàn ca dương — một bản cài pass 8/8 vẫn có thể NHẬN encoding không canonical vì không ca nào ép nó từ chối thứ gì. Bên nào đọc metadatum label 1234 PHẢI từ chối đúng các `cbor_hex` dưới đây.\","
    );
    println!("  \"must_reject\": [");
    println!("{}", malformed().join(",\n"));
    println!("  ],");
    println!(
        "  \"must_skip_note\": \"Chiều ngược lại: `t` chưa biết phải BỎ QUA chứ không ném lỗi. Cài quá nghiêm ở đây thì mọi reader cũ chết ngày thêm loại record mới.\","
    );
    println!("  \"must_skip\": [");
    println!("{}", skip_case());
    println!("  ]");
    println!("}}");
}
