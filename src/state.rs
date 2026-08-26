//! `state_root` field-level + field-proof (INV-E6, CHỐT-4).
//!
//! Khối "Mã hóa state leaf" của `_CONTRACT.md`:
//! ```text
//! fvh  = H_dom("LN/STRATA/state/fval/v1", field_value_bytes)
//! leaf = H_dom("LN/STRATA/state/leaf/v1", u32_be(len(key)) ‖ key ‖ fvh)
//! node = H_dom("LN/STRATA/state/node/v1", left ‖ right)
//! state_root = node-root trên các leaf đã sort theo key tăng dần
//! ```
//! `field_value_bytes` có thể là giá trị inline HOẶC content_cid THUẦN (CHỐT-4 —
//! KHÔNG class byte, để field-proof không leak loại).
//!
//! Cây dùng tag RIÊNG (TAG_STATE_*), KHÔNG đụng tag MMR. Dup-leaf guard: số lá lẻ
//! KHÔNG copy lá cuối — carry lên tầng trên (đối xứng với MMR carry, INV-E8).
//! Proof một trường chỉ lộ `value_cid + fvh + sibling-hash`; KHÔNG lộ key/value
//! trường khác (sibling đã băm).
//!
//! # Làm mù (blinding) — `Strata-Math.md` §6.3
//!
//! `Strata-Math.md:292` **không đòi** salt: nó xếp blinding vào *"giải pháp khi cần
//! giấu cả số trường và chống so-khớp"*, và đóng bằng *"đánh đổi có chủ đích… khi cần
//! kín hơn thì bật padding + blinding"*. Tức đây là **tuỳ chọn có điều kiện**.
//!
//! **Nhưng hồ sơ cây là đúng điều kiện kích hoạt.** Các trường *"đã phun thuốc:
//! có/không"*, *"giai đoạn: ra hoa"* có **miền nhỏ**: `fvh` của chúng dò cạn được bằng
//! vét cạn tiền ảnh, và §6.3 còn nêu thêm chuyện so-khớp *"có đổi / không đổi"* giữa
//! hai proof cùng vị trí. Trước đợt này [`fval_hash`] **không nhận salt**, nên lối
//! thoát mà spec chừa sẵn **không có cách nào bật lên**. Vấn đề không phải *"code lệch
//! spec"*; vấn đề là một tuỳ chọn không xây được thì trên thực tế là **không tồn tại**.
//!
//! ## Một chỗ đi khác câu chữ spec — và vì sao
//!
//! §6.3 viết `fvh_i = H_dom(TAG, salt_i ‖ field_value_bytes)`. Nối trần như vậy là
//! **nhập nhằng biên**: `(salt="ab", value="c")` và `(salt="a", value="bc")` cho **cùng
//! một `fvh`**. Mà cả hai đều do **người ghi** chọn, còn `FieldProof` thì công khai
//! `salt` + `value` để verifier băm lại ⇒ người ghi có thể **đổi lời khai về giá trị**
//! sau khi đã ký, chỉ bằng cách dịch biên. Đúng miền dữ liệu đang cần blinding (chuỗi
//! ngắn, ít entropy) thì việc dựng hai cặp cùng có nghĩa là dễ.
//!
//! ⇒ Ở đây salt được **length-prefix**: `u32_be(len(salt)) ‖ salt ‖ value`. Salt RỖNG
//! giữ **nguyên xi** dạng cũ (`H_dom(TAG, value)`) nên mọi `state_root` đã ký từ trước
//! **không đổi một bit**. Đây là điểm cần anh Đức xác nhận vào spec — ghi ra ở đây
//! thay vì sửa lặng lẽ.
//!
//! ## Cái đợt này CHƯA làm, nói thẳng
//!
//! Lõi đã **bật được** blinding. Chưa có: daemon lưu salt theo từng version, schema
//! HTTP mang salt, và chỗ chọn-tham-gia theo chính sách trường. Chừng nào chưa có ba
//! thứ đó thì blinding **chưa chạy trong sản xuất** — chỉ là đã có đường để bật.

