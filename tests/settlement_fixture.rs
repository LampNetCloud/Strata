//! Đối chiếu codec label 1234 với test-vector CHUNG `apis/settlement-metadata.json`.
//!
//! Vì sao có test này: `anchor-io/submitter/submit.ts` dựng metadatum label 1234 **độc
//! lập** với `settlement.rs` — cùng layout `[{t,a}]` + luật chunk 64B, viết hai lần bằng
//! hai ngôn ngữ. Trước đây KHÔNG có gì bắt được lúc hai bên trôi khỏi nhau: TS ghi lên
//! chain một hình dạng mà `decode_records` từ chối, và lỗi chỉ lộ khi `resolve` thật.
//!
//! Test này khoá phía Rust; `npm run test:fixture` bên submitter khoá phía TS trên CÙNG
//! file vector. Đổi codec mà quên một bên ⇒ một trong hai đỏ.

use lampnet_strata::chain::StrataAnchor;
use lampnet_strata::settlement::{SettlementRecord, decode_records, encode_records};
use serde_json::Value;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex hợp lệ"))
        .collect()
}

fn h32(s: &str) -> [u8; 32] {
    unhex(s).try_into().expect("phải đúng 32 byte")
}

fn record_from_json(r: &Value) -> SettlementRecord {
    match r["t"].as_u64().expect("t") {
        1 => SettlementRecord::Anchor(StrataAnchor {
            ref_id: h32(r["ref_id"].as_str().unwrap()),
            head_version_hash: h32(r["head_version_hash"].as_str().unwrap()),
            mmr_root: h32(r["mmr_root"].as_str().unwrap()),
            seq: r["seq"].as_u64().expect("seq"),
        }),
        2 => SettlementRecord::KeyRotation(unhex(r["payload"].as_str().unwrap())),
        other => panic!("t không hỗ trợ trong vector: {other}"),
    }
}

fn fixture() -> Value {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/apis/settlement-metadata.json"
    ))
    .expect("đọc apis/settlement-metadata.json");
    serde_json::from_str(&raw).expect("vector là JSON hợp lệ")
}

/// Encode phía Rust phải ra ĐÚNG byte ghi trong vector — đây là cái TS phải khớp.
#[test]
fn encode_matches_shared_fixture() {
    let fx = fixture();
    assert_eq!(
        fx["label"].as_u64(),
        Some(1234),
        "label vector phải là 1234"
    );
    let cases = fx["cases"].as_array().expect("cases");
    assert!(!cases.is_empty(), "vector rỗng thì test này vô nghĩa");

    for c in cases {
        let name = c["name"].as_str().unwrap();
        let recs: Vec<SettlementRecord> = c["records"]
            .as_array()
            .expect("records")
            .iter()
            .map(record_from_json)
            .collect();
        let got = hex(&encode_records(&recs));
        let want = c["expected_cbor_hex"].as_str().expect("expected_cbor_hex");
        assert_eq!(got, want, "case `{name}`: CBOR lệch vector chung");
    }
}

/// Vector phải decode ngược ra đúng record — chặn trường hợp vector tự nó sai.
#[test]
fn fixture_round_trips_through_decoder() {
    for c in fixture()["cases"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let recs: Vec<SettlementRecord> = c["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(record_from_json)
            .collect();
        let bytes = unhex(c["expected_cbor_hex"].as_str().unwrap());
        let back = decode_records(&bytes)
            .unwrap_or_else(|e| panic!("case `{name}`: decoder từ chối chính vector: {e}"));
        assert_eq!(back, recs, "case `{name}`: round-trip lệch");
    }
}

/// Biên chunk 64B phải có mặt trong vector — nếu ai đó rút gọn vector thì test này kêu.
/// 64B (cấm chunk) và 65B (phải chunk 64+1) là hai ca dễ trôi nhất giữa hai ngôn ngữ.
#[test]
fn fixture_covers_chunk_boundary() {
    let fx = fixture();
    let names: Vec<&str> = fx["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    for must in ["rotation_64B_boundary_single", "rotation_65B_chunked_64_1"] {
        assert!(names.contains(&must), "vector thiếu ca biên `{must}`");
    }
}
