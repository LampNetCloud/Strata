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
use lampnet_strata::state::{
    TAG_STATE_FVAL, TAG_STATE_FVAL_SALTED, TAG_STATE_NODE, build_state_root, fval_hash,
    fval_hash_salted, leaf_hash,
};
use lampnet_strata::version::{CanonicalError, StrataVersion, TAG_VER, parse_canonical_core};
use serde_json::Value;

const FIXTURE: &str = include_str!("../apis/canonical-core-vectors.json");

/// Số vector/negative-control TỐI THIỂU. Chặn việc "sửa test cho xanh" bằng cách rút bớt ca —
/// cùng lý do `settlement_fixture.rs` giữ hằng này.
const MIN_VECTORS: usize = 5;
const MIN_NEGATIVE_CONTROLS: usize = 3;
const MIN_STATE_VECTORS: usize = 6;
const MIN_MODE_VECTORS: usize = 4;
const MIN_MODE_NEGATIVE_CONTROLS: usize = 2;

fn unhex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "hex phải chẵn ký tự: {s}");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex hợp lệ"))
        .collect()
}

fn hexs(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
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

/// Khoá `state_root` (INV-E6, CHỐT-4) — dựng lại từ **các trường**, không so hex-với-hex.
///
/// Xin bởi `OriLife-Core#161`: bên đó cài `build_state_root` **từ spec, không có vector đối
/// chiếu**, khác hẳn `canonical_core` vốn đã có vector khoá. Mà `state_root` là trường #5 của
/// `canonical_core` ⇒ nằm trong `version_hash` ⇒ **được ký**. Lệch ở đây là ký sai vĩnh viễn.
#[test]
fn state_root_vectors_khop_builder() {
    let f = fixture();
    let svs = f["state_root_vectors"]
        .as_array()
        .expect("mảng state_root_vectors");
    assert!(
        svs.len() >= MIN_STATE_VECTORS,
        "còn {} state vector (tối thiểu {MIN_STATE_VECTORS})",
        svs.len()
    );

    for sv in svs {
        let name = sv["name"].as_str().expect("name");
        let items = sv["fields_in_given_order"]
            .as_array()
            .expect("fields_in_given_order");

        let mut fields: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        for it in items {
            let key = unhex(it["key"].as_str().expect("key"));
            let val = unhex(it["field_value_bytes"].as_str().expect("field_value_bytes"));

            // Trung gian phải khớp từng tầng: sai ở đâu thì chỉ đúng tầng đó, không phải đoán.
            let fvh = fval_hash(&val);
            assert_eq!(
                hexs(&fvh),
                it["fvh"].as_str().expect("fvh"),
                "[{name}] fvh lệch — tầng H_dom(tag_fval, field_value_bytes)"
            );
            assert_eq!(
                hexs(&leaf_hash(&key, &fvh)),
                it["leaf"].as_str().expect("leaf"),
                "[{name}] leaf lệch — tầng u32_be(len(key)) ‖ key ‖ fvh"
            );
            assert_eq!(
                key.len(),
                it["key_len"].as_u64().expect("key_len") as usize,
                "[{name}] key_len trong file không khớp key"
            );

            fields.push((key, val));
        }

        let want = sv["state_root"].as_str().expect("state_root");
        assert_eq!(
            hexs(&build_state_root(&fields)),
            want,
            "[{name}] state_root lệch"
        );

        // Sort theo key là BẮT BUỘC ⇒ hoán vị đầu vào không được đổi root.
        // Đảo ngược là đủ để bắt bản cài quên sort (và vector S4 vốn đã truyền vào thứ tự đảo).
        let mut rev = fields.clone();
        rev.reverse();
        assert_eq!(
            hexs(&build_state_root(&rev)),
            want,
            "[{name}] root ĐỔI khi đảo thứ tự đầu vào — bước sort theo key bị thiếu"
        );
    }
}

/// Lá và nút phải nằm ở **hai miền băm khác nhau** — lớp phòng vệ mà `OriLife-Core#324` mục A
/// chỉ ra là "đang đúng nhưng không có bài kiểm nào giữ".
///
/// Với khoá dài **28 byte**, tiền ảnh lá = `u32_be(4) + key(28) + fvh(32)` = **64 byte**, bằng
/// đúng tiền ảnh nút = `left(32) + right(32)`. Khi đó thứ DUY NHẤT ngăn một nút trong bị khai
/// là một lá là hai domain-tag khác nhau. Dùng chung tag ở hai chỗ vẫn cho root "hợp lệ", nên
/// không có test này thì ai dọn mã gộp hai dòng lại sẽ thấy CI xanh.
#[test]
fn leaf_va_node_khong_dung_chung_mien_bam() {
    let key28 = vec![b'k'; 28];
    let fvh = fval_hash(b"v");

    let mut preimage = Vec::with_capacity(64);
    preimage.extend_from_slice(&(key28.len() as u32).to_be_bytes());
    preimage.extend_from_slice(&key28);
    preimage.extend_from_slice(&fvh);
    assert_eq!(
        preimage.len(),
        64,
        "tiền đề của test: khoá 28 byte cho tiền ảnh lá đúng 64 byte"
    );

    // Cùng 64 byte đó, đọc như một nút (hai nửa 32 byte).
    let as_node = h_dom(TAG_STATE_NODE, &preimage);
    let as_leaf = leaf_hash(&key28, &fvh);

    assert_ne!(
        hexs(&as_leaf),
        hexs(&as_node),
        "lá và nút băm ra CÙNG giá trị trên cùng 64 byte tiền ảnh — domain separation đã mất, \
         một nút trong khai được thành một lá"
    );
}

/// Vector CHẾ ĐỘ `fvh` (`Strata#71`) — hai chế độ, hai domain-tag.
///
/// Bên phải-khớp (`strata_client.py`) hôm nay chỉ cài đường **không salt**. Vector này là thứ
/// **duy nhất** nói cho họ biết chế độ làm mù tồn tại, nó chọn miền băm bằng `salt` rỗng hay
/// không, và byte-layout của nó là gì. Thiếu nó thì họ không cài sai — họ **không cài**, rồi
/// mọi proof có salt đỏ ở phía client trong khi server nhìn không có gì sai.
#[test]
fn fval_mode_vectors_khop_lo() {
    let fx = fixture();
    let vs = fx["fval_mode_vectors"]
        .as_array()
        .expect("fval_mode_vectors");
    assert!(
        vs.len() >= MIN_MODE_VECTORS,
        "còn {} mode vector (tối thiểu {MIN_MODE_VECTORS})",
        vs.len()
    );

    let h = &fx["fval_mode_hash"];
    assert_eq!(
        h["tag_fval"].as_str().expect("tag_fval"),
        TAG_STATE_FVAL,
        "tag không-salt trong file lệch hằng của lõi"
    );
    assert_eq!(
        h["tag_fval_salted"].as_str().expect("tag_fval_salted"),
        TAG_STATE_FVAL_SALTED,
        "tag làm mù trong file lệch hằng của lõi"
    );
    assert_ne!(
        TAG_STATE_FVAL, TAG_STATE_FVAL_SALTED,
        "hai chế độ dùng CHUNG tag — đây chính là lỗ P0 Strata#71"
    );

    for v in vs {
        let name = v["name"].as_str().expect("name");
        let salt = unhex(v["salt"].as_str().expect("salt"));
        let value = unhex(v["value"].as_str().expect("value"));

        assert_eq!(
            salt.len(),
            v["salt_len"].as_u64().expect("salt_len") as usize,
            "[{name}] salt_len trong file không khớp salt"
        );

        // Chế độ do salt QUYẾT, không phải một nhãn ghi tay cạnh nó.
        let (want_mode, want_tag) = if salt.is_empty() {
            ("khong_salt", TAG_STATE_FVAL)
        } else {
            ("salted", TAG_STATE_FVAL_SALTED)
        };
        assert_eq!(
            v["mode"].as_str().expect("mode"),
            want_mode,
            "[{name}] nhãn mode lệch với salt thật"
        );
        assert_eq!(
            v["tag"].as_str().expect("tag"),
            want_tag,
            "[{name}] tag lệch với chế độ"
        );

        assert_eq!(
            hexs(&fval_hash_salted(&salt, &value)),
            v["fvh"].as_str().expect("fvh"),
            "[{name}] fvh lệch"
        );

        // salt RỖNG phải rơi về ĐÚNG đường cũ — điều kiện "không lịch sử nào phải tính lại".
        if salt.is_empty() {
            assert_eq!(
                fval_hash_salted(&salt, &value),
                fval_hash(&value),
                "[{name}] salt rỗng KHÔNG còn trùng bit với fval_hash — state_root đã ký sẽ đổi"
            );
        }
    }
}

/// Đối chứng âm cho `fval_mode_vectors` — dựng lại đúng khai thác của `Strata#71`.
///
/// Không có mục này thì một bản cài dùng **chung** một tag cho hai chế độ vẫn khớp toàn bộ
/// `fval_mode_vectors` ở trên: mọi vector đều đi một chiều (tính `fvh` từ `(salt, value)`),
/// nên không vector thuận nào phát hiện được rằng hai miền đã chồng lên nhau.
#[test]
fn fval_mode_negative_controls_phai_khac_mien() {
    let fx = fixture();
    let ncs = fx["fval_mode_negative_controls"]
        .as_array()
        .expect("fval_mode_negative_controls");
    assert!(
        ncs.len() >= MIN_MODE_NEGATIVE_CONTROLS,
        "còn {} đối chứng âm (tối thiểu {MIN_MODE_NEGATIVE_CONTROLS})",
        ncs.len()
    );

    // NC1 — hai CHẾ ĐỘ.
    let nc1 = &ncs[0];
    let salt = unhex(nc1["salt"].as_str().expect("salt"));
    let value_salted = unhex(nc1["value_salted"].as_str().expect("value_salted"));

    // `V` được DỰNG LẠI ở đây, không đọc từ file: nếu file ghi một `V` khác thì nó không còn
    // là khai thác nữa và đối chứng này thành trang trí.
    let mut v = Vec::new();
    v.extend_from_slice(&(salt.len() as u32).to_be_bytes());
    v.extend_from_slice(&salt);
    v.extend_from_slice(&value_salted);
    assert_eq!(
        hexs(&v),
        nc1["value_khong_salt"].as_str().expect("value_khong_salt"),
        "NC1: value_khong_salt trong file KHÔNG phải u32_be(|S|) ‖ S ‖ M — đối chứng đã hỏng"
    );

    let a = fval_hash_salted(&salt, &value_salted);
    let b = fval_hash(&v);
    assert_eq!(hexs(&a), nc1["fvh_salted"].as_str().expect("fvh_salted"));
    assert_eq!(
        hexs(&b),
        nc1["fvh_khong_salt_cua_V"]
            .as_str()
            .expect("fvh_khong_salt_cua_V")
    );
    assert_ne!(
        a, b,
        "NC1: hai CHẾ ĐỘ cho cùng một fvh ⇒ người ghi đổi được lời khai về giá trị SAU KHI ký"
    );

    // NC2 — biên salt/value bên trong nhánh có salt.
    let nc2 = &ncs[1];
    let (sa, va) = (
        unhex(nc2["a"]["salt"].as_str().expect("a.salt")),
        unhex(nc2["a"]["value"].as_str().expect("a.value")),
    );
    let (sb, vb) = (
        unhex(nc2["b"]["salt"].as_str().expect("b.salt")),
        unhex(nc2["b"]["value"].as_str().expect("b.value")),
    );
    let mut cat_a = sa.clone();
    cat_a.extend_from_slice(&va);
    let mut cat_b = sb.clone();
    cat_b.extend_from_slice(&vb);
    assert_eq!(
        cat_a, cat_b,
        "NC2 chỉ có nghĩa khi hai cặp nối trần ra CÙNG một chuỗi"
    );

    let fa = fval_hash_salted(&sa, &va);
    let fb = fval_hash_salted(&sb, &vb);
    assert_eq!(hexs(&fa), nc2["a"]["fvh"].as_str().expect("a.fvh"));
    assert_eq!(hexs(&fb), nc2["b"]["fvh"].as_str().expect("b.fvh"));
    assert_ne!(fa, fb, "NC2: thiếu length-prefix cho salt — biên dịch được");
}