use crate::u32_be;
use lampnet_merkle_anchor::hash::{Hash32, h_dom};

/// Tag băm giá trị trường (CHỐT-2).
pub const TAG_STATE_FVAL: &str = "LN/STRATA/state/fval/v1";

/// Tag cho `fvh` dạng **làm mù** — miền RIÊNG, không dùng chung với [`TAG_STATE_FVAL`].
///
/// Dùng chung một tag cho cả hai chế độ là một lỗ **P0**: length-prefix phân tách được
/// `salt` với `value` bên trong nhánh có salt, nhưng không phân tách nhánh-có-salt với
/// nhánh-không-salt. Với salt `S` và giá trị `M` bất kỳ, giá trị KHÔNG salt
/// `V = u32_be(|S|) ‖ S ‖ M` cho đúng cùng một `fvh` ⇒ người ghi cam kết `V` rồi xuất
/// proof khai `(S, M)`, verifier xanh. Không cần va chạm băm nào.
/// PoC: `poc_hai_che_do_khong_duoc_phan_tach_mien`, `poc_proof_khai_sai_van_xanh`.
pub const TAG_STATE_FVAL_SALTED: &str = "LN/STRATA/state/fval/salted/v1";
/// Tag state leaf (key + fval).
pub const TAG_STATE_LEAF: &str = "LN/STRATA/state/leaf/v1";
/// Tag state internal node.
pub const TAG_STATE_NODE: &str = "LN/STRATA/state/node/v1";

/// `fvh = H_dom(TAG_STATE_FVAL, field_value_bytes)` — dạng **không làm mù**.
///
/// Giữ nguyên byte-for-byte hành vi cũ: đây là dạng mà mọi `state_root` đã ký từ
/// trước dựa vào.
pub fn fval_hash(field_value_bytes: &[u8]) -> Hash32 {
    h_dom(TAG_STATE_FVAL, field_value_bytes)
}

/// `fvh` có **làm mù** (§6.3): `H_dom(TAG_STATE_FVAL_SALTED, u32_be(len(salt)) ‖ salt ‖ value)`.
///
/// - `salt` **rỗng** ⇒ trả về đúng [`fval_hash`] — không có nhánh nào đổi lịch sử.
/// - `salt` khác rỗng ⇒ length-prefix, **không** nối trần như câu chữ §6.3: nối trần
///   cho `(salt="ab", value="c")` và `(salt="a", value="bc")` **cùng một `fvh`**, mà
///   cả hai đều do người ghi chọn ⇒ người ghi đổi được lời khai về giá trị sau khi đã
///   ký. Xem doc đầu module.
///
/// Salt phải **ngẫu nhiên mỗi version** mới chặn được so-khớp liên-proof; một salt cố
/// định chỉ đổi bảng từ điển chứ không phá được nó.
pub fn fval_hash_salted(salt: &[u8], field_value_bytes: &[u8]) -> Hash32 {
    if salt.is_empty() {
        return fval_hash(field_value_bytes);
    }
    let mut buf = Vec::with_capacity(4 + salt.len() + field_value_bytes.len());
    buf.extend_from_slice(&u32_be(salt.len()));
    buf.extend_from_slice(salt);
    buf.extend_from_slice(field_value_bytes);
    h_dom(TAG_STATE_FVAL_SALTED, &buf)
}

/// `leaf = H_dom(TAG_STATE_LEAF, u32_be(len(key)) ‖ key ‖ fvh)`.
pub fn leaf_hash(key: &[u8], fvh: &Hash32) -> Hash32 {
    let mut buf = Vec::with_capacity(4 + key.len() + 32);
    buf.extend_from_slice(&u32_be(key.len()));
    buf.extend_from_slice(key);
    buf.extend_from_slice(fvh);
    h_dom(TAG_STATE_LEAF, &buf)
}

