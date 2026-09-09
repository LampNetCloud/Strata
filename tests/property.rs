//! Property test P1–P7 — `Strata-Tech.md` §9.3.
//!
//! Khác với test theo invariant §9.1 (ca cụ thể, đã đủ INV-E1..E9), lớp này sinh
//! **đầu vào ngẫu nhiên có cấu trúc** rồi khẳng định tính chất phải đúng với MỌI
//! đầu vào, và shrink về ca nhỏ nhất khi sai. Mục tiêu là bắt lớp lỗi mà ca viết
//! tay không nghĩ tới (biên, hoán vị, độ dài biến thiên, va chạm canonical).
//!
//! | # | Property §9.3 | Test |
//! |---|---|---|
//! | P1 | `mmr_root_deterministic` | [`p1_mmr_root_deterministic`] + golden vector |
//! | P2 | `mmr_inclusion_complete` | [`p2_mmr_inclusion_complete`] |
//! | P3 | `mmr_extend_monotone` (INV-E3) | [`p3_mmr_extend_monotone`] |
//! | P4 | `canonical_roundtrip` | [`p4_canonical_roundtrip`] + [`p4_canonical_injective`] |
//! | P5 | `state_root_order_independent` | [`p5_state_root_order_independent`] |
//! | P6 | `ts_monotone_enables_version_at` | [`p6_ts_monotone_enables_version_at`] |
//! | P7 | `ref_id_collision_resistance` | [`p7_ref_id_distinct_inputs_distinct_id`] |

use ed25519_dalek::SigningKey;
use lampnet_merkle_anchor::Blake3Hasher;
use lampnet_merkle_anchor::mmr::Mmr;
use lampnet_strata::chain::{Policy, StrataChain};
use lampnet_strata::refid::gen_ref_id_raw;
use lampnet_strata::state::build_state_root;
use lampnet_strata::version::{StrataVersion, parse_canonical_core};
use proptest::prelude::*;

// ───────────────────────── khung sinh dữ liệu dùng chung ─────────────────────────

/// Khoá + DID cố định: property ở đây nói về CẤU TRÚC (MMR/canonical/state), không
/// về chữ ký — cố định khoá giữ ca sinh ra tất định và shrink đọc được.
const SK_SEED: [u8; 32] = [7u8; 32];
const DID: [u8; 32] = [9u8; 32];
const REF_ID: [u8; 32] = [3u8; 32];

/// Một version "đề bài": phần caller quyết định, phần còn lại (seq/prev_hash/sig)
/// do chain sinh.
#[derive(Debug, Clone)]
struct VerSpec {
    content_cid: Vec<u8>,
    fields: Vec<(Vec<u8>, Vec<u8>)>,
    /// ts version này = ts trước + delta (⇒ ts KHÔNG-giảm, tiền đề của P6).
    ts_delta: u64,
}

fn field_strategy() -> impl Strategy<Value = (Vec<u8>, Vec<u8>)> {
    (
        prop::collection::vec(any::<u8>(), 1..8),
        prop::collection::vec(any::<u8>(), 0..16),
    )
}

fn ver_spec_strategy() -> impl Strategy<Value = VerSpec> {
    (
        prop::collection::vec(any::<u8>(), 0..40),
        prop::collection::vec(field_strategy(), 0..6),
        0u64..1_000,
    )
        .prop_map(|(content_cid, fields, ts_delta)| VerSpec {
            content_cid,
            fields,
            ts_delta,
        })
}

/// 1..12 version — đủ để MMR có nhiều đỉnh (n lẻ/chẵn, carry) mà vẫn chạy nhanh.
fn chain_spec_strategy() -> impl Strategy<Value = Vec<VerSpec>> {
    prop::collection::vec(ver_spec_strategy(), 1..12)
}

