//! Khoá phía Rust cho test-vector chung `apis/canonical-core-vectors.json`.
//!
//! Cùng khuôn `settlement_fixture.rs` (PR #33): **một file vector, hai phía cùng đọc**. Phía
//! kia hiện là `OriLifeTrace/OriLife-Core` — `strata_client.py` cài lại đúng layout này bằng
//! Python. Hai bản cài độc lập cùng một byte-layout mà không có vector chung thì không gì
//! bắt được lúc chúng trôi khỏi nhau, và lỗi chỉ lộ khi chữ ký verify hỏng trên máy chủ thật.
//!
//! Vector do Rust sinh (`cargo run --example dump_canonical_core_fixture`) vì Rust giữ cả
//! encoder lẫn decoder.

use lampnet_strata::hash::h_dom;
use lampnet_strata::version::{CanonicalError, StrataVersion, TAG_VER, parse_canonical_core};
use serde_json::Value;

const FIXTURE: &str = include_str!("../apis/canonical-core-vectors.json");

/// Số vector/negative-control TỐI THIỂU. Chặn việc "sửa test cho xanh" bằng cách rút bớt ca —
/// cùng lý do `settlement_fixture.rs` giữ hằng này.
const MIN_VECTORS: usize = 5;
const MIN_NEGATIVE_CONTROLS: usize = 3;

fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "hex phải chẵn ký tự: {s}");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex hợp lệ"))
        .collect()
}

fn arr32(s: &str) -> [u8; 32] {
    let v = unhex(s);
    assert_eq!(v.len(), 32, "trường cố định phải đúng 32 byte");
    let mut o = [0u8; 32];
    o.copy_from_slice(&v);
    o
}

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("apis/canonical-core-vectors.json phải là JSON hợp lệ")
}

/// Dựng lại `canonical_core` + `version_hash` từ **các trường**, rồi so với chuỗi đã ghi.
///
/// Cố ý dựng từ trường chứ không so hai chuỗi hex với nhau: so hex-với-hex chỉ chứng minh file
/// không đổi, không chứng minh encoder còn sinh ra đúng nó.
#[test]
fn vectors_khop_encoder() {
    let f = fixture();
    let vs = f["vectors"].as_array().expect("mảng vectors");
    assert!(
        vs.len() >= MIN_VECTORS,
        "fixture bị rút còn {} vector (tối thiểu {MIN_VECTORS}) — nếu cố ý thì sửa hằng và nói lý do",
        vs.len()
    );

    for v in vs {
        let name = v["name"].as_str().expect("name");
        let ver = StrataVersion::unsigned(
            v["seq"].as_u64().expect("seq"),
            arr32(v["prev_hash"].as_str().expect("prev_hash")),
            unhex(v["content_cid"].as_str().expect("content_cid")),
            arr32(v["state_root"].as_str().expect("state_root")),
            arr32(v["author_did"].as_str().expect("author_did")),
            arr32(v["policy_hash"].as_str().expect("policy_hash")),
            v["ts"].as_u64().expect("ts"),
        );

        let core = ver.canonical_core();
        let got_core: String = core.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            got_core,
            v["canonical_core"].as_str().expect("canonical_core"),
            "[{name}] canonical_core lệch — encoder đã đổi byte-layout"
        );

        // Độ dài ghi trong file là dữ kiện độc lập: nó bắt được cả trường hợp hex đúng mà
        // công thức `148 + len(content_cid)` bị hiểu sai ở phía đọc.
        assert_eq!(
            core.len() as u64,
            v["canonical_core_len"]
                .as_u64()
                .expect("canonical_core_len"),
            "[{name}] độ dài lệch"
        );
        assert_eq!(
            core.len(),
            148 + unhex(v["content_cid"].as_str().unwrap()).len(),
            "[{name}] phá công thức 148 + len(content_cid)"
        );

        let got_vh: String = h_dom(TAG_VER, &core)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            got_vh,
            v["version_hash"].as_str().expect("version_hash"),
            "[{name}] version_hash lệch — đổi tag, đổi hash nền, hoặc đổi canonical_core"
        );

        // Round-trip qua decoder: encoder và decoder phải nói cùng một thứ tiếng.
        let back =
            parse_canonical_core(&core).expect("[{name}] decoder từ chối chính đầu ra của encoder");
        assert_eq!(
            back.canonical_core(),
            core,
            "[{name}] round-trip không trùng byte"
        );
    }
}

/// Ba ca decoder PHẢI từ chối.
///
/// Lưu ý về một negative control **đã bị loại bỏ có chủ ý**: ca "bỏ length-prefix thì hai
/// `content_cid` khác nhau cho cùng một chuỗi" KHÔNG tồn tại với layout hiện tại —
/// `content_cid` là trường biến độ dài DUY NHẤT, kẹp giữa 40 byte đầu và 104 byte đuôi đều cố
/// định, nên phần giữa luôn khôi phục được từ tổng độ dài. Ca đó từng được nêu nhầm ở
/// `OriLife-Core#161` và đã đính chính. Length-prefix vẫn cần, nhưng để decoder TỪ CHỐI được
/// input hỏng (ba ca dưới) và để mang van trần `<2³²` của §1.7 — không phải để chống nhập nhằng.
#[test]
fn negative_controls_bi_tu_choi() {
    let f = fixture();
    let ncs = f["negative_controls"]
        .as_array()
        .expect("mảng negative_controls");
    assert!(
        ncs.len() >= MIN_NEGATIVE_CONTROLS,
        "còn {} negative control (tối thiểu {MIN_NEGATIVE_CONTROLS})",
        ncs.len()
    );

    for nc in ncs {
        let name = nc["name"].as_str().expect("name");
        let bytes = unhex(nc["bytes"].as_str().expect("bytes"));
        let want = nc["expect_error"].as_str().expect("expect_error");

        let err = parse_canonical_core(&bytes).err().unwrap_or_else(|| {
            panic!("[{name}] decoder CHẤP NHẬN input hỏng — negative control xanh giả")
        });

        let got = match err {
            CanonicalError::Truncated { .. } => "Truncated",
            CanonicalError::LengthOverflow { .. } => "LengthOverflow",
            CanonicalError::TrailingBytes { .. } => "TrailingBytes",
        };
        assert_eq!(got, want, "[{name}] decoder từ chối nhưng SAI lý do");
    }
}
