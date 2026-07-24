# STRATA — Trần cứng `<2³²` fail-loud cho canonical length/count-prefix (issue #18)

> Đồng bộ hợp đồng canonical encoding giữa Strata §1.7 và Spectra §1.9. Nguồn: anh Đức mở
> `LampNetCloud/Strata#18` sau khi Spectra vá `#15` (PR `LampNetCloud/Spectra#65`).

## 0. Vấn đề
`u32_be(n) = (n as u32).to_be_bytes()` (`src/lib.rs`) **truncate lặng lẽ** khi `n ≥ 2³²`:
length/count-prefix bị cụt → **mất song ánh** canonical → hai input khác cho cùng byte →
`H_dom` trùng → vỡ tất định băm (lan xuống consumer không phát hiện được). Cùng lớp lỗi
Spectra vừa vá ở §1.9.

## 1. Đối chiếu §1.7 ↔ Spectra §1.9 (task 1)
Soi độc lập trên code Strata: quy tắc 1–4 + `H_dom = BLAKE3(tag ‖ 0x00 ‖ x)` + `DOM_SEP 0x00`
**khớp hoàn toàn** §1.9. **Điểm lệch duy nhất**: trần `<2³²` phía Strata mới ngầm-định-kiểu
(`u32` trong code), chưa phát biểu thành hợp đồng spec. Không thấy lệch nào khác.

## 2. Vá (PR #21)
### Code — fail-loud (land theo quyền code-owner)
- `src/lib.rs::u32_be`: `assert!(n <= u32::MAX)` — **fail-loud, chạy CẢ release** (KHÔNG
  `debug_assert!`). Đặt tại **van chung `u32_be`** thay vì chỉ `canonical_version_bytes`, vì
  cả `version::content_cid`, `field_policy::field_key`, `audit::leaf_bytes` đều đi qua nó ⇒
  một chỗ chặn phủ hết mọi encoder không guard. `batch::entry_bytes` đã guard graceful
  `PayloadTooLarge` (đường Result) từ trước nên không chạm assert.
- 2 test: `boundary_u32_max_ok` (tới `u32::MAX` encode OK) + `over_2pow32_fails_loud`
  (≥2³² panic — chỉ truyền `n`, KHÔNG cấp phát 4GiB).

### Spec — ⚠️ CHỜ ANH ĐỨC DUYỆT
`spec/Strata-Tech.md §1.7` quy tắc 3: soạn sẵn câu trần `<2³²` + fail-mode theo yêu cầu #18
điểm 2. **Spec là miền anh Đức** → giữ trong PR nhưng KHÔNG tự merge phần spec; chờ anh
chỉnh/duyệt wording. Code fail-loud không phụ thuộc câu spec.

## 3. Đường feed ≥4 GiB (task 3)
`content_cid` và `key` (`field_key`) là `Vec<u8>` — **kiểu KHÔNG chặn** dù ngữ nghĩa
`content_cid` = BLAKE3 32B. Không có code path **cố ý** đẩy ≥4 GiB (CID luôn 32B; `field_key`
là tên trường, thực tế nhỏ). Nhưng vì kiểu mở nên phải chốt trần thành **hợp đồng tường
minh**; `assert!` là van thực thi.

*Ngoài phạm vi #18:* `anchor_sink::AnchoredTable::serialize` dùng `(rows.len() as u32)` trực
tiếp cho count — nhưng là **daemon-persist**, KHÔNG hash-canonical §1.7 → để ngoài PR; route
qua `u32_be` được nếu muốn kín (follow-up nhỏ).

## 4. Kiểm chứng
```
cargo test --workspace          → 149 pass (118 lib, +2 test mới); clippy sạch
cargo test --release u32_be_tests → over_2pow32_fails_loud PASS (assert sống ở release)
```

## 5. Trạng thái
| Mục | Trạng thái |
|---|---|
| Code fail-loud `u32_be` + test | ✅ PR #21 (land theo quyền code-owner) |
| Spec §1.7 quy tắc 3 (câu trần) | ⏳ chờ anh Đức duyệt wording |
| Đối chiếu §1.7↔§1.9 (task1) | ✅ khớp, chỉ thiếu phát biểu trần |
| Feed ≥4GiB (task3) | ✅ xác nhận: type mở, cần hợp đồng — đã chốt bằng assert |
| Follow-up `AnchoredTable` persist count | ⏳ tuỳ anh (ngoài §1.7) |
| Spectra#7: API Strata cho INV-SP9 (append_version/prove_field/state_root + TAG_TRACK/TRACKLINK/REGION) | ⏳ chưa làm |