/// Dựng chain thật từ đề bài (ký + policy thật, đi qua đúng `append_version`).
fn build_chain(specs: &[VerSpec]) -> StrataChain {
    let sk = SigningKey::from_bytes(&SK_SEED);
    let mut policy = Policy::new();
    policy.allow(DID, sk.verifying_key());
    let ph = policy.policy_hash();

    let mut ts = 0u64;
    let s0 = &specs[0];
    let mut v0 = StrataVersion::unsigned(
        0,
        [0u8; 32],
        s0.content_cid.clone(),
        build_state_root(&s0.fields),
        DID,
        ph,
        ts,
    );
    v0.sign(&sk);
    let mut chain = StrataChain::genesis(REF_ID, v0, &policy).expect("genesis hợp lệ");

    for s in &specs[1..] {
        ts += s.ts_delta;
        let seq = chain.head().seq + 1;
        let prev = chain.head_version_hash();
        let mut v = StrataVersion::unsigned(
            seq,
            prev,
            s.content_cid.clone(),
            build_state_root(&s.fields),
            DID,
            ph,
            ts,
        );
        v.sign(&sk);
        chain.append_version(v, &policy).expect("append hợp lệ");
    }
    chain
}

/// Root của MMR dựng ĐỘC LẬP với `StrataChain` (chỉ từ danh sách `version_hash`).
fn independent_mmr_root(chain: &StrataChain) -> [u8; 32] {
    let mut m = Mmr::<Blake3Hasher>::new();
    for seq in 0..chain.len() as u64 {
        m.append(&chain.version(seq).unwrap().version_hash());
    }
    m.root()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    // ─────────────────────────────────── P1 ───────────────────────────────────
    /// **P1 `mmr_root_deterministic`** — cùng dãy version (cùng thứ tự seq) → cùng
    /// `mmr_root`.
    ///
    /// Kiểm ba đường dựng KHÁC NHAU phải trùng root: (a) dựng lại từ đầu, (b) `clone`
    /// (tái dựng MMR từ `versions` — xem `chain.rs` impl Clone), (c) MMR độc lập chỉ từ
    /// `version_hash`. Nếu root phụ thuộc trạng thái nội bộ/thứ tự thao tác chứ không
    /// chỉ phụ thuộc dãy leaf thì (b)/(c) lệch.
    #[test]
    fn p1_mmr_root_deterministic(specs in chain_spec_strategy()) {
        let a = build_chain(&specs);
        let b = build_chain(&specs);
        prop_assert_eq!(a.mmr_root(), b.mmr_root());
        prop_assert_eq!(a.head_version_hash(), b.head_version_hash());
        prop_assert_eq!(a.clone().mmr_root(), a.mmr_root());
        prop_assert_eq!(independent_mmr_root(&a), a.mmr_root());
    }

    // ─────────────────────────────────── P2 ───────────────────────────────────
    /// **P2 `mmr_inclusion_complete`** — ∀ `seq ≤ head`, `prove_version(seq)` verify
    /// được dưới `mmr_root`. Kèm negative control: đổi leaf → phải fail (nếu không,
    /// "verify được" là vô nghĩa).
    #[test]
    fn p2_mmr_inclusion_complete(specs in chain_spec_strategy()) {
        let chain = build_chain(&specs);
        let root = chain.mmr_root();
        for seq in 0..chain.len() as u64 {
            let (proof, size, vh) = chain.prove_version(seq).expect("seq ≤ head phải có proof");
            prop_assert!(
                StrataChain::verify_version(root, &vh, seq, size, &proof),
                "inclusion proof seq={} phải verify dưới mmr_root", seq
            );
            let mut bad = vh;
            bad[0] ^= 0xff;
            prop_assert!(
                !StrataChain::verify_version(root, &bad, seq, size, &proof),
                "negative control: leaf giả seq={} phải fail", seq
            );
        }
        prop_assert!(chain.prove_version(chain.len() as u64).is_none(), "seq > head → None");
    }
}