/// `node = H_dom(TAG_STATE_NODE, left ‖ right)`.
fn node_hash(left: &Hash32, right: &Hash32) -> Hash32 {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(left);
    buf.extend_from_slice(right);
    h_dom(TAG_STATE_NODE, &buf)
}

/// Trả về key đầu tiên xuất hiện **nhiều hơn một lần** trong `fields`, nếu có (INV-E6).
///
/// **Vì sao phải gác:** [`build_state_root`] sort bằng `sort_by`, mà `sort_by` của Rust là
/// sort **ổn định** — hai mục trùng key giữ nguyên thứ tự đầu vào. Nên cùng một tập dữ liệu,
/// truyền vào theo hai thứ tự khác nhau, cho **hai `state_root` khác nhau**. Mà `state_root`
/// là trường #5 của `canonical_core` ⇒ nằm trong `version_hash` ⇒ **được ký**. Một root phụ
/// thuộc thứ tự người gọi xếp danh sách là một chữ ký nói về thứ mình không kiểm soát.
///
/// Lẽ độc lập thứ hai: [`prove_field`] chứng minh một trường **theo tên**. Chứng minh theo
/// tên chỉ có nghĩa khi một tên ứng với đúng một giá trị — trùng key làm hỏng chính cửa đó,
/// không chỉ làm root đổi.
///
/// Phạm vi reject theo `#40` P6: **chỉ `state_fields`**, KHÔNG `field_policy`, và reject
/// **kể cả khi hai mục cùng giá trị** — cùng giá trị vẫn đổi SỐ LÁ nên vẫn đổi root.
///
/// **Vì sao không tự chặn bên trong [`build_state_root`]:** hàm đó vô-lỗi và được gọi ở
/// nhiều chỗ nội bộ (`derived_index`, `composite`) nơi tập field đã qua cửa. Chỗ phải gọi là
/// **biên nhận dữ liệu ngoài** — `node/src/dto.rs::to_pairs`.
pub fn find_duplicate_key(fields: &[(Vec<u8>, Vec<u8>)]) -> Option<Vec<u8>> {
    let mut keys: Vec<&[u8]> = fields.iter().map(|(k, _)| k.as_slice()).collect();
    keys.sort_unstable();
    keys.windows(2)
        .find(|w| w[0] == w[1])
        .map(|w| w[0].to_vec())
}

/// Một trường kèm salt làm mù. `salt` rỗng = không làm mù (dạng cũ).
///
/// Kiểu riêng thay vì `(Vec<u8>, Vec<u8>, Vec<u8>)` vì ba `Vec<u8>` cạnh nhau thì
/// hoán vị nhầm hai cái là **đổi `state_root` mà vẫn biên dịch**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaltedField {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
    pub salt: Vec<u8>,
}

impl SaltedField {
    /// Trường KHÔNG làm mù — dạng mọi đường ghi hôm nay đang dùng.
    pub fn plain(key: Vec<u8>, value: Vec<u8>) -> Self {
        Self {
            key,
            value,
            salt: Vec::new(),
        }
    }

    pub fn new(key: Vec<u8>, value: Vec<u8>, salt: Vec<u8>) -> Self {
        Self { key, value, salt }
    }

    fn fvh(&self) -> Hash32 {
        fval_hash_salted(&self.salt, &self.value)
    }
}

/// Nâng danh sách `(key, value)` thành [`SaltedField`] với salt rỗng.
pub fn plain_fields(fields: &[(Vec<u8>, Vec<u8>)]) -> Vec<SaltedField> {
    fields
        .iter()
        .map(|(k, v)| SaltedField::plain(k.clone(), v.clone()))
        .collect()
}

