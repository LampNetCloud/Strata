# STRATA — S7: CI + parity Rust↔TS (issue #24)

> Milestone **S7** (`STRATA-ROADMAP.md §2`). Phạm vi đợt này: dựng gate cho repo vốn
> **không có workflow nào**, và khoá điểm chạm hai-ngôn-ngữ của codec label 1234.
> **Cập nhật:** 2026-07-30.

---

## 0. Vấn đề

`gh pr checks` trên repo này trả **"no checks reported"** — mọi lệnh `cargo test` /
`clippy` / `fmt` / `tsc` tới giờ chỉ đúng ở máy nào có chạy. Cùng lớp gap đã gặp ở
`VeDataIO/Glint#12` và gói `tx-builder` của Mosaic: **gói nằm ngoài mọi workflow thì test
của nó là trang trí**.

Triệu chứng lộ ra khi land #19: `cargo fmt --all -- --check` **đỏ 8 hunk ngay trên `main`**,
ở cả file mà PR đó không đụng (`src/derived_index.rs`, `tests/settlement.rs`). Không phải
regression — `rustfmt` mới hơn lúc viết code, mà repo **không pin toolchain**.

## 1. Thứ tự thực hiện (có ràng buộc, không đảo được)

| Bước | PR | Vì sao phải đúng thứ tự này |
|---|---|---|
| 1. Pin toolchain | **#29** ✅ MERGED | Sweep trước khi pin thì kết quả trôi theo máy ⇒ phải sweep lại |
| 2. `cargo fmt --all` một lượt | **#30** ✅ MERGED (`e80806b`) | Bật `fmt --check` trước khi sweep thì **mọi PR đỏ oan** |
| 3. Workflow `ci.yml` | **#31** ⏳ mở | Chặn ở tầng quyền — xem §4 |
| (kèm) Parity Rust↔TS | **#33** ✅ MERGED (`0fefc0b`) | Có test rồi mới có cái để đưa vào CI |

## 2. Đã land

**#29 — `rust-toolchain.toml`**: pin `channel = "1.96.0"` + `components = ["rustfmt", "clippy"]`.
Không format lại gì trong PR đó.

**#30 — fmt sweep**: 5 file, +45/−21, **diff thuần format** (xuống dòng, gom tham số, bọc
struct literal). Sau bước này `cargo fmt --all -- --check` **sạch 0 hunk** trên `main` ⇒
`fmt --check` bật làm gate được mà không đỏ oan.

**#33 — test-vector chung cho metadatum label 1234.** Đây là phần đáng kể nhất của đợt.

## 3. Parity Rust↔TS (#33)

### 3.1 Điểm chạm thật nằm ở đâu

Ban đầu định làm fixture cho **merkle root**, nhưng kiểm ra thì **sai chỗ**: Strata dùng
`lampnet-merkle-anchor` (BLAKE3, `H_dom(tag ‖ 0x00 ‖ x)`, root = `H_dom(tag, u64_be(n) ‖ peaks)`),
còn `mosaic/merkle-builder` bên VeData dùng **SHA3-256**, tag byte `0x00/0x01/0x02`, root =
`SHA3(0x02 ‖ popcount(N) u32be ‖ peaks)`, leaf còn có `salt_secret`. **Hai hệ neo khác nhau
cho hai spec khác nhau, không phải một thứ cài hai lần** ⇒ so root bằng nhau là vô nghĩa.

Điểm chạm thật: **`anchor-io/submitter/submit.ts` dựng metadatum label 1234 độc lập với
`src/settlement.rs`** — cùng layout `[{t, a}]`, cùng luật chunk 64B, viết hai lần bằng hai
ngôn ngữ. Không gì bắt được lúc hai bên trôi khỏi nhau: TS ghi lên chain một hình dạng mà
`decode_records` từ chối, và lỗi **chỉ lộ khi `resolve` thật**.

### 3.2 Cách khoá

| Thành phần | Vai trò |
|---|---|
| `apis/settlement-metadata.json` | Test-vector **chung**, 8 ca. Sinh bởi `cargo run --example dump_settlement_fixture` — **Rust ra đề vì Rust giữ decoder**; bên phải-khớp không nên tự ra đề |
| `tests/settlement_fixture.rs` | Khoá phía Rust: encode khớp CBOR hex · round-trip qua decoder · chặn việc rút gọn vector |
| `anchor-io/submitter/test/fixture.test.ts` + `npm run test:fixture` | Khoá phía TS trên **cùng** file vector |

