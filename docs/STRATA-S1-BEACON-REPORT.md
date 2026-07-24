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

### ⏳ LIVE Preview smoke (bước kế — cần creds preview ở A/VeData/.env)
Kịch bản: (1) `op:policy_id` lấy address+policyId; (2) submit beacon `seq=0` (mint) → txid;
(3) chờ confirm; (4) `resolve` (BlockfrostQuery beacon mode) đọc lại đúng anchor; (5) submit
`seq=1` (di chuyển beacon) → resolve trả `seq=1`. ⚠️ **CHƯA merge như end-to-end** trước khi
smoke này xanh (bài học Hydra L1-fallback-stub). `beacon_mode` mặc định off nên nhánh an toàn.

## 4. ĐỀ XUẤT SPEC — chờ anh Đức chốt (miền spec)

Beacon là mode mới, có điểm chạm `_CONTRACT.md` + `Strata-API §8.1`. Đề xuất (chưa ghi đè
spec canonical):

**(i) Thêm `Strata-API §8.1(d)` — Beacon mode (opt-in):**
> Backend Settlement hỗ trợ tùy chọn `beacon_policy` (native minting policy `sig(publisher)`,
> KHÔNG phải Plutus validator — Settlement vẫn "no on-chain script"). Beacon NFT
> `unit = policy_id ‖ ref_id` (assetName = `ref_id`, 32B). `resolve` ở beacon mode xác định
> anchor mới nhất theo **tx gần nhất đụng asset** thay vì quét cửa sổ địa chỉ ⇒ miễn nhiễm
> flood-eviction (#14). Giao thức beacon-walk: mỗi anchor tiêu UTxO giữ beacon + gửi beacon
> sang UTxO mới mang metadata anchor. Trust root = khoá publisher (không thêm giả định).

**(ii) Hiệu chỉnh `§8.1(b):504` (INV-E7 hai lớp):**
> Làm rõ: lớp cross-process của backend Settlement bằng **quét địa chỉ (legacy)** là
> **best-effort** — bị flood-eviction làm mù (#14); đảm bảo cross-process chống-rollback đầy
> đủ cho reader bên thứ ba **chỉ khi bật `beacon_mode`** (hoặc dùng backend Mosaic A). Legacy
> vẫn đủ cho publisher-1-ref_id / reader tin daemon.

**(iii)** `_CONTRACT.md`: ghi native minting policy beacon KHÔNG vi phạm tính "off-chain
content + commitment" của Settlement (không thêm validator, không đội datum).

*(#15 là mục spec riêng — dòng `§8.1(a):487` mô tả sai payload — để anh Đức xử cùng đợt.)*

## 5. Trạng thái

| Mục | Trạng thái |
|---|---|
| PR #17 regression test | ✅ MERGED (`11e2f64`) |
| #14 quyết định A-opt | ✅ chốt (comment issue #14) |
| #14 read-side (resolve chống-flood) | ✅ CODE + test xanh (nhánh `thinh/strata-14-beacon-resolve`) |
| #14 write-side (beacon-walk submit) | ✅ CODE + `tsc` xanh (`4048668`); ⏳ LIVE Preview smoke bước kế |
| Đề xuất spec §8.1(d) + hiệu chỉnh :504 | ⏳ chờ anh Đức chốt |
| #15 spec payload | ⏳ anh Đức |
| #16 code-only cleanup | ⏳ hoãn (sau #14) |