/// Sắp các field theo key tăng dần và trả về leaf-hash tương ứng (tất định).
fn sorted_leaves(fields: &[SaltedField]) -> Vec<(Vec<u8>, Hash32)> {
    let mut v: Vec<(Vec<u8>, Hash32)> = fields
        .iter()
        .map(|f| (f.key.clone(), leaf_hash(&f.key, &f.fvh())))
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// Một tầng cây: gộp đôi, lá lẻ carry nguyên lên (dup-leaf guard — KHÔNG copy).
fn fold_level(level: &[Hash32]) -> Vec<Hash32> {
    let mut next = Vec::with_capacity(level.len().div_ceil(2));
    let mut i = 0;
    while i + 1 < level.len() {
        next.push(node_hash(&level[i], &level[i + 1]));
        i += 2;
    }
    if i < level.len() {
        next.push(level[i]); // carry lá lẻ — không nhân đôi (CVE-2012-2459)
    }
    next
}

/// `build_state_root(fields)` — state_root field-level (§3.6), **không làm mù**.
/// `fields` rỗng → 0^32.
pub fn build_state_root(fields: &[(Vec<u8>, Vec<u8>)]) -> Hash32 {
    build_state_root_salted(&plain_fields(fields))
}

/// [`build_state_root`] với salt theo từng trường (§6.3). Salt rỗng ở **mọi** trường
/// ⇒ kết quả **trùng từng bit** với `build_state_root`.
pub fn build_state_root_salted(fields: &[SaltedField]) -> Hash32 {
    let leaves: Vec<Hash32> = sorted_leaves(fields).into_iter().map(|(_, h)| h).collect();
    if leaves.is_empty() {
        return [0u8; 32];
    }
    let mut level = leaves;
    while level.len() > 1 {
        level = fold_level(&level);
    }
    level[0]
}

/// Field-proof từ `state_root` (INV-E6). KHÔNG lộ giá trị/khoá trường khác — sibling
/// chỉ là hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldProof {
    /// Khoá trường được chứng minh.
    pub key: Vec<u8>,
    /// `field_value_bytes` công khai (content_cid THUẦN hoặc giá trị inline — CHỐT-4).
    /// Verifier băm lại để so `fvh`.
    pub value: Vec<u8>,
    /// `fvh` của trường (tiện cho verifier; cũng tính lại được từ `value` + `salt`).
    pub fvh: Hash32,
    /// Salt làm mù (§6.3). **Rỗng = không làm mù** — dạng của mọi proof hôm nay.
    ///
    /// Phải công khai trong proof: verifier băm lại `value` để so `fvh`, nên không có
    /// salt thì không verify được. Điều đó **không** phá tính chất blinding: cái
    /// blinding chặn là bên **thứ ba** đọc `fvh` của trường **khác** (sibling) rồi dò
    /// cạn miền nhỏ hoặc so khớp giữa hai proof. Người đã được đưa proof thì vốn được
    /// biết chính trường đó.
    pub salt: Vec<u8>,
    /// Đường anh em từ lá lên root: `(sibling_hash, sibling_is_right)`.
    /// `sibling_is_right == true` ⇒ nút hiện tại là con TRÁI.
    pub siblings: Vec<(Hash32, bool)>,
    /// state_root mục tiêu (gắn với một version).
    pub state_root: Hash32,
}

/// Sinh field-proof cho `key` (không làm mù). Trả `None` nếu key không có.
pub fn prove_field(fields: &[(Vec<u8>, Vec<u8>)], key: &[u8]) -> Option<FieldProof> {
    prove_field_salted(&plain_fields(fields), key)
}