Ca biên có chủ ý: 63B · **64B (cấm chunk)** · **65B (64+1)** · 128B (64+64) · 129B (64+64+1) ·
`seq` multi-byte · nhiều record trong một metadatum.

Refactor kèm theo, không đổi hành vi: tách `buildMetadata()` thành **hàm thuần + export**
(trước nằm trong `main()` nên cách duy nhất để kiểm là chạy tx thật), và `main()` chỉ chạy
khi gọi trực tiếp để `import` từ test không đụng ví/mạng.

### 3.3 Lỗ hổng trong chính test này — tự tìm ra, đã vá

Bản test đầu chuẩn hoá **"bytestring trần"** và **"mảng chứa đúng 1 chunk"** về cùng một
dạng `[hex]`. Hậu quả: cố tình đổi ngưỡng chunk 64→32 mà test **vẫn xanh 8/8** — trong khi
đó đúng là malleability decoder Rust từ chối (*"≤64B mà lại chunk"*).

**Hàm chuẩn hoá làm mất đúng cái khác biệt cần bắt thì test thành trang trí.** Vá: vector +
`normalize` giữ phân biệt **≤64B = chuỗi JSON, >64B = mảng JSON**.

**Negative control sau khi vá:**

| Phá gì ở phía TS | Kết quả |
|---|---|
| ngưỡng chunk 64 → 32 | ❌ 2 ca đỏ |
| bước chunk 64 → 32 | ❌ 4 ca đỏ |
| đảo thứ tự `ref_id` ↔ `head_version_hash` | ❌ 3 ca đỏ |
| khôi phục nguyên trạng | ✅ 8/8 xanh |

**Luật rút ra:** test parity phải chạy negative control **trước khi land**, phá từng thứ một.

## 4. Vì sao #31 chưa merge — chặn ở tầng QUYỀN

`Cargo.toml` pin `lampnet-merkle-anchor` từ repo **private** `LampNetCloud/Anchor` (rev
`f864fa3`). `GITHUB_TOKEN` mặc định của Actions **không đọc được repo khác** ⇒ `cargo fetch`
fail auth. Repo Strata hiện có **0 secret**.

Không tự gỡ được, hai lẽ:
1. **Fine-grained PAT chỉ tạo được trên web UI** — GitHub không có API mint token.
2. **Đặt secret đòi quyền admin repo**, mà `lrybi` là `{admin: false, push: true}` trên
   **cả** `Strata` lẫn `Anchor`.

Đã hỏi anh Đức ở comment #31 với ba hướng: thêm secret `ANCHOR_READ_TOKEN` · **cho `Anchor`
public** · cấp admin. Nghiêng hướng public: crate là sub-primitive **đã đóng băng** (1
commit từ 2026-07-03, không chứa secret), và nó gỡ rào cho **mọi** consumer thay vì mỗi
consumer xin token một lần.

> Rào này **lặp lại đúng chuyện #2/#4** hồi 2026-07-03: lần đó **máy dev** thiếu quyền đọc
> repo private (anh Đức xử bằng cách tách `Anchor` ra + cấp read); lần này **runner CI**
> thiếu quyền — mà runner thì không add-collaborator được.

Workflow có sẵn **bước preflight** kiểm secret và in hướng dẫn thẳng vào log, thay vì để
`cargo fetch` chết với lỗi auth khó đọc.

## 5. Số đo (tại máy, toolchain 1.96.0 đã pin)

```
cargo test --workspace                 → 171 pass / 0 fail   (168 + 3 test fixture mới)
cargo clippy --workspace --all-targets → 0 warning
cargo fmt --all -- --check             → sạch (0 hunk)
tsc --noEmit                           → exit 0
npm run test:fixture                   → 8/8 case khớp vector chung
```

## 6. Còn lại của S7

- **#31** — chờ quyền (§4). Merge được ngay khi có secret hoặc `Anchor` public.
- **Property test P1–P7** (`Strata-Tech §9.3`) — **chưa có** (`proptest`/`quickcheck` = 0 hit).
  Đây là phần còn lại lớn nhất của S7: `mmr_root_deterministic` · `mmr_inclusion_complete` ·
  `mmr_extend_monotone` · `canonical_roundtrip` · `state_root_order_independent` ·
  `ts_monotone_enables_version_at` · `ref_id_collision_resistance`.
- Cân nhắc mở rộng khuôn vector chung sang các điểm chạm hai-ngôn-ngữ khác nếu phát sinh.
