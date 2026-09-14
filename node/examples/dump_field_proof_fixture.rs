//! Sinh test-vector CHUNG cho field-proof → `apis/field-proof-vectors.json`.
//!
//! **Vì sao Rust ra đề:** Rust giữ cả bên sinh ([`prove_field_salted`]) lẫn bên kiểm
//! ([`verify_field_proof`]). Bên kiểm độc lập (`scripts/verify_field_proof.py`) không nên tự ra
//! đề cho chính mình — cùng khuôn `dump_canonical_core_fixture` và `dump_settlement_fixture`.
//!
//! **Phần `proof` và `version` sinh bằng CHÍNH DTO của daemon** (`FieldProofResp::new`,
//! `VersionDto`), không chép lại hình dạng bằng tay: bản đầu tự dựng JSON và ghi `key` dạng hex,
//! trong khi dây trả `key` là chuỗi UTF-8 — bộ kiểm khớp fixture mà hỏng trên proof thật. Nằm ở
//! crate node vì DTO ở đây.
//!
//! **Mỗi vector đi tới `version_hash`, không dừng ở `state_root`.** Proof chỉ nói *"giá trị
//! này nằm dưới `state_root` này"*; thứ nằm trên chuỗi (label 1234) là `head_version_hash`. Một
//! bên kiểm dừng ở `state_root` thì đang tin `state_root` do daemon khai.
//!
//! **Ca `must_reject` ghi rõ TẦNG phải hỏng** (`fvh` · `state_root` · `version_hash`): bên kiểm
//! từ chối vì một lý do khác lý do đã khai là bên kiểm đang may, không phải đang đúng.
//!
//! Chạy: `cargo run -p lampnet-strata-node --example dump_field_proof_fixture > apis/field-proof-vectors.json`

use lampnet_strata::state::{
    FieldProof, SaltedField, TAG_STATE_FVAL, TAG_STATE_FVAL_SALTED, TAG_STATE_LEAF, TAG_STATE_NODE,
    build_state_root_salted, fval_hash_salted, prove_field_salted,
};
use lampnet_strata::version::{StrataVersion, TAG_VER};
use lampnet_strata_node::dto::{FieldProofResp, VersionDto};
use serde_json::{Value, json};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// CID giả 32 byte, tất định theo nhãn — đúng hình dạng giá trị mà OriLife ghi (CHỐT-4).
fn cid(label: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    let mut x: u32 = 2_166_136_261;
    for i in 0..32u32 {
        for b in label.bytes() {
            x = (x ^ u32::from(b)).wrapping_mul(16_777_619);
        }
        x = (x ^ i).wrapping_mul(16_777_619);
        out.push((x >> 24) as u8);
    }
    out
}

fn plain(k: &str, v: &[u8]) -> SaltedField {
    SaltedField::plain(k.as_bytes().to_vec(), v.to_vec())
}

fn salted(k: &str, v: &[u8], s: &[u8]) -> SaltedField {
    SaltedField::new(k.as_bytes().to_vec(), v.to_vec(), s.to_vec())
}

/// Đúng thân `GET /v1/strata/:ref/proof/field/:key`.
fn proof_json(p: &FieldProof, seq: u64) -> Value {
    serde_json::to_value(FieldProofResp::new(p, seq)).expect("json")
}

/// Đúng object `version` trong thân `GET /v1/strata/:ref/version?at=<ts>`. Trường
/// `version_hash` trong đó là lời khai của daemon — bộ kiểm độc lập không dùng nó.
fn version_json(v: &StrataVersion) -> Value {
    serde_json::to_value(VersionDto::from(v)).expect("json")
}

struct Case {
    name: &'static str,
    why: &'static str,
    fields: Vec<SaltedField>,
    key: &'static str,
    seq: u64,
}

fn build(c: &Case) -> (FieldProof, StrataVersion) {
    let proof =
        prove_field_salted(&c.fields, c.key.as_bytes()).expect("key phải có trong tập trường");
    let v = StrataVersion::unsigned(
        c.seq,
        [0x5a; 32],
        cid(&format!("ho-so/{}", c.name)),
        build_state_root_salted(&c.fields),
        [0x22; 32],
        [0x33; 32],
        1_789_000_000 + c.seq,
    );
    (proof, v)
}