/// [`prove_field`] trên tập trường có salt (§6.3).
pub fn prove_field_salted(fields: &[SaltedField], key: &[u8]) -> Option<FieldProof> {
    let target = fields.iter().find(|f| f.key == key)?;
    let value = target.value.clone();
    let salt = target.salt.clone();
    let fvh = target.fvh();

    let leaves: Vec<Hash32> = sorted_leaves(fields).into_iter().map(|(_, h)| h).collect();
    // Vị trí của leaf đang chứng minh (key duy nhất sau khi caller bảo đảm; nếu trùng
    // key, lấy lần xuất hiện đầu sau sort).
    let target_leaf = leaf_hash(key, &fvh);
    let mut pos = leaves.iter().position(|h| *h == target_leaf)?;

    let mut level = leaves;
    let mut siblings = Vec::new();
    while level.len() > 1 {
        let is_left = pos % 2 == 0;
        if is_left {
            if pos + 1 < level.len() {
                // có anh em bên phải
                siblings.push((level[pos + 1], true));
            }
            // pos lẻ-cuối được carry → không có sibling ở tầng này
        } else {
            siblings.push((level[pos - 1], false));
        }
        level = fold_level(&level);
        pos /= 2;
    }

    Some(FieldProof {
        key: key.to_vec(),
        value,
        fvh,
        salt,
        siblings,
        state_root: build_state_root_salted(fields),
    })
}