proptest! {
    // P3 quét MỌI (n, seq) và dựng lại chain prefix cho từng n ⇒ O(n²) lần ký Ed25519.
    // Ở 256 case × 12 version nó chiếm ~77s một mình (đo 2026-08-01, build debug) —
    // đắt hơn toàn bộ phần còn lại cộng lại. Hạ số case + rút ngắn chain: miền (n, seq)
    // vẫn quét ĐỦ trong mỗi case, chỉ giảm số dãy khác nhau được thử.
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    // ─────────────────────────────────── P3 ───────────────────────────────────
    /// **P3 `mmr_extend_monotone` (INV-E3)** — append-only: mở rộng chain KHÔNG đổi
    /// lịch sử.
    ///
    /// Phát biểu §9.3 viết "proof hợp lệ dưới `root_n` vẫn hợp lệ dưới `root_{n+1}`".
    /// Với MMR **bind theo size**, câu đó không đúng nguyên văn: `verify` nhận
    /// `mmr_size` nên proof-tại-size-n chỉ verify dưới `root_n`. Nội dung THẬT của
    /// INV-E3 mà cài đặt phải giữ, và test này khẳng định:
    ///
    /// 1. `root_n` và proof lịch sử **tái dựng được** từ chain đã dài hơn
    ///    (`prove_version_at(seq, n)`) và **trùng byte** với proof sinh lúc chain còn
    ///    đúng n version ⇒ không có cách nào viết lại lịch sử mà vẫn khớp anchor cũ;
    /// 2. proof lịch sử vẫn verify dưới `root_n` đã neo;
    /// 3. `root_n` của prefix trùng với `root_n` tái dựng từ chain dài hơn.
    #[test]
    fn p3_mmr_extend_monotone(specs in prop::collection::vec(ver_spec_strategy(), 1..8)) {
        let full = build_chain(&specs);
        let total = full.len() as u64;

        for n in 1..=total {
            // Chain "quá khứ": đúng n version đầu — tương ứng trạng thái lúc neo root_n.
            let past = build_chain(&specs[..n as usize]);
            let root_n = past.mmr_root();

            // (3) root lịch sử tái dựng từ chain dài hơn phải trùng.
            let mut m = Mmr::<Blake3Hasher>::new();
            for seq in 0..n {
                m.append(&full.version(seq).unwrap().version_hash());
            }
            prop_assert_eq!(m.root(), root_n, "root_{} tái dựng phải trùng prefix", n);

            for seq in 0..n {
                let (past_proof, _, past_vh) = past.prove_version(seq).unwrap();
                let (hist_proof, hist_vh) = full
                    .prove_version_at(seq, n)
                    .expect("seq < n ≤ len → phải có proof lịch sử");

                // (1) trùng byte: append-only không đụng proof cũ.
                prop_assert_eq!(&hist_proof, &past_proof, "proof lịch sử seq={} n={} lệch", seq, n);
                prop_assert_eq!(hist_vh, past_vh);

                // (2) verify dưới root_n đã neo.
                prop_assert!(
                    StrataChain::verify_version(root_n, &hist_vh, seq, n, &hist_proof),
                    "proof lịch sử seq={} phải verify dưới root_{}", seq, n
                );
            }
        }
        // Ngoài miền: seq ≥ mmr_size, hoặc mmr_size > len.
        prop_assert!(full.prove_version_at(0, 0).is_none());
        prop_assert!(full.prove_version_at(0, total + 1).is_none());
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    // ─────────────────────────────────── P4 ───────────────────────────────────
    /// **P4 `canonical_roundtrip`** — `canonical_core` → `parse_canonical_core` → cùng
    /// `StrataVersion` (trừ `sig`, CHỐT-1 không encode sig); và encode **tất định
    /// byte-chính-xác** (cùng input → cùng byte, mọi lần).
    #[test]
    fn p4_canonical_roundtrip(
        seq in any::<u64>(),
        prev in any::<[u8; 32]>(),
        cid in prop::collection::vec(any::<u8>(), 0..80),
        sr in any::<[u8; 32]>(),
        did in any::<[u8; 32]>(),
        ph in any::<[u8; 32]>(),
        ts in any::<u64>(),
        sig_byte in any::<u8>(),
    ) {
        let mut v = StrataVersion::unsigned(seq, prev, cid, sr, did, ph, ts);
        v.sig = [sig_byte; 64]; // sig KHÔNG vào canonical (CHỐT-1) ⇒ không được ảnh hưởng
        let bytes = v.canonical_core();

        prop_assert_eq!(&bytes, &v.canonical_core(), "encode phải tất định byte-chính-xác");

        let back = parse_canonical_core(&bytes).expect("byte do chính encoder sinh phải parse được");
        prop_assert_eq!(back.seq, v.seq);
        prop_assert_eq!(back.prev_hash, v.prev_hash);
        prop_assert_eq!(&back.content_cid, &v.content_cid);
        prop_assert_eq!(back.state_root, v.state_root);
        prop_assert_eq!(back.author_did, v.author_did);
        prop_assert_eq!(back.policy_hash, v.policy_hash);
        prop_assert_eq!(back.ts, v.ts);
        prop_assert_eq!(back.sig, [0u8; 64], "sig không nằm trong canonical ⇒ về 0^64");
        // Cùng version_hash: đủ để nói roundtrip giữ nguyên phần được cam kết.
        prop_assert_eq!(back.version_hash(), v.version_hash());
        prop_assert_eq!(back.canonical_core(), bytes);
    }

    /// **P4 (mặt thứ hai) — song ánh.** Hai version khác nhau ở BẤT KỲ trường core nào
    /// phải cho byte canonical khác nhau (⇒ `version_hash` khác). Mất tính này thì hai
    /// lịch sử khác nhau trùng hash.
    ///
    /// **Lưu vết mutation-test (2026-08-01):** bỏ hẳn `u32_be(len(content_cid))` khỏi
    /// `canonical_core` KHÔNG làm test này đỏ — và đó là kết quả ĐÚNG, không phải test
    /// yếu. Layout hiện tại có **đúng một** trường biến độ dài (`content_cid`) đứng
    /// trước toàn trường cố định, nên tổng độ dài buffer đã tự xác định `len(cid)` ⇒
    /// encoding vẫn song ánh. Length-prefix ở đây là **dự phòng**: nó trở thành
    /// load-bearing ngay khi thêm trường biến độ dài THỨ HAI. (Mutation đó vẫn bị bắt,
    /// bởi `p4_canonical_roundtrip` — decoder đọc prefix nên lệch ngay.)
    #[test]
    fn p4_canonical_injective(
        a in ver_core_strategy(),
        b in ver_core_strategy(),
    ) {
        let (va, vb) = (a.build(), b.build());
        if a != b {
            prop_assert_ne!(va.canonical_core(), vb.canonical_core(), "core khác → byte phải khác");
            prop_assert_ne!(va.version_hash(), vb.version_hash());
        } else {
            prop_assert_eq!(va.canonical_core(), vb.canonical_core());
        }
    }

    /// **P4 (negative control)** — decoder phải CHẶT: byte cụt, byte thừa đuôi, hoặc
    /// length-prefix vượt phần còn lại đều phải `Err`. Decoder lỏng = mở lại đúng lớp
    /// nhập nhằng mà canonical sinh ra để đóng.
    #[test]
    fn p4_parse_rejects_malformed(
        cid in prop::collection::vec(any::<u8>(), 0..40),
        cut in 1usize..40,
        tail in prop::collection::vec(any::<u8>(), 1..8),
    ) {
        let v = StrataVersion::unsigned(1, [1u8; 32], cid, [2u8; 32], [3u8; 32], [4u8; 32], 5);
        let bytes = v.canonical_core();

        // (a) cụt: bỏ `cut` byte cuối.
        let cut = cut.min(bytes.len());
        prop_assert!(parse_canonical_core(&bytes[..bytes.len() - cut]).is_err(), "byte cụt phải Err");

        // (b) thừa đuôi: nối thêm rác.
        let mut extra = bytes.clone();
        extra.extend_from_slice(&tail);
        prop_assert!(parse_canonical_core(&extra).is_err(), "byte thừa đuôi phải Err");

        // (c) length-prefix khai man vượt phần còn lại.
        let mut lied = bytes.clone();
        lied[40..44].copy_from_slice(&u32::MAX.to_be_bytes());
        prop_assert!(parse_canonical_core(&lied).is_err(), "len-prefix vượt buffer phải Err");
    }

    // ─────────────────────────────────── P5 ───────────────────────────────────
    /// **P5 `state_root_order_independent`** — hoán vị thứ tự nhập field → cùng
    /// `state_root` (sort theo key §3.6).
    ///
    /// Sinh field với **key phân biệt** — đúng ngữ nghĩa "hồ sơ có tập trường", vì
    /// `build_state_root` sort theo key và sort là **ổn định**: nếu cho phép key trùng
    /// thì hoán vị ĐỔI root (xem `state_root_dup_key_not_permutation_invariant`).
    #[test]
    fn p5_state_root_order_independent(
        fields in distinct_key_fields(),
        seed in any::<u64>(),
    ) {
        let permuted = permute(&fields, seed);
        prop_assert_eq!(
            build_state_root(&permuted), build_state_root(&fields),
            "hoán vị thứ tự nhập không được đổi state_root"
        );

        // Negative control: đổi MỘT giá trị → root phải đổi (nếu không, "bằng nhau" ở
        // trên có thể chỉ vì root không phụ thuộc input).
        if !fields.is_empty() {
            let mut changed = fields.clone();
            changed[0].1.push(0xAA);
            prop_assert_ne!(build_state_root(&changed), build_state_root(&fields));
        }
    }

    // ─────────────────────────────────── P6 ───────────────────────────────────
    /// **P6 `ts_monotone_enables_version_at`** — `ts` không-giảm ⇒ `version_at(t)` trả
    /// đúng version (binary search khớp tham chiếu quét tuyến tính: version có `ts` lớn
    /// nhất ≤ t), và proof kèm theo verify được dưới `mmr_root` hiện tại.
    #[test]
    fn p6_ts_monotone_enables_version_at(
        specs in chain_spec_strategy(),
        probe in any::<u64>(),
    ) {
        let chain = build_chain(&specs);
        let root = chain.mmr_root();
        let size = chain.len() as u64;

        // Điểm dò: t ngẫu nhiên + mọi ts thật và ts±1 (biên của binary search).
        let mut probes = vec![probe, 0, u64::MAX];
        for seq in 0..size {
            let ts = chain.version(seq).unwrap().ts;
            probes.push(ts);
            probes.push(ts.saturating_sub(1));
            probes.push(ts.saturating_add(1));
        }

        for t in probes {
            // Tham chiếu độc lập: quét tuyến tính.
            let expected: Option<u64> = (0..size)
                .rev()
                .find(|&s| chain.version(s).unwrap().ts <= t);

            match (chain.version_at(t), expected) {
                (None, None) => {}
                (Some((v, proof)), Some(exp_seq)) => {
                    let exp = chain.version(exp_seq).unwrap();
                    // ts đơn điệu nhưng có thể BẰNG nhau ⇒ so theo ts + tính "lớn nhất":
                    // binary search trả seq lớn nhất trong nhóm cùng ts, đúng bằng
                    // tham chiếu quét ngược.
                    prop_assert_eq!(v.seq, exp.seq, "version_at({}) sai seq", t);
                    prop_assert!(v.ts <= t);
                    prop_assert!(
                        StrataChain::verify_version(root, &v.version_hash(), v.seq, size, &proof),
                        "proof kèm version_at({}) phải verify", t
                    );
                }
                (got, exp) => prop_assert!(
                    false, "version_at({}) lệch tham chiếu: got={:?} expected_seq={:?}",
                    t, got.map(|(v, _)| v.seq), exp
                ),
            }
        }
    }

    // ─────────────────────────────────── P7 ───────────────────────────────────
    /// **P7 `ref_id_collision_resistance`** — `(author_did, nonce)` khác → `ref_id`
    /// khác.
    ///
    /// `Did` trong Strata là `[u8; 32]` cố định (CHỐT-5), nên test ở ĐÚNG miền hợp lệ:
    /// did 32 byte, nonce độ dài tuỳ ý. Miền `author_did` độ dài tuỳ ý có va chạm cấu
    /// trúc — xem `ref_id_variable_len_did_collides` (không phải va chạm BLAKE3).
    ///
    /// **Miền sinh hẹp có chủ đích.** Bản đầu lấy did ngẫu nhiên 32 byte đầy đủ: hai did
    /// gần như LUÔN khác nhau ⇒ nhánh "cùng did, khác nonce" không bao giờ chạy, và
    /// mutation-test cho thấy bỏ hẳn `nonce` khỏi `gen_ref_id_raw` mà test **vẫn xanh**.
    /// Nay did lấy từ pool 4 giá trị + nonce từ bảng chữ cái 2 ký tự ⇒ cả bốn tổ hợp
    /// (cùng/khác) × (did/nonce) đều xuất hiện thật.
    #[test]
    fn p7_ref_id_distinct_inputs_distinct_id(
        ia in 0usize..DID_POOL.len(),
        ib in 0usize..DID_POOL.len(),
        nonce_a in prop::collection::vec(0u8..2, 0..4),
        nonce_b in prop::collection::vec(0u8..2, 0..4),
    ) {
        let (did_a, did_b) = (DID_POOL[ia], DID_POOL[ib]);
        let a = gen_ref_id_raw(&did_a, &nonce_a);
        let b = gen_ref_id_raw(&did_b, &nonce_b);
        if ia == ib && nonce_a == nonce_b {
            prop_assert_eq!(a, b, "cùng input → cùng ref_id (tất định)");
        } else {
            prop_assert_ne!(a, b, "input khác → ref_id phải khác");
        }
    }

    /// **P7 (cô lập nonce)** — GIỮ NGUYÊN did, chỉ đổi nonce → `ref_id` phải đổi.
    /// Chính ca mà mutation "bỏ `nonce`" phải làm đỏ.
    #[test]
    fn p7_nonce_alone_changes_ref_id(
        i in 0usize..DID_POOL.len(),
        nonce_a in prop::collection::vec(0u8..3, 0..4),
        nonce_b in prop::collection::vec(0u8..3, 0..4),
    ) {
        prop_assume!(nonce_a != nonce_b);
        let did = DID_POOL[i];
        prop_assert_ne!(
            gen_ref_id_raw(&did, &nonce_a), gen_ref_id_raw(&did, &nonce_b),
            "cùng did, nonce khác → ref_id phải khác"
        );
    }

    /// **P7 (cô lập did)** — GIỮ NGUYÊN nonce, chỉ đổi did → `ref_id` phải đổi.
    #[test]
    fn p7_did_alone_changes_ref_id(
        ia in 0usize..DID_POOL.len(),
        ib in 0usize..DID_POOL.len(),
        nonce in prop::collection::vec(0u8..3, 0..4),
    ) {
        prop_assume!(ia != ib);
        prop_assert_ne!(
            gen_ref_id_raw(&DID_POOL[ia], &nonce), gen_ref_id_raw(&DID_POOL[ib], &nonce),
            "cùng nonce, did khác → ref_id phải khác"
        );
    }
}

/// Pool `Did` nhỏ (4 giá trị 32 byte phân biệt) — đủ nhỏ để "hai did BẰNG nhau" là ca
/// thường gặp trong khi sinh, đủ khác nhau về byte để không tự tạo cấu trúc giả.
const DID_POOL: [[u8; 32]; 4] = [[0x11; 32], [0x22; 32], [0xA5; 32], [0xFE; 32]];

// ─────────────────── strategy/helper phụ (ngoài macro proptest) ───────────────────

/// Phần core của một version, so sánh được — dùng cho P4 song ánh.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VerCore {
    seq: u64,
    prev: [u8; 32],
    cid: Vec<u8>,
    state_root: [u8; 32],
    did: [u8; 32],
    policy_hash: [u8; 32],
    ts: u64,
}