fn main() {
    let cases = vec![
        Case {
            name: "P1-4-truong",
            why: "4 lá chẵn — đường hay gặp nhất: 2 sibling, không carry",
            fields: vec![
                plain("giai_doan", &cid("giai_doan/dau_qua")),
                plain("giong", &cid("giong/ri6")),
                plain("ho_so_chu", &cid("ho_so_chu")),
                plain("da_phun_thuoc", b"co"),
            ],
            key: "giai_doan",
            seq: 2,
        },
        Case {
            name: "P2-3-truong-la-le-carry",
            why: "3 lá, chứng minh lá CUỐI: nó được carry nguyên lên tầng trên nên chỉ có 1 sibling — bên kiểm tự thêm sibling 'nhân đôi' sẽ đỏ ở đây",
            fields: vec![plain("a", b"1"), plain("b", b"2"), plain("c", b"3")],
            key: "c",
            seq: 1,
        },
        Case {
            name: "P3-1-truong",
            why: "1 lá: 0 sibling, state_root BẰNG lá — vòng lặp sibling không chạy lần nào",
            fields: vec![plain("giai_doan", &cid("giai_doan/ra_hoa"))],
            key: "giai_doan",
            seq: 0,
        },
        Case {
            name: "P4-salt-khac-rong",
            why: "trường được chứng minh có salt ⇒ fvh ở miền `fval/salted` (Strata#71); các trường khác trộn có salt và không salt",
            fields: vec![
                salted(
                    "giai_doan",
                    &cid("giai_doan/thu_hoach"),
                    b"salt-ngau-nhien-cho-vector-P4",
                ),
                plain("giong", &cid("giong/monthong")),
                salted("ho_so_chu", &cid("ho_so_chu"), b"salt-khac"),
                plain("vung", b"tien-giang"),
            ],
            key: "giai_doan",
            seq: 3,
        },
        Case {
            name: "P5-5-truong-giua",
            why: "5 lá, chứng minh lá thứ 3 sau sort: sibling đổi chiều giữa các tầng và đi qua một tầng có carry",
            fields: vec![
                plain("e", b"5"),
                plain("d", b"4"),
                plain("c", b"3"),
                plain("b", b"2"),
                plain("a", b"1"),
            ],
            key: "c",
            seq: 4,
        },
    ];

    let mut vectors = Vec::new();
    let mut built = Vec::new();
    for c in &cases {
        let (p, v) = build(c);
        vectors.push(json!({
            "name": c.name,
            "why": c.why,
            "fields": c.fields.iter().map(|f| json!({
                "key": hex(&f.key), "value": hex(&f.value), "salt": hex(&f.salt),
            })).collect::<Vec<_>>(),
            "proof": proof_json(&p, v.seq),
            "version": version_json(&v),
            "version_hash": hex(&v.version_hash()),
        }));
        built.push((p, v));
    }

    // ── must_reject — mỗi ca phá ĐÚNG MỘT chỗ, và khai tầng phải hỏng ────────────────
    let (p1, v1) = &built[0];
    let (p4, v4) = &built[3];
    let mut rejects = Vec::new();
    let mut reject =
        |name: &str, why: &str, stage: &str, p: &FieldProof, v: &StrataVersion, vh: [u8; 32]| {
            rejects.push(json!({
                "name": name, "why": why, "must_fail_at": stage,
                "proof": proof_json(p, v.seq), "version": version_json(v), "version_hash": hex(&vh),
            }));
        };

    let mut r = p1.clone();
    r.value = cid("giai_doan/ra_hoa");
    reject(
        "R1-sua-value-giu-fvh",
        "đổi giá trị khai, giữ fvh cũ — bên kiểm không băm lại value sẽ nhận",
        "fvh",
        &r,
        v1,
        v1.version_hash(),
    );

    let mut r = p1.clone();
    r.value = cid("giai_doan/ra_hoa");
    r.fvh = fval_hash_salted(&r.salt, &r.value);
    reject(
        "R2-sua-value-tinh-lai-fvh",
        "đổi giá trị VÀ tính lại fvh cho khớp — chỉ cây mới bắt được",
        "state_root",
        &r,
        v1,
        v1.version_hash(),
    );

    let mut r = p1.clone();
    r.key = b"giong".to_vec();
    reject(
        "R3-sua-key",
        "giữ value/fvh, khai sang tên trường khác — key nằm trong lá",
        "state_root",
        &r,
        v1,
        v1.version_hash(),
    );

    let mut r = p1.clone();
    r.siblings[0].1 = !r.siblings[0].1;
    reject(
        "R4-dao-chieu-sibling",
        "đảo cờ trái/phải của sibling đầu — bên kiểm bỏ qua cờ chiều sẽ nhận",
        "state_root",
        &r,
        v1,
        v1.version_hash(),
    );

    let mut r = p1.clone();
    r.siblings.pop();
    reject(
        "R5-bot-sibling",
        "bỏ sibling cuối — đường ngắn hơn cây",
        "state_root",
        &r,
        v1,
        v1.version_hash(),
    );

    let mut r = p4.clone();
    r.salt.clear();
    reject(
        "R6-bo-salt-bam-nham-mien",
        "proof có salt mà khai salt rỗng ⇒ fvh phải tính ở miền không salt và lệch (Strata#71)",
        "fvh",
        &r,
        v4,
        v4.version_hash(),
    );

    let mut r = p4.clone();
    r.salt.clear();
    r.fvh = fval_hash_salted(&r.salt, &r.value);
    reject(
        "R7-bo-salt-tinh-lai-fvh",
        "bỏ salt VÀ tính lại fvh không salt — lá đổi nên cây phải lệch",
        "state_root",
        &r,
        v4,
        v4.version_hash(),
    );

    let (p2, _) = &built[1];
    let (_, v5) = &built[4];
    reject(
        "R8-proof-dung-version-khac",
        "proof hợp lệ nhưng ghép với version của một hồ sơ khác: state_root proof đưa vào canonical_core không ra version_hash đã neo",
        "version_hash",
        p2,
        v5,
        v5.version_hash(),
    );

    let mut vv = v1.clone();
    vv.ts += 1;
    reject(
        "R9-version-sua-ts",
        "proof đúng, version khai lệch ts 1 giây so với bản đã neo — version_hash kỳ vọng vẫn là bản neo",
        "version_hash",
        p1,
        &vv,
        v1.version_hash(),
    );

    let out = json!({
        "_note": "Test-vector CHUNG cho field-proof. SINH TỰ ĐỘNG bởi `cargo run -p lampnet-strata-node --example dump_field_proof_fixture` — đừng sửa tay. Bên Rust khoá bằng node/tests/field_proof_fixture.rs; bên kiểm độc lập là scripts/verify_field_proof.py.",
        "rules": {
            "h_dom": "BLAKE3(UTF-8(tag) || 0x00 || x)",
            "fvh_salt_rong": format!("H_dom({TAG_STATE_FVAL}, value)"),
            "fvh_salt_khac_rong": format!("H_dom({TAG_STATE_FVAL_SALTED}, u32_be(len(salt)) || salt || value)"),
            "leaf": format!("H_dom({TAG_STATE_LEAF}, u32_be(len(key)) || key || fvh)"),
            "node": format!("H_dom({TAG_STATE_NODE}, left || right)"),
            "sibling": "[hash, sibling_is_right]: true ⇒ nút hiện tại là con TRÁI ⇒ node(acc, hash); false ⇒ node(hash, acc)",
            "canonical_core": "seq u64_be || prev_hash 32 || u32_be(len(content_cid)) || content_cid || state_root 32 || author_did 32 || policy_hash 32 || ts u64_be",
            "version_hash": format!("H_dom({TAG_VER}, canonical_core) — state_root đưa vào đây là state_root DỰNG LẠI TỪ PROOF"),
        },
        "vectors": vectors,
        "must_reject": rejects,
    });
    println!("{}", serde_json::to_string_pretty(&out).expect("json"));
}
