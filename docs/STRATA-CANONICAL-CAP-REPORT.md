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
- `anchor_sink::AnchoredTable::to_bytes`: count-prefix chuyển từ cast `as u32` trực tiếp sang
  **`crate::u32_be`** (2026-07-30). Trước đó doc của hàm ghi format `u32_be(count)` nhưng code
  không đi qua `u32_be` ⇒ câu "van chung phủ hết mọi prefix" chưa đúng. Đây là đường
  **persist**, không phải hash-canonical, nên hậu quả nếu truncate là parse strict trả `None`
  ⇒ **mất bảng đã lưu**, không phải va chạm `H_dom` — nhưng vá để không prefix nào nằm ngoài
  hợp đồng. (Đây là follow-up đã nêu ở §3, nay làm luôn thay vì treo.)

### Spec — TÁCH SANG PR RIÊNG (không giữ trong PR code)
`spec/Strata-Tech.md §1.7` quy tắc 3: câu trần `<2³²` + fail-mode theo yêu cầu #18 điểm 2 đã
soạn, nhưng **spec là miền anh Đức** nên **không gộp cùng code**: PR #21 ban đầu bọc cả hai ⇒
tự chặn chính nó 6 ngày (2026-07-24 → 07-30) chỉ vì một dòng chờ duyệt wording. Đã tách:
code + report land theo quyền code-owner; câu spec đứng PR riêng để anh Đức chỉnh/merge.
**Bài học:** không bọc thay-đổi-miền-người-khác chung PR với phần mình tự land được.

## 3. Đường feed ≥4 GiB (task 3)
`content_cid` và `key` (`field_key`) là `Vec<u8>` — **kiểu KHÔNG chặn** dù ngữ nghĩa
`content_cid` = BLAKE3 32B. Không có code path **cố ý** đẩy ≥4 GiB (CID luôn 32B; `field_key`
là tên trường, thực tế nhỏ). Nhưng vì kiểu mở nên phải chốt trần thành **hợp đồng tường
minh**; `assert!` là van thực thi.

*Ngoài phạm vi #18 — ĐÃ LÀM 2026-07-30:* `anchor_sink::AnchoredTable::to_bytes` từng dùng
`(rows.len() as u32)` trực tiếp cho count (daemon-persist, KHÔNG hash-canonical §1.7). Ban đầu
để ngoài PR, nhưng khi rà lại thấy nó làm **câu spec sai**: spec khẳng định `u32_be` là van
chung cho *mọi* count-prefix mà chỗ này vòng qua. Đã route qua `u32_be`. Chỗ còn lại
`anchor_sink.rs:326` cast `as u32` là **CBOR head có nhánh `u64`** (`n <= u32::MAX` mới cast) —
đúng, không phải length-prefix §1.7, giữ nguyên.

## 4. Kiểm chứng
```
cargo test --workspace            → 168 pass / 0 fail; clippy --all-targets 0 warning
                                    (149 lúc soạn PR 07-24; +19 từ main sau #19/#25/#26)
cargo test --release u32_be_tests → 2/2 PASS, over_2pow32_fails_loud "should panic ... ok"
                                    ⇒ assert SỐNG ở release, không bị tối ưu bỏ
```

## 5. Trạng thái
| Mục | Trạng thái |
|---|---|
| Code fail-loud `u32_be` + test | ✅ **PR #21 MERGED 2026-07-30** (land theo quyền code-owner) |
| `AnchoredTable::to_bytes` count qua `u32_be` | ✅ trong PR #21 — khép câu "van chung phủ hết" |
| Spec §1.7 quy tắc 3 (câu trần) | ⏳ **PR riêng #27** — miền anh Đức, tách khỏi PR code |
| Đối chiếu §1.7↔§1.9 (task1) | ✅ khớp, chỉ thiếu phát biểu trần |
| Feed ≥4GiB (task3) | ✅ xác nhận: type mở, cần hợp đồng — đã chốt bằng assert |
| Follow-up `AnchoredTable` persist count | ✅ đã làm (xem §3) |
| Spectra#7: API Strata cho INV-SP9 (append_version/prove_field/state_root + TAG_TRACK/TRACKLINK/REGION) | ⏳ chưa làm |