impl VerCore {
    fn build(&self) -> StrataVersion {
        StrataVersion::unsigned(
            self.seq,
            self.prev,
            self.cid.clone(),
            self.state_root,
            self.did,
            self.policy_hash,
            self.ts,
        )
    }
}

/// Miền hẹp có chủ đích: giá trị nhỏ ⇒ dễ sinh hai core BẰNG nhau lẫn KHÁC nhau ở
/// đúng một trường, nên cả hai nhánh của P4-song-ánh đều được chạy thật.
fn ver_core_strategy() -> impl Strategy<Value = VerCore> {
    (
        0u64..4,
        prop::array::uniform32(0u8..2),
        prop::collection::vec(0u8..2, 0..4),
        prop::array::uniform32(0u8..2),
        prop::array::uniform32(0u8..2),
        prop::array::uniform32(0u8..2),
        0u64..4,
    )
        .prop_map(
            |(seq, prev, cid, state_root, did, policy_hash, ts)| VerCore {
                seq,
                prev,
                cid,
                state_root,
                did,
                policy_hash,
                ts,
            },
        )
}

/// Tập field có key ĐÔI MỘT PHÂN BIỆT (ngữ nghĩa "hồ sơ có tập trường").
fn distinct_key_fields() -> impl Strategy<Value = Vec<(Vec<u8>, Vec<u8>)>> {
    prop::collection::vec(
        (
            prop::collection::vec(any::<u8>(), 1..6),
            prop::collection::vec(any::<u8>(), 0..12),
        ),
        0..8,
    )
    .prop_map(|fields| {
        let mut seen = std::collections::BTreeSet::new();
        fields
            .into_iter()
            .filter(|(k, _)| seen.insert(k.clone()))
            .collect()
    })
}

