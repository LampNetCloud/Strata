# Strata S7 — Property test P1–P7 (`Strata-Tech.md` §9.3)

**Ngày:** 2026-08-01 · **PR:** #38 · **Issue spec kèm theo:** #39 · **Phạm vi:** phần còn lại lớn nhất
của milestone S7 sau đợt CI (`STRATA-S7-CI-REPORT.md`).

Trước đợt này repo có **0** property test (không `proptest`, không `quickcheck`) trong khi
`Strata-Tech.md` §9.3 liệt kê 7 property bắt buộc. Test theo invariant §9.1 đã phủ đủ
INV-E1..E9 nhưng đều là **ca viết tay**: chúng khẳng định "với đầu vào này thì đúng", không
khẳng định "với mọi đầu vào thì đúng".

---

## 1. Kết quả

| | |
|---|---|
| Test mới | **14** (7 property §9.3 + 3 property phụ trợ + 1 golden vector + 2 lưu vết biên + 1 negative-control decoder) |
| Toàn workspace | **185 pass / 0 fail** (`cargo test --workspace`) |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| `cargo fmt --all --check` | exit 0 |
| Thời gian chạy `tests/property.rs` | ~30 s (build debug) |
| Mã nguồn | `tests/property.rs` (mới), `src/version.rs` (+ decoder), `src/lib.rs` (re-export), `Cargo.toml` (dev-dep `proptest` 1.11) |

`proptest` vào **`[dev-dependencies]`**, không chạm đường build production.

### Bảng phủ §9.3

| # | Property §9.3 | Test | Ghi chú |
|---|---|---|---|
| P1 | `mmr_root_deterministic` | `p1_mmr_root_deterministic` + `p1_golden_mmr_root_pins_byte_layout` | 3 đường dựng độc lập phải trùng root; **golden vector** ghim byte-layout |
| P2 | `mmr_inclusion_complete` | `p2_mmr_inclusion_complete` | ∀ seq ≤ head + negative control leaf giả |
| P3 | `mmr_extend_monotone` (INV-E3) | `p3_mmr_extend_monotone` | xem §3 — phát biểu spec cần hiệu chỉnh |
| P4 | `canonical_roundtrip` | `p4_canonical_roundtrip`, `p4_canonical_injective`, `p4_parse_rejects_malformed` | cần viết **decoder** mới, xem §2 |
| P5 | `state_root_order_independent` | `p5_state_root_order_independent` | + lưu vết biên key trùng (§4.2) |
| P6 | `ts_monotone_enables_version_at` | `p6_ts_monotone_enables_version_at` | đối chiếu binary-search với quét tuyến tính |
| P7 | `ref_id_collision_resistance` | `p7_ref_id_distinct_inputs_distinct_id` + 2 test cô lập | + phát hiện §4.1 |

---

## 2. P4 đòi một decoder mà repo chưa có

`version.rs` chỉ có **encoder** `canonical_core()`. P4 phát biểu `canonical_version_bytes →
parse → cùng StrataVersion (trừ sig)` — không có `parse` thì P4 không viết được.

Đã thêm `version::parse_canonical_core(&[u8]) -> Result<StrataVersion, CanonicalError>`.
Đây là **nghịch đảo của layout đã cố định**, không phải layout mới: không đụng
`_CONTRACT.md`, không đổi một byte nào của đường băm/ký.

Decoder **chặt có chủ đích**, từ chối cả ba:

1. byte cụt (`Truncated`),
2. `len(content_cid)` khai vượt phần còn lại (`LengthOverflow`),
3. **byte thừa ở đuôi** (`TrailingBytes`).