/// Verify field-proof: tính lại root từ `value` + đường anh em, so `state_root`.
/// INV-E6: không cần biết trường khác — chỉ dùng sibling-hash.
pub fn verify_field_proof(proof: &FieldProof) -> bool {
    // 1. fvh phải khớp băm của value công khai (chống khai man fvh). Có salt thì băm
    //    theo dạng làm mù — salt rỗng rơi về đúng dạng cũ.
    if fval_hash_salted(&proof.salt, &proof.value) != proof.fvh {
        return false;
    }
    // 2. Tính lại từ leaf lên root.
    let mut acc = leaf_hash(&proof.key, &proof.fvh);
    for (sib, sib_is_right) in &proof.siblings {
        acc = if *sib_is_right {
            node_hash(&acc, sib) // nút hiện tại con trái, anh em bên phải
        } else {
            node_hash(sib, &acc)
        };
    }
    acc == proof.state_root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> Vec<(Vec<u8>, Vec<u8>)> {
        vec![
            (b"diagnosis".to_vec(), b"cid_diag".to_vec()),
            (b"birthdate".to_vec(), b"cid_bd".to_vec()),
            (b"name".to_vec(), b"cid_name".to_vec()),
            (b"address".to_vec(), b"cid_addr".to_vec()),
            (b"phone".to_vec(), b"cid_phone".to_vec()),
        ]
    }

    #[test]
    fn root_deterministic_regardless_of_input_order() {
        let mut f1 = fields();
        let mut f2 = fields();
        f2.reverse();
        assert_eq!(build_state_root(&f1), build_state_root(&f2));
        f1.swap(0, 2);
        assert_eq!(build_state_root(&f1), build_state_root(&f2));
    }

    #[test]
    fn field_proof_round_trip_all_fields() {
        let f = fields();
        for (k, _) in &f {
            let p = prove_field(&f, k).expect("proof tồn tại");
            assert!(verify_field_proof(&p), "verify trường {:?}", k);
            assert_eq!(p.state_root, build_state_root(&f));
        }
    }

    #[test]
    fn field_proof_fails_on_value_tamper() {
        // INV-E6: đổi giá trị trường được chứng minh → verify fail.
        let f = fields();
        let mut p = prove_field(&f, b"diagnosis").unwrap();
        p.value = b"cid_FAKE".to_vec();
        p.fvh = fval_hash(&p.value); // khai báo fvh khớp value giả
        assert!(
            !verify_field_proof(&p),
            "value giả phải fail (root không khớp)"
        );
    }

    #[test]
    fn field_proof_no_leak_other_fields() {
        // INV-E6: proof của 1 trường KHÔNG chứa key/value trường khác — chỉ sibling-hash.
        let f = fields();
        let p = prove_field(&f, b"diagnosis").unwrap();
        // Không sibling nào trùng một giá trị thô của trường khác.
        for (k, val) in &f {
            if k == b"diagnosis" {
                continue;
            }
            for (sib, _) in &p.siblings {
                assert_ne!(
                    sib.as_slice(),
                    val.as_slice(),
                    "sibling không được là value thô"
                );
                assert_ne!(
                    sib.as_slice(),
                    k.as_slice(),
                    "sibling không được là key thô"
                );
            }
        }
    }

    #[test]
    fn field_proof_cross_field_substitution_fails() {
        // Lấy proof của trường A, đổi key sang trường B (cùng cây) → fail.
        let f = fields();
        let mut p = prove_field(&f, b"name").unwrap();
        p.key = b"phone".to_vec();
        assert!(!verify_field_proof(&p));
    }

    #[test]
    fn single_field_tree() {
        let f = vec![(b"only".to_vec(), b"v".to_vec())];
        let p = prove_field(&f, b"only").unwrap();
        assert!(verify_field_proof(&p));
        assert!(p.siblings.is_empty());
    }

    #[test]
    fn cid_value_is_pure_no_class_byte() {
        // CHỐT-4: value_cid công khai trong proof = content_cid thuần (ở đây 32B hash thuần),
        // KHÔNG có class byte dẫn đầu → không leak loại.
        let pure_cid = [0x11u8; 32].to_vec();
        let f = vec![(b"file".to_vec(), pure_cid.clone())];
        let p = prove_field(&f, b"file").unwrap();
        assert_eq!(p.value, pure_cid);
        assert!(verify_field_proof(&p));
    }

    // ── INV-E6: trùng key ────────────────────────────────────────────────────

    #[test]
    fn find_duplicate_key_bat_dung_ca_trung_gia_tri() {
        let k = |s: &str| s.as_bytes().to_vec();

        assert_eq!(find_duplicate_key(&[]), None);
        assert_eq!(
            find_duplicate_key(&[(k("a"), vec![1]), (k("b"), vec![2])]),
            None
        );

        // Trùng key, KHÁC giá trị.
        assert_eq!(
            find_duplicate_key(&[(k("a"), vec![1]), (k("b"), vec![2]), (k("a"), vec![9])]),
            Some(k("a"))
        );

        // Trùng key, CÙNG giá trị — vẫn phải bắt (#40 P6 chốt reject kể cả same-value),
        // vì cùng giá trị vẫn thêm một LÁ nên vẫn đổi root.
        assert_eq!(
            find_duplicate_key(&[(k("dup"), vec![7]), (k("dup"), vec![7])]),
            Some(k("dup"))
        );
    }

    /// Đây là LÝ DO của gác trên, viết thành số: `sort_by` là sort ỔN ĐỊNH nên hai mục
    /// trùng key giữ nguyên thứ tự đầu vào ⇒ đảo thứ tự ⇒ ĐỔI `state_root`. Root đó nằm
    /// trong `canonical_core` ⇒ trong `version_hash` ⇒ đã được KÝ.
    ///
    /// Test này cố ý khẳng định hành vi HỎNG, không phải hành vi mong muốn: ngày nào ai đó
    /// làm `build_state_root` bất biến theo thứ tự cả khi trùng key, test này đỏ và người
    /// đó phải quay lại đọc vì sao gác nằm ở biên.
    #[test]
    fn trung_key_lam_state_root_phu_thuoc_thu_tu() {
        let k = |s: &str| s.as_bytes().to_vec();
        let a = [(k("x"), vec![1]), (k("x"), vec![2]), (k("y"), vec![3])];
        let mut b = a.clone();
        b.swap(0, 1);

        assert_ne!(
            build_state_root(&a),
            build_state_root(&b),
            "nếu hai root này BẰNG nhau thì lý do của find_duplicate_key đã đổi — đọc lại docstring"
        );

        // Và cả hai đều bị gác bắt, nên không đường nào trong hai đường trên tới được chuỗi.
        assert!(find_duplicate_key(&a).is_some());
        assert!(find_duplicate_key(&b).is_some());
    }
}

#[cfg(test)]
mod blinding_tests {
    use super::*;

    fn plain() -> Vec<(Vec<u8>, Vec<u8>)> {
        vec![
            (b"sprayed".to_vec(), b"yes".to_vec()),
            (b"stage".to_vec(), b"flowering".to_vec()),
        ]
    }