/// Hoán vị tất định theo `seed` (Fisher–Yates với LCG — không cần dep rng trong test).
fn permute<T: Clone>(items: &[T], seed: u64) -> Vec<T> {
    let mut out = items.to_vec();
    let mut s = seed | 1;
    for i in (1..out.len()).rev() {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let j = (s >> 33) as usize % (i + 1);
        out.swap(i, j);
    }
    out
}

// ─────────────────────── pin/lưu vết ngoài bảng P1–P7 ───────────────────────

/// **P1 golden vector** — root cố định cho một dãy version cố định.
///
/// P1 property ở trên chỉ chứng minh root **ổn định trong cùng một build**; câu "trên
/// mọi máy" cần một giá trị NEO. Số dưới đây do chính đợt này sinh ra và ghim lại: đổi
/// bất kỳ khâu nào trong `canonical_core`/tag/MMR sẽ làm test đỏ — đó chính là mục
/// đích (byte-layout đã cố định trong `_CONTRACT.md`, không được trôi lặng lẽ).
#[test]
fn p1_golden_mmr_root_pins_byte_layout() {
    let specs: Vec<VerSpec> = (0..5u8)
        .map(|i| VerSpec {
            content_cid: vec![i; i as usize],
            fields: vec![
                (vec![b'k', i], vec![i, i.wrapping_add(1)]),
                (vec![b'a'], vec![i]),
            ],
            ts_delta: 10 + i as u64,
        })
        .collect();
    let chain = build_chain(&specs);
    let got = hex(&chain.mmr_root());
    assert_eq!(
        got, GOLDEN_MMR_ROOT,
        "mmr_root golden đổi ⇒ byte-layout canonical/MMR/tag đã trôi — đối chiếu _CONTRACT.md trước khi cập nhật số này"
    );
}