Lý do phải chặt: decoder lỏng ở bất kỳ điểm nào trong ba điểm đó ⇒ hai chuỗi byte khác nhau
cùng giải ra một version ⇒ mở lại đúng lớp nhập nhằng mà canonical §1.7 sinh ra để đóng
(cùng họ với trần `< 2³²` của `u32_be`, issue #18). `sig` trả về `0^64` — đúng CHỐT-1, `sig`
không nằm trong canonical nên **không thể** khôi phục từ byte; `version_hash` của kết quả
trùng bản gốc.

---

## 3. P3 — phát biểu §9.3 không đúng nguyên văn với MMR bind-theo-size

§9.3 viết:

> P3 `mmr_extend_monotone`: ∀ proof hợp lệ dưới `root_n` vẫn hợp lệ dưới `root_{n+1}` (INV-E3)

Câu này **không đúng nguyên văn** với cài đặt hiện tại, và không phải do cài đặt sai:
`mmr::verify` nhận `mmr_size` làm tham số, tức proof được **bind vào size**. Proof sinh ở
size *n* chỉ verify dưới `root_n`; muốn verify dưới `root_{n+1}` phải sinh lại proof ở size
*n+1*. Đó là thiết kế đúng — nếu proof cũ verify được dưới root mới thì `mmr_size` đã không
tham gia ràng buộc, và đó mới là lỗ hổng.

Nội dung THẬT của INV-E3 mà append-only phải giữ, và là cái test khẳng định:

1. proof lịch sử **tái dựng được** từ chain đã dài hơn (`prove_version_at(seq, n)`) và
   **trùng byte** với proof sinh lúc chain còn đúng *n* version — tức mở rộng không viết lại
   được lịch sử;
2. proof lịch sử vẫn verify dưới `root_n` **đã neo**;
3. `root_n` tái dựng từ chain dài hơn trùng `root_n` của chain prefix.

→ **Đề xuất sửa text §9.3 (miền anh Đức)**, xem §5.

---

## 4. Hai phát hiện, đều do property test lôi ra

### 4.1 `ref_id` — nối `author_did ‖ nonce` không length-prefix ⇒ va chạm cấu trúc

`src/refid.rs`:

```rust
pub fn gen_ref_id_raw(author_did: &[u8], nonce: &[u8]) -> Hash32 {
    let mut x = Vec::with_capacity(author_did.len() + nonce.len());
    x.extend_from_slice(author_did);
    x.extend_from_slice(nonce);
    h_dom(TAG_REF, &x)
}
```

Không có length-prefix ⇒ `("ab", "c")` và `("a", "bc")` cho **cùng một `ref_id`**. Đã ghim
bằng test `ref_id_variable_len_did_collides_without_length_prefix` (test PASS = va chạm có
thật; nếu ai đó vá thì test đỏ và buộc phải cập nhật lưu vết).

**Không phải va chạm BLAKE3** — là mất song ánh ở tầng encode đầu vào, đúng lớp lỗi mà issue
#18 / §1.7 quy tắc 3 vừa siết cho trường length-prefix.

**Mức độ thật (không thổi phồng):** trên đường đi hợp lệ, `Did = [u8; 32]` cố định (CHỐT-5)
nên `author_did` luôn 32 byte ⇒ **không va chạm trong dùng bình thường**. Phơi nhiễm nằm ở
chỗ chữ ký hàm nhận `&[u8]` độ dài tuỳ ý và `gen_ref_id` được **re-export ở gốc crate**, nên
caller ngoài (module khác / repo khác) chạm được. Chưa thấy call-site nào trong repo truyền
did khác 32 byte.

**Vì sao KHÔNG tự vá:** `Strata-Tech.md:314` viết chính công thức này —
`ref_id = H_dom("LN/STRATA/ref/v1", author_did ‖ genesis_nonce)`. Code khớp spec. Thêm
length-prefix là **đổi giá trị `ref_id`** của mọi hồ sơ đã sinh (định danh ổn định, INV-E5,
"KHÔNG đổi qua các phiên bản") ⇒ quyết định thuộc spec, không phải chỗ dev tự chốt. Đã mở
issue, xem §5.

### 4.2 `state_root` không bất biến với hoán vị khi **key trùng**

`build_state_root` sort theo key bằng sort **ổn định**. Với hai field cùng key khác giá trị,
hoán vị thứ tự nhập ⇒ thứ tự lá đổi ⇒ `state_root` đổi. Ghim bằng
`state_root_dup_key_not_permutation_invariant`.

Không phải lỗi băm: là hệ quả của việc `fields: &[(Vec<u8>, Vec<u8>)]` **không ràng buộc key
duy nhất ở KIỂU**. P5 vì vậy chạy trên miền key phân biệt — đúng ngữ nghĩa "hồ sơ có tập
trường" của §3.6. Điểm cần quyết: có siết (dedupe/từ chối) hay để nguyên + ghi rõ tiền đề.
Đưa vào cùng issue §5 vì nó chạm ngữ nghĩa `state_root`.

---

## 5. Việc treo sang anh Đức

Issue **#39** mở kèm đợt này (2 điểm spec + 1 đề xuất text):

1. **`ref_id` length-prefix** (§4.1) — sửa hay giữ + ghi tiền đề "`author_did` LUÔN 32 byte"
   vào spec.
2. **key trùng trong `build_state_root`** (§4.2) — siết ở kiểu/API hay ghi tiền đề.

Kèm **đề xuất hiệu chỉnh text §9.3 P3** (§3) cho khớp MMR bind-theo-size.

Rút kinh nghiệm #18 (PR #21 treo 6 ngày vì bọc 1 dòng spec chung với code): **PR này thuần
code + test, không kèm sửa spec nào.**

---

## 6. Chống "xanh giả" — mutation test 10 ca

Property test dễ mắc bệnh xanh-vì-không-khẳng-định-gì (bài học negative control ở PR #33).
Nên trước khi land đã **cố tình phá code** rồi kiểm từng property có đỏ đúng chỗ không:

| # | Mutation | Test phải đỏ | Kết quả |
|---|---|---|---|
| M1 | decoder bỏ check byte-thừa-đuôi | `p4_parse_rejects_malformed` | ✅ đỏ |
| M2 | `build_state_root` không sort theo key | `p5_state_root_order_independent` | ✅ đỏ |
| M3 | đổi `TAG_VER` v1→v2 | `p1_golden_...` | ✅ đỏ |
| M4 | `prove_version_at` bỏ qua `mmr_size` | `p3_mmr_extend_monotone` | ✅ đỏ |
| M5 | `version_at` dùng `<` thay `<=` | `p6_ts_monotone_enables_version_at` | ✅ đỏ |
| M6 | `gen_ref_id_raw` bỏ `nonce` | `p7_*` | ❌ **VẪN XANH** → xem dưới |
| M7 | MMR append leaf hằng thay `version_hash` | `p2_mmr_inclusion_complete` | ✅ đỏ |
| M8 | `gen_ref_id_raw` bỏ `author_did` | `p7_*` | ✅ đỏ |
| M9 | `canonical_core` bỏ len-prefix | — | xanh ở `injective`, **đỏ** ở `roundtrip` → xem dưới |
| M10 | decoder đảo thứ tự `state_root`↔`author_did` | `p4_canonical_roundtrip` | ✅ đỏ |

**M6 bắt được một test xanh giả thật.** Bản P7 đầu sinh `did_a`, `did_b` ngẫu nhiên đủ 32
byte ⇒ hai did **gần như luôn khác nhau** ⇒ nhánh "cùng did, khác nonce" chưa bao giờ chạy,
nên bỏ hẳn `nonce` khỏi hàm mà test vẫn xanh. Đã vá: did lấy từ pool 4 giá trị, nonce từ bảng
chữ cái 2 ký tự, **cộng thêm hai property cô lập** (`p7_nonce_alone_changes_ref_id`,
`p7_did_alone_changes_ref_id`). Chạy lại M6 → đỏ. *Bài học: miền sinh quá rộng làm nhánh
quan trọng không bao giờ được thăm — rộng ≠ mạnh.*

**M9 xanh ở `p4_canonical_injective` là ĐÚNG, không phải test yếu.** Layout hiện tại có
**đúng một** trường biến độ dài (`content_cid`) đứng trước toàn trường cố định, nên tổng độ
dài buffer đã tự xác định `len(cid)` ⇒ encoding vẫn song ánh dù bỏ prefix. Length-prefix ở
đây là **dự phòng**, trở thành load-bearing ngay khi thêm trường biến độ dài **thứ hai**. Đã
ghi chú tại chỗ trong `tests/property.rs`. (Mutation vẫn bị suite bắt — qua
`p4_canonical_roundtrip`, vì decoder đọc prefix nên lệch ngay.)

---

## 7. Ghi chú vận hành

- **P3 tốn nhất.** Nó quét mọi `(n, seq)` và dựng lại chain prefix cho từng `n` ⇒ O(n²) lần
  ký Ed25519; ở 256 case × 12 version nó chiếm ~77 s **một mình**, đắt hơn toàn bộ phần còn
  lại cộng lại. Đã tách block riêng: 48 case × chain ≤ 8. Miền `(n, seq)` vẫn quét **đủ**
  trong mỗi case, chỉ giảm số dãy khác nhau được thử. Cả file nay ~30 s.
- **Golden vector.** P1 dạng property chỉ chứng minh root ổn định *trong cùng một build*;
  câu "trên mọi máy" cần một giá trị NEO. `GOLDEN_MMR_ROOT` =
  `2da5091fd1d666016bb515e675657816c36404ba4f23ea9d92894a8302a56d26`. Đổi bất kỳ khâu nào
  trong canonical/tag/MMR sẽ làm test đỏ — đúng mục đích, vì byte-layout đã cố định ở
  `_CONTRACT.md`.
- **CI không cần sửa gì.** `ci.yml` của PR #31 đã chạy `cargo test --workspace`, phủ luôn
  `tests/property.rs`. Không có phụ thuộc chéo giữa PR này và PR #31 — PR này land được ngay
  cả khi #31 còn kẹt quyền đọc repo `Anchor`.
- **Bẫy tự gặp trong đợt này:** cổng `fmt` từng báo "SẠCH" trong khi thực tế đỏ, vì `$?` sau
  `cargo fmt --all --check | tail -3` là exit-code của `tail` chứ không phải của `cargo`.
  Đúng lớp lỗi "cổng đọc sai nguồn sự thật" đã ghi ở đợt Hydra. Đã chạy lại bằng exit-code
  thật.

---

## 8. Trạng thái S7 sau đợt này

| Bước | Trạng thái |
|---|---|
| pin `rust-toolchain.toml` | ✅ PR #29 |
| `fmt --check` sạch trên main | ✅ PR #30 |
| workflow `ci.yml` | ⏳ **PR #31 — chặn ở tầng QUYỀN** (secret `ANCHOR_READ_TOKEN` / deploy key repo `Anchor`; `lrybi` là `{admin:false, push:true}`) |
| property test P1–P7 | ✅ **đợt này** |

S7 chỉ còn đúng một việc, và việc đó **không nằm trong tay dev**.