    /// Salt rỗng ⇒ **không đổi một bit** so với đường cũ. Đây là bài quan trọng nhất
    /// của cả tính năng: `state_root` nằm trong `version_hash` **đã được ký**, nên
    /// một thay đổi làm lệch root là làm hỏng chữ ký của toàn bộ lịch sử.
    #[test]
    fn salt_rong_khong_doi_root_cu() {
        let f = plain();
        assert_eq!(
            build_state_root(&f),
            build_state_root_salted(&plain_fields(&f))
        );
        assert_eq!(fval_hash_salted(&[], b"yes"), fval_hash(b"yes"));
    }

    /// Làm mù phải **thật sự** đổi commitment — nếu không thì cờ salt chỉ là trang trí.
    #[test]
    fn salt_doi_fvh_va_doi_root() {
        assert_ne!(fval_hash_salted(b"s1", b"yes"), fval_hash(b"yes"));
        assert_ne!(
            fval_hash_salted(b"s1", b"yes"),
            fval_hash_salted(b"s2", b"yes"),
            "cùng giá trị, khác salt ⇒ khác fvh — đây chính là thứ chặn so-khớp \
             liên-proof và tấn công từ điển trên trường boolean"
        );

        let salted = vec![
            SaltedField::new(b"sprayed".to_vec(), b"yes".to_vec(), b"s1".to_vec()),
            SaltedField::new(b"stage".to_vec(), b"flowering".to_vec(), b"s2".to_vec()),
        ];
        assert_ne!(build_state_root(&plain()), build_state_root_salted(&salted));
    }

    /// 🔺 Chỗ đi khác câu chữ `Strata-Math §6.3`: nối trần `salt ‖ value` cho phép
    /// **dịch biên**, tức người ghi đổi được lời khai về giá trị sau khi đã ký.
    ///
    /// Length-prefix chặn đúng chỗ đó. Bài này sẽ **đỏ** nếu có ai "sửa cho khớp
    /// spec" bằng cách bỏ prefix đi.
    #[test]
    fn nhap_nhang_bien_salt_value_bi_chan() {
        assert_ne!(
            fval_hash_salted(b"ab", b"c"),
            fval_hash_salted(b"a", b"bc"),
            "nối trần sẽ cho hai cặp này CÙNG một fvh — và cả hai đều do người ghi chọn"
        );
    }

    /// **PoC lỗ P0 — cùng một `fvh` cho hai LỜI KHAI khác nhau, không cần va chạm băm.**
    ///
    /// `fval_hash` và `fval_hash_salted` dùng CHUNG `TAG_STATE_FVAL`. Length-prefix phân
    /// tách được `salt` với `value` **bên trong** nhánh có salt, nhưng KHÔNG phân tách
    /// nhánh-có-salt với nhánh-không-salt. Nên với salt `S` và giá trị `M` bất kỳ, giá trị
    /// KHÔNG salt `V = u32_be(|S|) ‖ S ‖ M` cho **đúng cùng một `fvh`**.
    ///
    /// Hệ quả: người ghi cam kết trường dưới dạng không-salt `V`, rồi sau đó xuất một
    /// `FieldProof` khai `salt = S, value = M` — verifier băm lại ra đúng `fvh`, đúng
    /// `state_root`, **xanh**. Hoặc ngược lại. Tức người ghi đổi được lời khai về giá trị
    /// SAU KHI `state_root` đã nằm trong `version_hash` đã ký — đúng thứ field-proof sinh
    /// ra để chặn.
    ///
    /// Đây là cùng một lỗi phân tách miền mà `nhap_nhang_bien_salt_value_bi_chan` đã chặn,
    /// chỉ lùi lên một bậc: chặn giữa hai trường thì được, giữa hai CHẾ ĐỘ thì chưa.
    #[test]
    fn poc_hai_che_do_khong_duoc_phan_tach_mien() {
        let s: &[u8] = b"SALT";
        let m: &[u8] = b"YES";

        // Giá trị không-salt mà người ghi tự dựng — không cần biết gì ngoài `S` và `M`.
        let mut v = Vec::new();
        v.extend_from_slice(&u32_be(s.len()));
        v.extend_from_slice(s);
        v.extend_from_slice(m);

        assert_ne!(
            fval_hash(&v),
            fval_hash_salted(s, m),
            "cùng fvh cho hai lời khai khác nhau ⇒ người ghi đổi được lời khai sau khi đã ký"
        );
    }

