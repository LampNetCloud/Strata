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

// ─────────────────────────────────────────────────────────────────────────────
// Nửa ÂM của fixture — anh Đức chốt hướng (b) ở PR #40.
//
// Tám ca `cases` ở trên toàn là ca dương: chúng chứng minh encoder hai bên sinh ra cùng
// byte. Chúng KHÔNG chứng minh bên nào từ chối được encoding không canonical. Một bản cài
// pass 8/8 vẫn có thể nhận `<=64B mà lại chunk` rồi ghi thứ đó lên chain.
//
// Chừng nào chưa có hai test dưới đây thì gọi fixture là "nguồn sự thật DUY NHẤT" là nói
// quá — nó mới là nguồn sự thật cho nửa dương.
// ─────────────────────────────────────────────────────────────────────────────

/// Số ca âm tối thiểu — chặn việc "sửa test cho xanh" bằng cách rút bớt ca.
const MIN_MUST_REJECT: usize = 6;

#[test]
fn must_reject_thi_decoder_phai_tu_choi() {
    let fx = fixture();
    let cases = fx["must_reject"]
        .as_array()
        .expect("fixture thiếu khối must_reject");
    assert!(
        cases.len() >= MIN_MUST_REJECT,
        "còn {} ca âm (tối thiểu {MIN_MUST_REJECT})",
        cases.len()
    );

    for c in cases {
        let name = c["name"].as_str().unwrap();
        let want = c["expect_error"].as_str().unwrap();
        let bytes = unhex(c["cbor_hex"].as_str().unwrap());

        let err = decode_records(&bytes).err().unwrap_or_else(|| {
            panic!("ca âm `{name}`: decoder CHẤP NHẬN encoding không canonical — xanh giả")
        });

        // So theo TÊN biến thể, không so chuỗi hiển thị: thông điệp lỗi được phép đổi chữ,
        // còn phân loại lỗi thì là hợp đồng với bên đọc.
        let got = match err {
            lampnet_strata::settlement::PayloadError::BadCbor(_) => "BadCbor",
            lampnet_strata::settlement::PayloadError::BadShape(_) => "BadShape",
            lampnet_strata::settlement::PayloadError::BadChunking => "BadChunking",
        };
        assert_eq!(got, want, "ca âm `{name}`: từ chối nhưng SAI phân loại");
    }
}

/// Chiều ngược lại: `t` chưa biết phải BỎ QUA, không được ném lỗi.
///
/// Cài quá nghiêm ở đây cũng hỏng — mọi reader cũ sẽ chết đúng ngày thêm một loại record
/// mới. Ca này giữ cho `must_reject` không bị hiểu thành "cứ lạ là từ chối".
#[test]
fn must_skip_thi_t_la_khong_gay_loi() {
    let fx = fixture();
    for c in fx["must_skip"].as_array().expect("thiếu khối must_skip") {
        let name = c["name"].as_str().unwrap();
        let want = c["expect_records"].as_u64().unwrap() as usize;
        let bytes = unhex(c["cbor_hex"].as_str().unwrap());

        let recs = decode_records(&bytes)
            .unwrap_or_else(|e| panic!("ca `{name}`: `t` lạ KHÔNG được thành lỗi, nhận: {e}"));
        assert_eq!(
            recs.len(),
            want,
            "ca `{name}`: bỏ qua `t` lạ nhưng số record còn lại lệch"
        );
    }
}