/// Sinh bởi `p1_golden_mmr_root_pins_byte_layout` (2026-08-01, rustc pin 1.96.0).
const GOLDEN_MMR_ROOT: &str = "2da5091fd1d666016bb515e675657816c36404ba4f23ea9d92894a8302a56d26";

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// **Lưu vết P5 — biên nằm ngoài phát biểu §9.3.** `build_state_root` sort theo key
/// bằng sort **ổn định**, nên khi có KEY TRÙNG (hai giá trị khác nhau cùng key) thì
/// hoán vị thứ tự nhập ĐỔI `state_root`. Không phải lỗi băm: là hệ quả của việc
/// `fields: &[(key, value)]` không ràng buộc key duy nhất ở KIỂU.
///
/// Test này ghim hành vi thật để không ai đọc P5 rồi tưởng root bất biến với mọi input.
#[test]
fn state_root_dup_key_not_permutation_invariant() {
    let a: Vec<(Vec<u8>, Vec<u8>)> = vec![
        (b"k".to_vec(), b"v1".to_vec()),
        (b"k".to_vec(), b"v2".to_vec()),
    ];
    let b: Vec<(Vec<u8>, Vec<u8>)> = vec![
        (b"k".to_vec(), b"v2".to_vec()),
        (b"k".to_vec(), b"v1".to_vec()),
    ];
    assert_ne!(
        build_state_root(&a),
        build_state_root(&b),
        "nếu bằng nhau thì ghi chú này đã lỗi thời — cập nhật lại"
    );
}

/// **Lưu vết P7 — va chạm CẤU TRÚC khi `author_did` độ dài biến thiên.**
///
/// `gen_ref_id_raw(author_did, nonce)` nối `author_did ‖ nonce` **không length-prefix**
/// (`src/refid.rs`), nên `("ab", "c")` và `("a", "bc")` cho CÙNG `ref_id`. Trong đường
/// đi hợp lệ, `Did = [u8; 32]` cố định nên không chạm; nhưng chữ ký hàm nhận `&[u8]` và
/// `gen_ref_id` được re-export ở gốc crate, nên caller ngoài chạm được.
///
/// Cùng lớp lỗi song ánh canonical §1.7 quy tắc 3 (issue #18). Ghim lại để quyết định
/// nằm ở spec chứ không trôi lặng.
#[test]
fn ref_id_variable_len_did_collides_without_length_prefix() {
    let x = gen_ref_id_raw(b"ab", b"c");
    let y = gen_ref_id_raw(b"a", b"bc");
    assert_eq!(
        x, y,
        "nếu KHÁC nhau thì refid đã được length-prefix — cập nhật lưu vết này + P7"
    );
}
