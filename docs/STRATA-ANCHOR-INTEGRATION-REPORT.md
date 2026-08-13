# Strata — Báo cáo mối nối neo on-chain (OriLife-Core ↔ Strata ↔ Mosaic)

> **Repo:** `LampNetCloud/Strata` · **Mở:** 2026-08-13
> **Phạm vi:** đường neo đầu-cuối từ tầng ứng dụng xuống L1 Cardano — ranh giới module, hợp đồng byte đã ghim, và các mảnh còn thiếu.
> **Vì sao gộp một file:** ba việc dưới đây (review PR #42, trả YC-6 cho OriLife, đo khoảng trống `MosaicBackend`) đều nằm trên **cùng một mối nối**; tách file theo từng PR/issue sẽ làm mất chính bức tranh đó.

---

## 1. Tầng thật sự của đường neo

```
OriLife-Core  (MassTreeIdentify/core/strata_client.py)
   ↓
Strata core   — giữ logic chain, enforce INV-E7 (seq đơn điệu). KHÔNG có dep Cardano.
   ↓  trait AnchorSink                 src/anchor_sink.rs:85
MosaicAnchorSink                       src/anchor_sink.rs:491
   ↓  trait MosaicBackend              src/anchor_sink.rs:474
Mosaic (VeData) — dựng tx + submit + trả phí L1
   ↓
Cardano
```

Ranh giới cố định từ issue #1, ghi ở `src/anchor_sink.rs:7-10`: *"Strata giữ logic chain; **Mosaic giữ tx; KHÔNG dựng tx neo trong Strata**"*.

**Hệ quả cho tầng ứng dụng:** OriLife-Core không cần biết Mosaic tồn tại. Nó nói chuyện với Strata; Mosaic cắm vào đáy Strata. Mọi câu hỏi "OriLife nối thẳng vào Mosaic?" trả lời là **không**.

**Vị trí code Mosaic:** `VeDataIO/Core` dưới `mosaic/` — cả on-chain (`mosaic/aiken/validators/`) lẫn off-chain (`mosaic/tx-builder/`, `merkle-builder/`, `ts/src/intake/`, `ts/src/anchor/`). Repo `VeDataIO/Mosaic` **rỗng** (`git/trees/HEAD` → `409 Git Repository is empty`), đừng nhầm.

---

## 2. Khoảng trống đo được ở seam Mosaic

| Kiểm | Lệnh | Kết quả |
|---|---|---|
| Có impl `MosaicBackend` production? | `grep -rn "impl .* MosaicBackend for" --include=*.rs .` | **1 kết quả duy nhất** — `MockMosaic` ở `tests/anchor_sink.rs:111` |
| Daemon cắm sink nào? | `node/src/bin/strata_node.rs:9-10` | mặc định `DisabledSink` ⇒ **mọi neo thật trả 501** |

⇒ Đường Mosaic CIP-68 hiện **chưa chạy được ngoài test**. Đây là mảnh chặn việc "nối Mosaic vào OriLife".

**Điểm cần làm rõ trước khi code (chưa chốt):** `spec/Strata-API.md:421` ghi backend mặc định **đã chốt = Settlement** (metadata label 1234, đã nghiệm thu on-chain Preview), Mosaic CIP-68 dành cho hồ sơ giá-trị-cao; và *"đường submit production giao VeData vận hành (Strata gửi lô anchor qua **intake**)"*. Tức có thể production **không** đi qua `MosaicBackend` (Rust) mà đi qua **Mosaic intake** (HTTP). Hai hướng này chưa được phân định:

- (A) hiện thực `impl MosaicBackend` trong Rust, gọi Mosaic SDK/Lucid;
- (B) Strata đẩy lô anchor sang Mosaic intake, `MosaicBackend` giữ nguyên vai trò seam cho test.

Chọn (A) hay (B) quyết định hình dạng việc tiếp theo. Chưa tự chốt.

### 2.1 Va chạm chi phí — Mosaic-A không batch được, do THIẾT KẾ

Ba dữ kiện độc lập, cộng lại thành một ràng buộc cứng:

1. `MosaicAnchorSink::publish` nhận **một** anchor mỗi lượt (`src/anchor_sink.rs:585`) — không có API nhận lô.
2. Validator ép **đúng MỘT script input mỗi tx** — `thread.single_script_input`, chính bản vá lỗ gộp-luồng của `Core#50` vòng 2. Tức không thể tiêu nhiều anchor thread trong cùng một tx, **kể cả khi muốn**.
3. `Strata#42` siết `seq' == seq + 1` ở tầng đẩy ⇒ không bỏ qua version nào được.

⇒ **một tx cho mỗi lineage, mỗi version.** Đối chiếu Settlement thì ngược hẳn: `encode_records(records: &[SettlementRecord])` gói **N anchor vào một CBOR array** (`src/settlement.rs:159`), `Submitter::submit` nhận cả slice (`:333`).

Số đo thật từ `VeDataIO/Core: docs/VEDATA-MOSAIC-LOAD-FEE-REPORT.md` (18 mẫu, 0.8948–0.9003 tADA, phí **bất biến theo số record** dưới root):

| Đường | tx | Phí cho 100 cây |
|---|---|---|
| `mosaic_anchor` batch-root (pilot sầu riêng đã dùng) | 1 tx / N record | **~0.896 tADA** |
| Settlement label 1234 | 1 tx / N anchor | **~0.896 tADA** |
| **Mosaic-A CIP-68 (`strata_anchor`)** | **1 tx / 1 lineage** | **~89.6 tADA** |

Chênh **100×** và tăng tuyến tính theo số cây. Report load/fee kết luận *"phí bất biến ⇒ **phải batch**"*; Mosaic-A là đường **duy nhất** không batch được.

**Hệ quả cho việc tích hợp:** "nối Mosaic vào OriLife" phải tách đôi — đội cây quy mô lớn đi batch-root/Settlement, còn Mosaic-A dành cho hồ sơ giá-trị-cao lẻ, vì nó là đường **duy nhất** cho INV-E7 **độc lập khoá** (beacon-native chỉ miễn-nhiễm flood, tính đơn điệu vẫn tin khoá publisher — anh Đức chốt ở PR #19). Đây là đánh đổi bảo mật ↔ chi phí **per-lineage**, không phải chọn một đường dùng chung.

Kèm theo: với Mosaic-A, `AnchorPriority` mất nghĩa "neo thưa" — `Milestone`/`BatchDaily` thành cấu hình hỏng chắc chắn (xem §3 điểm 3).

---

## 3. Review PR #42 — `fix(anchor)`: chặn nhảy bậc seq ở `MosaicAnchorSink`

Tác giả anh Đức, hiện thực hướng (B) chốt 2026-08-07. Bản vá thêm `AnchorError::SeqGap` chặn tại sink trước khi dựng tx, không cho `seq > on_chain_seq + 1` đi lên chuỗi.

**Phần đúng:** bốn nhánh `match` không gộp được; `expected` chỉ đúng seq phải neo tiếp; fail cứng không-retryable đúng phân tầng retry; test kiểm cả nhánh "không đẻ tx" lẫn đường hợp lệ đi tiếp.

**Phát hiện chính — đường GHI không đi qua gác chống đầu độc.** Repo có hai đường đọc on-chain:

| | dùng ở | hợp đồng bảo mật |
|---|---|---|
| `read_anchor()` `src/anchor_sink.rs:486` | `resolve()` | **có** — doc buộc backend trả *mọi* UTxO ứng viên kèm asset, sink lọc theo thread-token NFT one-shot (`:596-602`) |
| `on_chain_seq()` `src/anchor_sink.rs:475` | **`publish()`** `:588` | **không có** — doc đúng một dòng |

`publish()` quyết cả ba nhánh từ `on_chain_seq()`. Nghĩa là `expected_token` / `with_thread_token` / `verify_resolved` chỉ bảo vệ đường ĐỌC.

Không vá được ở phía backend: thứ duy nhất biết thread-token đúng là **sink**, còn `on_chain_seq` trả một số vô hướng nên sink không còn gì để lọc. Về cấu trúc, `on_chain_seq()` **không thể** chống đầu độc.

**Kịch bản hỏng.** Validator `strata_anchor` chỉ có `spend` + `else(_) { fail }` ⇒ **CREATE không được gác**; header validator (`VeDataIO/Core: mosaic/aiken/validators/strata_anchor.ak:22-36`) đã ghi thẳng chuyện "parallel/poisoned thread". Nếu Phase-2 hiện thực `on_chain_seq` theo cách tự nhiên nhất (UTxO mới nhất tại address theo `ref_id`):

1. Kẻ lạ trả một UTxO vào địa chỉ script mang `ref_id` nạn nhân, `seq = 2^63`. Không cần chữ ký, chi phí một min-ADA.
2. Operator thật neo `seq=1` → `on_chain_seq` trả `Some(2^63)` → nhánh `s > anchor.seq` → `RollbackAttempt`.
3. `RollbackAttempt` fail cứng, không retryable ⇒ **lineage kẹt vĩnh viễn** — đúng lỗi mà PR #42 sinh ra để chữa, vào bằng cửa khác và người ngoài mở được.

**Nhánh `None` mới cũng dựa trên mệnh đề chưa quyết định được.** Thread-token là NFT one-shot mint từ seed-UTxO genesis ⇒ **tại lần neo đầu chưa có token nào để pin**, nên "lineage chưa neo" chưa phải mệnh đề phân định được. Ngoài ra `on_chain_seq` trả `Ok(None)` do indexer trễ (là `Ok(None)` hợp lệ, không phải `Err`) sẽ khiến sink CREATE **UTxO genesis thứ hai** cho một `ref_id` đã có luồng sống.

**Điểm thứ ba (nhẹ hơn):** `Milestone`/`BatchDaily` + backend Mosaic nay là cấu hình **hỏng chắc chắn** — luôn `SeqGap` từ lần neo thứ hai, kèm thông điệp đọc như gọi sai thứ tự chứ không như sai cấu hình.

**Trạng thái:** đã gửi review, **chưa merge**, chờ anh Đức trả lời điểm 1 và 2 — [comment](https://github.com/LampNetCloud/Strata/pull/42#issuecomment-5276635810).

---

## 4. Trả YC-6 cho OriLife-Core — 3 issue treo từ 2026-07-08

Ba issue được xếp "Tầng C — chờ Strata trả lời" hơn một tháng. Đối chiếu code cho thấy **hai trong ba đã có sẵn câu trả lời trong repo**, và issue thứ ba không thật sự chờ Strata.

### 4.1 `#161` — byte-layout `canonical(core)`: **TLV length-prefix, KHÔNG phải CBOR**

Đã ghim từ S1 tại `src/version.rs:70`:

| # | trường | encoding | byte |
|---|--------|----------|------|
| 1 | `seq` | u64 big-endian | 8 |
| 2 | `prev_hash` | raw | 32 |
| 3 | `len(content_cid)` | u32 big-endian | 4 |
| 4 | `content_cid` | raw | n |
| 5 | `state_root` | raw | 32 |
| 6 | `author_did` | raw | 32 |
| 7 | `policy_hash` | raw | 32 |
| 8 | `ts` | u64 big-endian | 8 |

`sig` KHÔNG nằm trong `canonical_core` (CHỐT-1). Tổng = **148 + len(content_cid)**. Chỉ `content_cid` có length-prefix vì là trường biến độ dài duy nhất.

```
version_hash = H_dom("LN/STRATA/ver/v1", canonical_core)
H_dom(tag, x) = BLAKE3(tag ‖ 0x00 ‖ x)
```

**5 test-vector** sinh trực tiếp từ `StrataVersion::canonical_core()` + `h_dom` (không viết tay), gồm một **negative control** (`cid="ab"` vs `cid="abcd"`) bắt đúng lỗi bỏ length-prefix. Dán đầy đủ ở [comment #161](https://github.com/OriLifeTrace/OriLife-Core/issues/161#issuecomment-5276762382).

*Câu chữ spec:* `Strata-Math §3.1` ghi "TLV **hoặc** CBOR canonical" — rộng hơn cài đặt thật. Byte-layout không còn treo; chỉ còn việc ghim câu chữ vào `_CONTRACT.md`, phần đó thuộc anh Đức.

### 4.2 `#168` — `ts`: giây u64 BE, đơn điệu **KHÔNG-GIẢM** (`>=`)

Enforce ở `src/chain.rs:201` và `:362` (`if v.ts < head.ts` → `TimestampRegress`); audit-log cùng luật `src/audit.rs:133`.

⇒ **Hai version trong cùng một giây là hợp lệ.** Nỗi lo ban đầu của issue không xảy ra; vector V3/V4 là đúng cặp đó.

**Phát hiện ngược:** phòng thủ `ts = max(prev_ts + 1, now)` (hướng dẫn 07-08) là siết chặt hơn ràng buộc thật và **có hại** — nó đẩy `ts` vượt thời gian thật khi burst, mà (a) `ts` nằm trong `canonical_core` nên đã ký, vĩnh viễn; (b) `StrataChain::version_at(t)` (`src/chain.rs:477`) binary-search theo `ts`, nên truy vấn "giá trị tại thời điểm t" trả lời lệch. Đề xuất `max(prev_ts, now)` — khớp đúng `>=`. Đã trình bằng chứng, mời anh Đức đổi lại; **chưa tự chốt**.

### 4.3 `#167` — domain-tag `genesis_nonce`: Strata không quy định, tag không tồn tại

`src/refid.rs:20` `gen_ref_id_raw(author_did, nonce)` nhận `nonce` như **tham số đục** — không ràng buộc độ dài/nguồn gốc/ngẫu nhiên. Bảng domain-tag Strata (grep toàn `src/`) **không có `extkey/v1`**.

⇒ Suy `nonce` tất định từ `external_key` là quyết định của OriLife. Hai lưu ý đã gửi:

- **(a)** đừng đặt tag trong namespace `LN/STRATA/` (CHỐT-2 là bảng tag duy nhất của Strata, va tag sau này không phát hiện được); dùng `ORILIFE/extkey/v1`.
- **(b) cảnh báo bảo mật, nối thẳng vào §3:** `nonce` tất định ⇒ `ref_id` **đoán trước được**. Ghép với CREATE không gác chữ ký, kẻ lạ **dựng sẵn anchor thread độc cho một cây TRƯỚC KHI cây được neo lần đầu** — trường hợp xấu nhất, vì lúc đó chưa có thread thật nào để đối chiếu. Nếu cần `ref_id` tái lập, trộn bí mật per-deployment: `nonce = H_dom("ORILIFE/extkey/v1", external_key ‖ site_secret)`.

---

## 5. Việc còn mở

| # | Việc | Chờ ai |
|---|---|---|
| 1 | PR #42 — đường GHI có chuyển sang `read_anchor()` không; nhánh `None` giữ mở hay fail cứng | anh Đức |
| 2 | Chọn hướng (A) `impl MosaicBackend` Rust vs (B) Strata → Mosaic intake | anh Đức + VeData |
| 3 | `ts` — đổi hướng dẫn OriLife từ `max(prev_ts+1, now)` sang `max(prev_ts, now)` | anh Đức |
| 4 | Ghim "TLV, không CBOR" vào `_CONTRACT.md` (câu chữ spec) | anh Đức |
| 5 | Land 5 test-vector `canonical_core` thành fixture cố định Rust↔Python (mẫu như PR #33 làm cho label 1234) | Strata — làm được ngay |
| 6 | Thread-NFT one-shot bắt buộc cho anchor thread (đóng lỗ CREATE) | chưa có nhà — xem `Core#50` MB-5 |

---

## 6. Lưu vết phương pháp

- **Đừng suy kiến trúc từ tên repo.** Org `VeDataIO` có repo `Mosaic` nhưng nó rỗng; toàn bộ Mosaic ở `Core/mosaic/`. Kiểm bằng `gh api .../git/trees/HEAD`, không bằng `gh repo list`.
- **Hai đường đọc trông giống nhau nhưng khác hợp đồng bảo mật.** `read_anchor()` có, `on_chain_seq()` không — và điều đó chỉ lộ ra khi đọc doc của *cả hai*, vì tên hàm không nói gì. Khi một trait có hai cách hỏi cùng một sự thật, phải hỏi cách nào mang gác.
- **"Chờ team khác trả lời" cần kiểm lại định kỳ.** Ba issue treo hơn một tháng, hai trong số đó câu trả lời đã nằm sẵn trong code từ S1. Nhãn "Tầng C — chờ X" không tự hết hạn.
