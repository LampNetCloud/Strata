# Strata S4 — Composite Strata (rừng cha–con) + Tabular Merkle Sum Tree — Báo cáo

> **Người:** Thịnh (`lrybi`) — review + merge · **Ngày:** 2026-07-16 · **PR:** `LampNetCloud/Strata#10` (MERGED squash `d899ae2`)
> **Spec:** `Strata-Feat §6/§8` · `Strata-Math §12/§14` · `Strata-API §5.1` · **Contract:** CHỐT-2 (tag sum-tree) + CHỐT-4 (`ref_id` 32B thuần)
> **Nhánh:** `claude/strata-s4-composite-query-tabular` (base `main`) · **Code:** `src/composite.rs` (208) + `src/tabular.rs` (236) + `src/lib.rs` (+9 re-export)

## 1. Xuất xứ + phạm vi

Hai primitive tầng truy vấn cứu từ một nhánh local cũ (tách **trước** đợt merge S1/S2/S3), do agent dựng — anh Đức mở PR #10 nói thẳng "code agent vibe = mặc định nghi ngờ", trách nhiệm cuối + quyền merge là của Thịnh. Đã review lại từ đầu (đọc từng dòng + build/test thật + truy domain-tag tới crate Anchor + đối chiếu không đụng phần đã merge) → **đạt, merge**.

Nhánh cũ còn `query.rs` (FieldProof xuyên version) **KHÔNG kèm** — trùng primitive §8.2 mà `derived_index` (S2/#5) đã có. Đề xuất dọn API tách sang **issue #9** (xử lý riêng, xem §5).

## 2. `composite.rs` — Composite Strata (Feat §6, Math §12, API §5.1)

Đối tượng thật = **rừng** Strata nguyên thủy ghép cha–con. **KHÔNG primitive mới**: một Strata ghép `C` là Strata loại #4 mà `state` chứa tham chiếu con — mỗi con là một trường:

```
field_key   = role          (b"profile", b"posts", b"counters")
field_value = child_ref_id   (32B hash THUẦN — CHỐT-4, KHÔNG class byte)
```

`state_root(C)` cam kết toàn bộ danh sách con; thêm/bớt con = một `append_version` mới của cha (INV-E1/E2 nguyên). Quan hệ đệ quy: con của `C` lại có thể là composite.

| Thành phần | Mô tả |
|---|---|
| `composite_state_fields` / `composite_state_root` | dựng state-fields `(role, ref_id)` → `build_state_root` (tái dùng S-state) |
| `ParentChildProof` + `verify` | field-proof tầng cha `role→child_ref_id`; verify = `verify_field_proof` ∧ key==role ∧ value==child_ref_id |
| `prove_child(children, role)` | sinh proof tầng cha; `None` nếu role không có |
| `link_two_tier(parent, child_anchored_ref_id)` | kiểm **bước NỐI**: proof cha hợp lệ ∧ `child_ref_id` khớp `ref_id` con đã neo. **Nói thẳng**: bind gốc-con → ref_id là qua anchor con riêng, KHÔNG giả vờ bind crypto ở tầng cha |

**Ranh giới tin cậy trung thực:** proof "phần tử x thuộc C" = field-proof cha (role→ref_id) + proof CON (inclusion #1/#2 hoặc field #4) verify độc lập dưới anchor con. `link_two_tier` chỉ nối 2 tầng ở điểm `child_ref_id`. Mỗi tầng `O(log)`.

**4 test:** `composite_root_commits_all_children` (đổi con→root đổi; đảo thứ tự→cùng root), `parent_child_proof_round_trip`, `parent_proof_forged_child_ref_fails` (red-team giả child_ref), `composite_two_tier_proof` (kịch bản profile MXH 2 tầng đầy đủ + red-team nối ref_id khác → fail).

## 3. `tabular.rs` — Merkle Sum Tree một cột (Feat §8, Math §14)

Tổng/đếm/range một cột **CÓ bằng chứng** mà không lộ từng hàng. MST dựng **RIÊNG** trên cột cần tổng (KHÔNG trộn vào `state_root` hồ sơ cấu trúc — mỗi cây một miền, INV-E8). Bọc `lampnet_merkle_anchor::sumtree` (hash-agnostic, cài một lần), Strata dùng `<Blake3Hasher>`.

| Thành phần | Mô tả |
|---|---|
| `ColumnSumTree` (`build/len/root/root_node/total/sum_range`) | cây tổng cột từ `(row_key, u128)`; hand-impl Debug/Clone (tái dựng từ `rows` như `StrataChain`) |
| `RowSumProof` | `row_index` + `value` + `n_rows` + `SumRangeProof` — verify stateless |
| `prove_row` / `prove_range` | proof một hàng `[i,i+1)` / một dải `[a,b)`; `None` nếu ngoài phạm vi |
| `verify_row_sum` / `verify_row_sum_range` | tái dựng root khớp + tổng dải khớp `value` + `count` gốc khớp `n_rows` |

**Domain-tag (đã truy tới crate Anchor):** `TAG_LEAF="LN/STRATA/sum/leaf/v1"`, `TAG_NODE="LN/STRATA/sum/node/v1"` nướng sẵn trong `sumtree.rs` (áp qua `H_dom`) — **tách miền sạch** khỏi `mmr/*`, `state/*`, `ver/*` (mỗi cây suffix riêng, INV-E8 giữ). `u128` (lovelace/đơn vị nhỏ) tránh số thực bất định. Yếu hơn ZK đầy đủ (proof lộ tổng cục bộ anh em — muốn giấu cần blinding/ZK, backlog Math §6.3/§14).

**6 test:** `merkle_sum_tree_total` (tổng=total_sum gốc; proof mỗi hàng; sửa value→root đổi), `row_proof_wrong_value_fails`, `range_sum_proof` (+ khai man tổng dải→fail), `out_of_range_none`, `single_row_table`, `deterministic_root`.

## 4. Kiểm chứng

- `cargo build`: **0 warning**.
- `cargo test`: **121 xanh** (97 lib + 12 anchor_sink + 12 integration), 10 test mới (composite 4 + tabular 6) đều pass, không vỡ phần đã merge.
- Nền: `origin/main` (đã có S1/S2/S3). Anchor pin `rev f864fa3`.

## 5. Nit + issue liên quan

- **Nit `verify_row_sum` (để lại, không chặn merge):** luôn verify dải `[i,i+1)`; đưa proof từ `prove_range(a,b)` với `b>a+1` vào `verify_row_sum` (thay vì `verify_row_sum_range`) sẽ ra `false` — **an toàn** (không nhận nhầm), chỉ là API dễ gọi lộn. Code đã có comment cảnh báo.
- **Issue #9 / PR #11 (MERGED squash `ece19f7`):** dọn `derived_index` (code S2). **Điểm (1) đã làm:** gỡ field `seq` thừa trong `CompositeFieldProof`, `verify_composite` dùng thẳng `version.seq` (bớt 1 đường dựng proof lệch, logic crypto không đổi); test tamper đổi sang `version.seq` (version_hash đổi + vị trí lệch → inclusion fail). **Điểm (2) `prove_field_at_version` → backlog** (chưa caller thật cần dựng proof ngoài đường log/index). Issue đã đóng.
