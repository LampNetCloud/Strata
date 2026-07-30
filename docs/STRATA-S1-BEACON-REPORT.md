# STRATA — S1 follow-up: chống flood-eviction `resolve()` (issue #14) — beacon_mode (A-opt)

> Báo cáo theo dõi đợt audit hội đồng `settlement.rs` (issues #14/#15/#16 + PR #17). Ghi
> nhận quyết định thiết kế, phần đã code (read-side), phần hoãn (write-side), và **đề xuất
> spec chờ anh Đức chốt**.

## 0. Bối cảnh

Audit hội đồng (2026-07-22) phát hiện lỗ hổng **CAO #14**: backend Settlement `resolve()`
quét cửa sổ hữu hạn `resolve_scan_limit=500` các tx của ví publisher (MỚI→CŨ) rồi mới lọc
`input==publisher`. Kẻ tấn công chỉ cần biết địa chỉ publisher (bắt buộc công khai) bơm
≥500 tx rác **gửi tới** publisher → anchor thật rơi ngoài cửa sổ → `resolve→None` dù anchor
còn nguyên on-chain. Hệ quả: DoS đọc/verify permissionless + vô hiệu lớp chống-rollback
INV-E7 cross-process mà `Strata-API.md:504` tuyên bố. Reader bên thứ ba (SuperApp) phơi
nhiễm trực tiếp.

- **PR #17** (regression test) — đã **MERGED** vào `main` (`11e2f64`). Encode #14 thành
  executable spec. Verify: `cargo test` (mặc định) xanh; `-- --ignored` FAIL đúng bug
  (`None` vs `Some(5)`).
- **#15** (spec `§8.1(b):487` mô tả sai payload Settlement) — thuần spec, **để anh Đức**.
- **#16** (gom TB/Thấp) — vài mục code-only, **hoãn** (Thịnh chọn tập trung #14 trước).

## 1. Quyết định thiết kế — hướng A-opt (beacon NFT bật tùy chọn)

Trên UTXO-chain không cấm được người lạ gửi tx tới địa chỉ, và Blockfrost
`/addresses/{addr}/transactions` trộn cả tx vào/ra ⇒ "con-trỏ-latest" chống-flood **bắt
buộc là asset mình kiểm soát cung (NFT)**. Ba hướng cân nhắc:

| Hướng | Miễn nhiễm flood | Giữ "rẻ, không script" | Đổi contract |
|---|---|---|---|
| (A-full) beacon luôn bật | ✅ | ❌ (≈ trùng Mosaic) | mạnh |
| **(A-opt) beacon opt-in** ✅ **chọn** | ✅ khi bật | ✅ mặc định vẫn metadata-only | thêm mode (nhẹ) |
| (B) chấp nhận + ghi rõ | ❌ reader bên thứ ba | ✅ | phải walk back INV-E7 |

**Chốt A-opt** (Thịnh): mặc định giữ nguyên đường metadata-only rẻ; publisher nào cần
reader bên thứ ba verify chống-flood thì **bật `beacon_mode`**.

### Cơ chế beacon (giữ Settlement KHÔNG có Plutus validator)
- Beacon = NFT, **`assetName = ref_id`** (32B ≤ giới hạn 32B Cardano), `policyId` =
  **native minting policy `sig(publisher)`**. Kẻ lạ không ký được ⇒ không mint/di chuyển
  beacon ⇒ flood tx-gửi-tới-publisher KHÔNG chạm tới.
- Mỗi anchor: publisher **tiêu UTxO đang giữ beacon và gửi beacon sang UTxO mới** mang
  metadata anchor (label 1234, `{t,a}`). Beacon "đi tới" luôn nằm trên UTxO anchor mới nhất.
- `resolve(ref_id)`: `unit = policyId ++ ref_id` → hỏi asset-index tx **mới nhất** đụng
  beacon → đọc metadata tx đó. O(1), không quét cửa sổ.
- Trust root vẫn là **khoá publisher** (như hiện tại) — không thêm giả định mới.

## 2. Đã code phiên này — READ-SIDE (nhánh `thinh/strata-14-beacon-resolve`)

`resolve()` chống-flood + cấu hình + query asset. Read-side testable độc lập với write-side.

- **`src/settlement.rs`**
  - `trait ChainQuery`: thêm `asset_latest_tx(unit) -> Result<Option<String>>` (impl **mặc
    định** báo không hỗ trợ → `ChainQuery` chỉ-legacy không phải cài lại).
  - `SinkConfig`: thêm `beacon_policy: Option<String>` (mặc định `None` = legacy).
  - `resolve()` rẽ nhánh: `Some(policy)` → `resolve_via_beacon` (theo asset);
    `None` → `resolve_via_address_scan` (logic cũ, giữ nguyên). Tách helper
    `fold_best_anchor` dùng chung; `hex_lower` inline (không kéo crate `hex` vào core no-I/O).
  - Beacon path **defense-in-depth**: vẫn đối chiếu `input==publisher` của tx beacon mới
    nhất; lệch → fail-closed `Rejected` (không nhầm với "chưa neo").
- **`anchor-io/src/lib.rs`**: `BlockfrostQuery::asset_latest_tx` qua
  `/assets/{unit}/transactions?order=desc&count=1` (404 = asset chưa tồn tại → `None`).
- **`tests/regression_resolve_flood.rs`**: bỏ `#[ignore]`; test bất biến #14 nay chạy ở
  **beacon mode → PASS**. Thêm `legacy_mode_is_blinded_by_flood_documented` chốt đánh đổi
  của đường legacy (KHÔNG phải hồi quy — là lý do beacon tồn tại).

### Kết quả kiểm chứng (local, ổ native)
```
cargo test --test regression_resolve_flood → 3 passed; 0 failed; 0 ignored
  - resolve_must_not_be_blinded_by_flood ......... ok  (beacon: miễn nhiễm flood — done-criterion #14)
  - resolve_control_no_flood ..................... ok
  - legacy_mode_is_blinded_by_flood_documented ... ok  (tài-liệu-hoá giới hạn legacy)
cargo test --workspace → 4 + 116 (1 ignored: live-Preview có sẵn) + 12 + 12 + 3 + 2 pass; 0 failed
cargo clippy --workspace --tests → sạch
```

## 3. WRITE-SIDE — CODE + TYPECHECK XONG, LIVE Preview là bước kế

`beacon_mode` end-to-end cần write-side biết mint/di chuyển beacon. **Đã code + verify tĩnh**
(commit `4048668`):
- **`submitter/submit.ts`** (Lucid Evolution):
  - beacon-walk: mỗi anchor `t=1` → `unit = policyId ‖ ref_id`; chưa có → `mintAssets` (native
    policy `sig(publisher)` qua `scriptFromNative`) + `addSignerKey`; đã có →
    `collectFrom` UTxO giữ beacon; luôn gửi beacon (1 beacon / 1 output) sang UTxO mới mang
    metadata anchor. Native `sig` policy **KHÔNG cần validity-interval** ⇒ tránh bẫy lệch
    đồng hồ host. Chunk 64B giữ nguyên.
  - op `"policy_id"`: suy `policyId` từ `address` (offline, không secret) hoặc từ ví.
  - `initWallet()` tách dùng chung; response beacon kèm `policy_id`.
- **`anchor-io/lib.rs`**: `TsSubmitter.beacon` → gắn `"beacon":true` vào request JSON.
- **`submitter/tsconfig.json` + script `typecheck`** (`tsc --noEmit`) — đóng gap CI API-drift
  Lucid (bài học tx-builder-ci-gap).

### Kiểm chứng tĩnh (chưa on-chain)
```
cargo test --workspace           → 149 pass (field TsSubmitter.beacon)
submit.ts  tsc --noEmit          → exit 0 (khớp type Lucid thật: mintAssets/attach.MintingPolicy/
                                    pay.ToAddress/addSignerKey/utxosAtWithUnit/selectWallet.fromSeed)
scriptFromNative+mintingPolicyToId → policyId 28B (56 hex) OK; paymentCredentialOf exported/callable
```

### ✅ LIVE Preview smoke — ĐÃ CHẠY THẬT ĐẦU-CUỐI (2026-07-24)
Creds preview đọc tại chỗ `A/VeData/.env` (map `oMNEMONICpreview`/`oBLOCKFROST_API_KEY_preview`
→ env submit.ts; KHÔNG in secret). `anchor-io/examples/resolve_beacon.rs` gọi
`SettlementSink::resolve` beacon mode qua `BlockfrostQuery` thật.

- **policyId** = `87935847c3ba708c26525c8b8dea5157f7bea139395349f4af7252f4` (native `sig(publisher)`).
- **publisher** = `addr_test1qptpgpr555n7ge2mdgu9dmwnhvlj39uaw4sy5d4g94x9zawtm59cnx5tl3vaknp89pttmvu89tgk20rlv4732shqyflq2kgt84`.
- **ref_id test** = `1400beac…1400beac`.

| Bước | Tx (Preview) | resolve đọc lại |
|---|---|---|
| MINT beacon seq=0 (hvh=22.., mmr=33..) | `41cffc9f415225849897404b3ac2e519c7b1e7c7721e030e2ebce22e10f8bcf7` | **seq=0**, hvh/mmr khớp ✓ |
| MOVE beacon seq=1 (spend UTxO cũ + đẩy tới; hvh=44.., mmr=55..) | `95a6ff00c7eec89856f227909412f3fe438467eaf05afcfd3357902c822baf38` | **seq=1**, hvh/mmr khớp ✓ |

⟹ Toàn bộ vòng đời **mint → walk → resolve-theo-asset** chạy thật, `resolve` bám đúng latest.
Chống-flood là **bản chất** (kẻ lạ không mint/di chuyển được NFT — không cần flood thật để
chứng minh; unit test `resolve_must_not_be_blinded_by_flood` đã chốt logic). KHÔNG còn là stub.

## 4. ĐỀ XUẤT SPEC — anh Đức đã chốt hướng 2026-07-30 (text canonical do anh Đức ghi)

Beacon là mode mới, có điểm chạm `_CONTRACT.md` + `Strata-API §8.1`. Đề xuất (chưa ghi đè
spec canonical) — bản dưới đã áp 2 hiệu chỉnh anh Đức yêu cầu ở PR #19: tách bạch mức bảo
đảm ở (ii), thêm trần quy mô ở (i).

**(i) Thêm `Strata-API §8.1(d)` — Beacon mode (opt-in):**
> Backend Settlement hỗ trợ tùy chọn `beacon_policy` (native minting policy `sig(publisher)`,
> KHÔNG phải Plutus validator — Settlement vẫn "no on-chain script"). Beacon NFT
> `unit = policy_id ‖ ref_id` (assetName = `ref_id`, 32B). `resolve` ở beacon mode xác định
> anchor mới nhất theo **tx gần nhất đụng asset** thay vì quét cửa sổ địa chỉ ⇒ miễn nhiễm
> flood-eviction (#14). Giao thức beacon-walk: mỗi anchor tiêu UTxO giữ beacon + gửi beacon
> sang UTxO mới mang metadata anchor. Trust root = khoá publisher (không thêm giả định).
>
> **Trần quy mô.** Beacon per-ref_id là 1 NFT trên 1 UTxO cho mỗi `ref_id`, nên min-ADA bị
> khoá tăng tuyến tính theo số `ref_id`. Phù hợp tới **~10⁷ ref_id/publisher** (≈10⁷ × ~1,5
> tADA ≈ 15M ADA khoá — vẫn trong tầm); ở hàng trăm tỷ `ref_id` thì min-ADA vượt tổng cung
> ADA ⇒ per-ref_id KHÔNG mở rộng tới đó. Lối mở rộng là **aggregated-root** (một cây commit
> nhiều `ref_id`, proof `O(log N)`) — việc lớn, để roadmap, không thuộc #14.

**(ii) Hiệu chỉnh `§8.1(b):504` — tách bạch mức bảo đảm, KHÔNG gộp:**
> - **beacon-native** (`beacon_mode`, native policy `sig(publisher)`): **miễn nhiễm
>   flood-eviction** — kẻ lạ không mint/di chuyển được beacon. Nhưng native policy chỉ chặn
>   *mint lại*, **KHÔNG chặn *di chuyển***: chính publisher (hoặc khoá bị chiếm) vẫn spend
>   UTxO giữ beacon rồi gửi beacon sang một anchor `seq` **THẤP hơn**; `resolve` khi đó trả
>   seq thấp — một **rollback đã-xác-thực** mà reader mới không phát hiện được. Không có
>   validator nên không gì ép `seq_out > seq_in` on-chain. ⟹ tính đơn điệu ở mode này **vẫn
>   tin khoá publisher**.
> - **Mosaic A** (Plutus validator spend-recreate ép `seq_out > seq_in`): INV-E7 **độc lập
>   khoá** — chống cả key-compromise / publisher tự rollback.
> - **Quét địa chỉ (legacy)** là **best-effort**: bị flood-eviction làm mù (#14); vẫn đủ cho
>   publisher 1-ref_id / reader tin daemon.
>
> Reader bên thứ ba **chỉ cần chống-flood** → beacon-native đủ. **Cần chống cả
> key-compromise** → phải dùng Mosaic A. Trust root của beacon = khoá publisher, nhất quán
> với (i).

**(iii)** `_CONTRACT.md`: ghi native minting policy beacon KHÔNG vi phạm tính "off-chain
content + commitment" của Settlement (không thêm validator, không đội datum).

*(#15 là mục spec riêng — dòng `§8.1(a):487` mô tả sai payload — để anh Đức xử cùng đợt.)*

## 5. Trạng thái

| Mục | Trạng thái |
|---|---|
| PR #17 regression test | ✅ MERGED (`11e2f64`) |
| #14 quyết định A-opt | ✅ chốt (comment issue #14) |
| #14 read-side (resolve chống-flood) | ✅ CODE + test xanh (nhánh `thinh/strata-14-beacon-resolve`) |
| #14 write-side (beacon-walk submit) | ✅ CODE + `tsc` xanh + **LIVE Preview mint→walk→resolve seq0→seq1** (tx `41cffc9f`, `95a6ff00`) |
| Đề xuất spec §8.1(d) + hiệu chỉnh :504 | ✅ **anh Đức chốt hướng 2026-07-30** (PR #19): tách 2 mức bảo đảm beacon-native vs Mosaic A + thêm trần quy mô ~10⁷ ref_id. §4 đã áp; **text canonical vào spec do anh Đức ghi** |
| #14 land | PR #19 — anh Đức duyệt "chỉnh xong text thì land"; merge sau khi CI xanh |
| #15 spec payload | ⏳ anh Đức xử cùng đợt spec |
| #16 code-only cleanup | ✅ code trong PR #19 (`4a6f8b9` — phân loại lỗi theo code/status thay bare regex), land cùng #14 |

### Ghi nhận từ phản hồi anh Đức (2026-07-30)

Điểm anh Đức bổ sung mà bản đề xuất đầu của mình gộp thiếu chính xác: **native policy chặn
mint-lại nhưng KHÔNG chặn di-chuyển**, nên beacon-native không phải "chống rollback đầy đủ"
— nó chống *outsider*, còn tính đơn điệu vẫn dựa vào khoá publisher. Muốn đơn điệu độc-lập-khoá
thì phải có validator (Mosaic A). Bài học ghi lại: khi một cơ chế chỉ chặn được **một** trong
các đường ghi (mint / move / spend), đừng viết mức bảo đảm bằng một câu gộp.

Hệ quả kỹ thuật đã kiểm: rustdoc `src/settlement.rs:470–473` mô tả beacon **chỉ** ở mức
miễn-nhiễm-flood (không nói chống rollback) ⇒ code KHÔNG hứa quá, không phải sửa theo.
Chốt-đơn-điệu cho reader mới không dựng được thuần client-side (floor do caller cấp chỉ dời
niềm tin sang caller) — đường thật là Mosaic A hoặc aggregated-root, cả hai đã ở roadmap.
