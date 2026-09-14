//! Khoá phía Rust cho test-vector chung `apis/field-proof-vectors.json`.
//!
//! Cùng khuôn `tests/canonical_core_fixture.rs` của crate lõi: **một file vector, hai phía cùng
//! đọc**. Phía kia là `scripts/verify_field_proof.py` — bộ kiểm độc lập, không dùng mã Strata.
//!
//! Nằm ở crate node vì phần `proof`/`version` của fixture phải đúng **hình dạng dây**: bài dưới
//! so nguyên object `FieldProofResp::new` với fixture, nên đổi DTO (vd `key` từ UTF-8 sang hex)
//! mà không sinh lại fixture là đỏ.
//!
//! Dựng lại từ **tập trường**, không so hex-với-hex: so chuỗi với chuỗi chỉ chứng minh tệp không
//! đổi, không chứng minh `prove_field_salted` còn sinh ra đúng nó.

use lampnet_strata::state::{
    FieldProof, SaltedField, build_state_root_salted, fval_hash_salted, prove_field_salted,
    verify_field_proof,
};
use lampnet_strata::version::StrataVersion;
use lampnet_strata_node::dto::FieldProofResp;
use serde_json::Value;

const FIXTURE: &str = include_str!("../../apis/field-proof-vectors.json");

/// Số ca TỐI THIỂU — chặn việc "sửa cho xanh" bằng cách rút bớt ca.
const MIN_VECTORS: usize = 5;
const MIN_REJECTS: usize = 9;

fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "hex phải chẵn ký tự: {s}");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex hợp lệ"))
        .collect()
}

fn arr32(s: &str) -> [u8; 32] {
    unhex(s)
        .try_into()
        .expect("trường cố định phải đúng 32 byte")
}

fn fixture() -> Value {
    serde_json::from_str(FIXTURE).expect("apis/field-proof-vectors.json phải là JSON hợp lệ")
}

fn proof_of(p: &Value) -> FieldProof {
    FieldProof {
        // Dây: `key` là chuỗi UTF-8, mọi trường băm khác là hex (`FieldProofResp::new`).
        key: p["key"].as_str().unwrap().as_bytes().to_vec(),
        value: unhex(p["value"].as_str().unwrap()),
        salt: unhex(p["salt"].as_str().unwrap()),
        fvh: arr32(p["fvh"].as_str().unwrap()),
        siblings: p["siblings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| (arr32(s[0].as_str().unwrap()), s[1].as_bool().unwrap()))
            .collect(),
        state_root: arr32(p["state_root"].as_str().unwrap()),
    }
}

/// `version_hash` dựng từ các trường version, với `state_root` **lấy từ proof**.
fn vh_with_root(v: &Value, state_root: [u8; 32]) -> [u8; 32] {
    StrataVersion::unsigned(
        v["seq"].as_u64().unwrap(),
        arr32(v["prev_hash"].as_str().unwrap()),
        unhex(v["content_cid"].as_str().unwrap()),
        state_root,
        arr32(v["author_did"].as_str().unwrap()),
        arr32(v["policy_hash"].as_str().unwrap()),
        v["ts"].as_u64().unwrap(),
    )
    .version_hash()
}

#[test]
fn vectors_khop_prove_field_va_version_hash() {
    let f = fixture();
    let vs = f["vectors"].as_array().expect("mảng vectors");
    assert!(
        vs.len() >= MIN_VECTORS,
        "fixture bị rút còn {} vector",
        vs.len()
    );

    for v in vs {
        let name = v["name"].as_str().unwrap();
        let fields: Vec<SaltedField> = v["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| {
                SaltedField::new(
                    unhex(x["key"].as_str().unwrap()),
                    unhex(x["value"].as_str().unwrap()),
                    unhex(x["salt"].as_str().unwrap()),
                )
            })
            .collect();
        let want = proof_of(&v["proof"]);

        let got = prove_field_salted(&fields, &want.key).expect("key có trong tập trường");
        assert_eq!(
            got, want,
            "{name}: prove_field_salted không còn sinh ra proof trong fixture"
        );
        let seq = v["version"]["seq"].as_u64().unwrap();
        assert_eq!(
            serde_json::to_value(FieldProofResp::new(&got, seq)).unwrap(),
            v["proof"],
            "{name}: hình dạng dây của proof lệch fixture — sinh lại fixture"
        );
        assert!(
            verify_field_proof(&want),
            "{name}: proof trong fixture không verify"
        );
        assert_eq!(
            build_state_root_salted(&fields),
            want.state_root,
            "{name}: state_root dựng từ tập trường lệch proof"
        );
        assert_eq!(
            arr32(v["version"]["state_root"].as_str().unwrap()),
            want.state_root,
            "{name}: version.state_root lệch proof.state_root"
        );
        assert_eq!(
            vh_with_root(&v["version"], want.state_root),
            arr32(v["version_hash"].as_str().unwrap()),
            "{name}: version_hash lệch"
        );
    }
}

/// Mỗi ca phải hỏng ĐÚNG tầng đã khai — hỏng ở tầng khác nghĩa là ca đó không đo thứ nó nói.
#[test]
fn must_reject_hong_dung_tang() {
    let f = fixture();
    let rs = f["must_reject"].as_array().expect("mảng must_reject");
    assert!(
        rs.len() >= MIN_REJECTS,
        "fixture bị rút còn {} ca từ chối",
        rs.len()
    );

    for r in rs {
        let name = r["name"].as_str().unwrap();
        let p = proof_of(&r["proof"]);
        let fvh_ok = fval_hash_salted(&p.salt, &p.value) == p.fvh;
        let tree_ok = verify_field_proof(&p);
        let vh_ok =
            vh_with_root(&r["version"], p.state_root) == arr32(r["version_hash"].as_str().unwrap());

        let stage = match (fvh_ok, tree_ok, vh_ok) {
            (false, _, _) => "fvh",
            (true, false, _) => "state_root",
            (true, true, false) => "version_hash",
            (true, true, true) => "khong_hong",
        };
        assert_eq!(
            stage,
            r["must_fail_at"].as_str().unwrap(),
            "{name}: hỏng sai tầng"
        );
    }
}