    /// Vế hệ quả: lỗ trên đi thẳng tới `verify_field_proof` — một proof KHAI SAI vẫn xanh.
    ///
    /// Trường thật được cam kết là `v` (không salt). Kẻ xuất proof khai `salt=S, value=M`.
    /// Không đụng tới `siblings`, không đụng `state_root`.
    #[test]
    fn poc_proof_khai_sai_van_xanh() {
        let s: &[u8] = b"SALT";
        let m: &[u8] = b"YES";
        let mut v = Vec::new();
        v.extend_from_slice(&u32_be(s.len()));
        v.extend_from_slice(s);
        v.extend_from_slice(m);

        // Cam kết THẬT: một trường duy nhất, không salt, giá trị `v`.
        let that = vec![(b"chandoan".to_vec(), v.clone())];
        let mut proof = prove_field(&that, b"chandoan").expect("có key");

        // Đổi LỜI KHAI, giữ nguyên fvh/siblings/state_root.
        proof.value = m.to_vec();
        proof.salt = s.to_vec();

        assert!(
            !verify_field_proof(&proof),
            "proof khai sai giá trị mà vẫn verify xanh"
        );
    }

    /// Proof mang salt thì verify được; khai sai salt thì **đỏ**.
    ///
    /// Vế thứ hai mới là vế đáng giá: thiếu nó thì một verifier bỏ qua salt hoàn toàn
    /// vẫn xanh, và blinding trở thành thứ chỉ tồn tại ở phía người ghi.
    #[test]
    fn proof_co_salt_verify_duoc_va_sai_salt_thi_do() {
        let salted = vec![
            SaltedField::new(b"sprayed".to_vec(), b"yes".to_vec(), b"s1".to_vec()),
            SaltedField::new(b"stage".to_vec(), b"flowering".to_vec(), b"s2".to_vec()),
            SaltedField::new(b"lot".to_vec(), b"A17".to_vec(), b"s3".to_vec()),
        ];
        let p = prove_field_salted(&salted, b"sprayed").expect("có trường");
        assert_eq!(p.salt, b"s1".to_vec());
        assert!(verify_field_proof(&p));

        let mut sai = p.clone();
        sai.salt = b"s9".to_vec();
        assert!(!verify_field_proof(&sai), "sai salt phải đỏ");

        // Và đổi giá trị mà giữ nguyên fvh/salt cũng phải đỏ (gác cũ, không được mất).
        let mut doi_gia_tri = p.clone();
        doi_gia_tri.value = b"no".to_vec();
        assert!(!verify_field_proof(&doi_gia_tri));
    }

    /// Trường miền nhỏ là ca dùng sinh ra tính năng này: không salt thì bên thứ ba
    /// **tự dựng lại được** `fvh` từ một danh sách đoán ngắn.
    #[test]
    fn khong_salt_thi_mien_nho_do_can_duoc() {
        let doan = [b"yes".to_vec(), b"no".to_vec()];
        let that = fval_hash(b"yes");
        assert!(
            doan.iter().any(|g| fval_hash(g) == that),
            "không salt: hai lần đoán là ra"
        );
        let that_mu = fval_hash_salted(b"salt-ngau-nhien", b"yes");
        assert!(
            !doan.iter().any(|g| fval_hash(g) == that_mu),
            "có salt: cùng danh sách đoán đó không ra"
        );
    }
}
