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

## 5. Việc còn mở (cập nhật 2026-08-14)

| # | Việc | Chờ ai |
|---|---|---|
| 1 | PR #42 — đường GHI chuyển sang `read_anchor()`; nhánh `None` fail cứng; chặn `AnchorPriority` thưa ở constructor | anh Đức — **đã trả lời bằng chữ 13/08, code CHƯA đẩy** (`38cd2c9c` vẫn là commit duy nhất) |
| 2a | **Đội cây OriLife đi đường nào** | ✅ **CHỐT 2026-08-14 — batch-root/Settlement.** Mosaic-A giữ cho hồ sơ giá-trị-cao lẻ. Xem §9.3 |
| 2b | Chọn hướng (A) `impl MosaicBackend` Rust vs (B) Strata → Mosaic intake | ✅ **CHỐT 2026-08-14 — (B), dạng B1′**: Mosaic quyết lô · Strata kiểm INV-E7 + encode · Mosaic dựng tx + submit. Xem §9.6 |
| 2c | Luật `Strata#1` *"KHÔNG dựng tx neo trong Strata"* đã bị vượt trên thực tế — sửa luật hay khoanh phạm vi | anh Đức — xem §9.7 |
| 2d | Beacon `policyId` phụ thuộc khoá ví publisher, mà đích là **ví của chính người dùng** | chưa có nhà — xem §9.8, nối `VeDataIO/Core#87` |
| 3 | `ts` — đổi hướng dẫn OriLife từ `max(prev_ts+1, now)` sang `max(prev_ts, now)` | anh Đức |
| 4 | Ghim "TLV, không CBOR" vào `_CONTRACT.md` (câu chữ spec) | anh Đức |
| 5 | ~~Land test-vector `canonical_core` thành fixture cố định Rust↔Python~~ | ✅ **XONG** — PR #47 **MERGED** |
| 6 | Thread-NFT one-shot bắt buộc cho anchor thread (đóng lỗ CREATE) | chưa có nhà — xem `Core#50` **MB-5 / P0b** |
| 7 | ~~Enforce `DuplicateFieldKey` (E6) — `#39` điểm 2~~ | ✅ **XONG** — PR #50 **MERGED** (hoà giải với #48 — xem §9.2) |
| 8 | ~~Vector `state_root` + chốt encoding `field_value_bytes` cho OriLife~~ | ✅ **XONG** — PR #47 (S1–S6); phía OriLife `#324` **MERGED 2026-08-14 04:07** |
| 9 | CI repo — còn đúng một nút: khoá đọc `LampNetCloud/Anchor` + `gh secret set` | anh Đức (cần admin repo) — **chặn PR #31**, xem §9.4 |
| 10 | Spec `#40` phải gộp 3 mục mới trước khi land | 2/3 đã xác định (từ #48 đã land), mục thứ 3 chờ #42 — xem §9.4 |

---

## 7. Đợt 2026-08-13 (b) — dọn đường trước phiên critical path

Ba việc dưới đây đều **không chờ ai**, nên làm trước để phiên nối OriLife → Strata → Mosaic không phải dừng giữa chừng.

### 7.1 Vector `state_root` — đóng điểm chặn cuối của đường ký OriLife

`canonical_core` đã có 5 vector, nhưng `state_root` thì **chưa** — trong khi nó là **trường #5 của `canonical_core`**, tức nằm trong `version_hash`, tức **được ký**. Bên OriLife viết `build_state_root` **từ spec, không có vector đối chiếu** (`#324` tự khai). Lệch ở đó là ký sai vĩnh viễn, và chỉ lộ khi `BadSignature` 403 trên máy chủ thật.

Đã thêm **6 vector**, mỗi ca khoá một tầng khác nhau:

| | Ca | Khoá cái gì |
|---|---|---|
| S1 | rỗng | root = **32 byte 0**, không phải `H_dom(tag, "")` — ca dễ cài trượt nhất |
| S2 | một lá | root **BẰNG** leaf, không tầng nút nào chạy |
| S3 | ba lá | lá lẻ **carry nguyên**, không nhân đôi (CVE-2012-2459) |
| S4 | bốn lá, thứ tự đảo | sort theo key là **bắt buộc** |
| S5 | khoá **28 byte** | xem §7.2 |
| S6 | CID 32 byte | `field_value_bytes` là **byte đã giải mã**, không phải hex ASCII |

Fixture in **cả trung gian** (`fvh`, `leaf`), không chỉ root: bên phải-khớp chỉ có root thì lúc lệch không biết lệch tầng nào và phải đoán ngược.

**S6 trả câu hỏi còn treo của anh Đức** (`#161`): CID 64-hex vào `field_value_bytes` là **32 byte đã giải mã**. Nguồn không phải suy luận — `state.rs` khai thẳng *"content_cid THUẦN (CHỐT-4 — KHÔNG class byte, để field-proof không leak loại)"*. Lý do mạnh hơn quy ước: băm chuỗi hex làm **độ dài tự khai ra loại** của trường (64 ký tự ⇒ "đây là CID"), đúng thứ field-proof cố ý giấu.

### 7.2 Lá và nút dùng chung miền băm — lớp phòng vệ không ai giữ

Lợi (OriLife) tìm ra ở `#324` mục A: sửa cho hai domain-tag lá/nút giống hệt nhau thì **cả 25 bài kiểm bên họ vẫn xanh**. Kiểm phía Strata: **y hệt**.

Số của bạn ấy đúng — tiền ảnh lá là `u32_be(len(key)) ‖ key ‖ fvh`, nên khoá dài **28 byte** cho `4 + 28 + 32 = 64` byte, bằng **đúng** tiền ảnh nút `left(32) ‖ right(32)`. Khi hai độ dài trùng, thứ **duy nhất** ngăn một nút trong bị khai là một lá là hai tag khác nhau.

Đã thêm test riêng + mutation-test chính nó:

```
TAG_STATE_NODE := TAG_STATE_LEAF   ⇒ 2 test ĐỎ đúng chỗ
bỏ v.sort_by trong sorted_leaves   ⇒ ĐỎ ở vế đảo thứ tự
```

Bài học đáng giữ: đây là **lỗi dọn mã**, không phải lỗi logic. Người gộp hai dòng gần giống nhau sẽ không thấy gì sai, và không có test thì CI đồng ý với họ.

### 7.3 Trùng key — cả hai bên cùng hở, không bên nào canh

Lợi hỏi sang: Strata có chốt tên trường phải duy nhất không? **Có** — `#40` P6 (chỉ `state_fields`, không `field_policy`, reject kể cả same-value).

**Nhưng Strata cũng chưa enforce** — `grep DuplicateFieldKey` trả về rỗng. Nên mệnh đề *"hậu quả vẫn nổ ra ngoài chứ không ghi sai âm thầm"* trong review của Lợi **không đúng hôm nay**: nó dựa vào việc Strata từ chối, mà Strata chưa từ chối.

Nguyên nhân giống hệt hai bên: `sorted_leaves` dùng `sort_by`, mà `sort_by` của Rust là sort **ổn định** ⇒ hai mục trùng key giữ thứ tự đầu vào ⇒ **đảo thứ tự, đổi root**. Root đó được ký.

Đã vá ở **PR #50**, gác đặt ở **biên** (`dto.rs::to_pairs`, điểm chuyển đổi duy nhất cho cả `create` lẫn `append`) chứ không đổi chữ ký `build_state_root` — hàm đó vô-lỗi và được gọi nhiều chỗ nội bộ nơi field đã qua cửa. Ra **400 `MalformedRequest`** (đã kiểm `ApiError::Malformed → StatusCode::BAD_REQUEST`, không tin theo tên biến).

Ghi thêm: docstring `prove_field` **đã tự khai** giả định *"key duy nhất sau khi caller bảo đảm"* từ trước — đúng loại giả định sống lâu vì nó nằm trong comment chứ không nằm trong cửa.

### 7.4 Phía Core cùng đợt

`Core#90` đã merge sau khi sửa 3 điểm anh Đức nêu, kèm hai hạng mục mới có người đứng tên: **MB-6** (đối ứng VeData cho đường submit production — `Strata-API.md:421` giao một phía) và **MB-7** (`mosaic_update` gộp 2-input→1-output = **mất tài sản**, không phải DoS). Chi tiết ở `VeDataIO/Core` → `docs/VEDATA-ROADMAP.md`.

Điểm chạm với repo này: **MB-6 câu 2** (đội cây OriLife đi batch-root hay Mosaic-A) **nặng hơn câu 1** (hướng (A)/(B) của S11) — trả câu 2 trước có thể làm câu 1 thành không cần trả lời.

---

## 8. Lưu vết phương pháp

- **Đừng suy kiến trúc từ tên repo.** Org `VeDataIO` có repo `Mosaic` nhưng nó rỗng; toàn bộ Mosaic ở `Core/mosaic/`. Kiểm bằng `gh api .../git/trees/HEAD`, không bằng `gh repo list`.
- **Hai đường đọc trông giống nhau nhưng khác hợp đồng bảo mật.** `read_anchor()` có, `on_chain_seq()` không — và điều đó chỉ lộ ra khi đọc doc của *cả hai*, vì tên hàm không nói gì. Khi một trait có hai cách hỏi cùng một sự thật, phải hỏi cách nào mang gác.
- **"Chờ team khác trả lời" cần kiểm lại định kỳ.** Ba issue treo hơn một tháng, hai trong số đó câu trả lời đã nằm sẵn trong code từ S1. Nhãn "Tầng C — chờ X" không tự hết hạn.
- **Một lớp phòng vệ không có test là một lớp phòng vệ sắp mất.** Cách phát hiện duy nhất là **cố ý phá nó rồi chạy lại bộ kiểm** — đọc mã thì nó vẫn trông đúng. Lợi tìm ra chỗ lá/nút bằng đúng cách đó, và bên Strata hở y hệt.
- **"Bên kia sẽ canh" phải kiểm, không được giả định.** Review của Lợi hạ mức mục trùng key vì tin Strata từ chối; Strata thì chưa. Hai bên cùng dựa vào nhau là **không bên nào gác** — và nó đọc như đã gác.
- **Gác đặt ở biên nhận dữ liệu ngoài, không đặt ở hàm tính.** Đổi chữ ký `build_state_root` là đổi lan man qua `derived_index`/`composite` nơi field đã qua cửa, mà vẫn không gác chỗ dữ liệu thật đi vào.

---

## 9. Đợt 2026-08-14 — phiên critical path: dọn hàng chờ PR + chốt câu 2

Vào phiên với **9 PR mở**. Ra phiên với **3** — và câu hỏi nặng nhất của MB-6 đã có đáp án.

### 9.1 Sáu PR đã land, mỗi cái qua một phép thử **cố ý phá**

Không PR nào được merge chỉ vì "test xanh". Với mỗi cái, gác chính bị gỡ ra rồi chạy lại bộ kiểm — nếu bộ kiểm vẫn xanh thì cái gác đó chưa được ai canh.

| PR | Nội dung | Phép thử | Kết quả |
|---|---|---|---|
| **#48** (anh Đức) | 6 lỗ cổng daemon + route khô `_canonical` | `is_allowed` → `if false` | ✅ ĐỎ đúng 1 bài |
| **#48** | trần `ts` hai lớp | `TS_MILLIS_FLOOR` → `u64::MAX` | ❌ **193/193 VẪN XANH** — xem §9.2 |
| **#50** | gác trùng key `state_fields` | (hoà giải, xem §9.2) | — |
| **#47** | vector `canonical_core` + `state_root` | hoán `policy_hash` ↔ `author_did` trong encoder | ✅ ĐỎ `vectors_khop_encoder` |
| **#49** | nửa ÂM fixture label 1234 | `if chunks.len() < 2` → `if false` | ✅ ĐỎ `must_reject_thi_decoder_phai_tu_choi` |
| **#51** | seam đồng hồ cho `check_ts` | lặp lại mutation của #48 | ✅ nay ĐỎ |
| **#27** | spec Tech §1.7 trần `<2³²` | (spec, đã đối chiếu mã ở vòng review trước) | — |

Số test: **185 → 208**. `cargo fmt --all -- --check` và `cargo clippy --workspace --all-targets -- -D warnings` exit 0 ở mọi mốc.

Chọn mutation có chủ ý, không lấy chỗ dễ. Ví dụ ở **#47**: hoán hai trường **cùng độ dài 32 byte** ở vị trí liền nhau — không đổi tổng độ dài, nên mọi bài kiểm chỉ so `len` đều bỏ lọt. Đó đúng là lớp lỗi mà bảng vector sinh ra để bắt.

### 9.2 Hai chỗ chỉ lộ ra vì chạy thật, không lộ ra khi đọc mã

**(a) Trần `ts` của #48 mutation-survivable — lớp phòng vệ không ai canh.**

`check_ts` có hai lớp: trần tuyệt đối `TS_MILLIS_FLOOR = 10¹²` (không cần đồng hồ) và biên lệch `±300 s` quanh đồng hồ daemon. Gỡ **hẳn** lớp 1 thì **193/193 vẫn xanh** — kể cả bài mang tên `ts_in_milliseconds_rejected_by_absolute_ceiling_not_by_clock`, chính cái tên hứa là nó kiểm trần chứ không kiểm đồng hồ.

Không phải bài kiểm viết ẩu, mà là **không viết khác được từ phía HTTP**: mọi `ts` vượt `10¹²` thì cũng vượt `now + 300` với `now` là đồng hồ thật (`≈ 1,79 × 10⁹`). Hai lớp **trùng miền trên mọi đầu vào mà một request gửi tới được**.

Ca duy nhất lớp 1 gánh một mình: `now == None` — đồng hồ đặt **trước** epoch ⇒ `duration_since(UNIX_EPOCH)` lỗi ⇒ lớp 2 bị `if let Some(now)` bỏ qua. Không tới được ca đó chừng nào `SystemTime::now()` còn gọi thẳng trong hàm.

Vá ở **#51**: tách `check_ts_at(ts, now: Option<u64>)` thuần, `check_ts` gọi nó với `now_secs()`. Không đổi một byte hành vi. Ba bài kiểm, và **vế thứ hai quan trọng ngang vế thứ nhất** — `check_ts_at(ts_giây_thật, None)` phải `Ok`; thiếu vế đó thì một hiện thực `Err`-luôn cũng làm bài thứ nhất xanh, và daemon trên máy đồng hồ hỏng sẽ từ chối sạch mọi ghi mà CI vẫn báo đạt.

**(b) #48 và #50 vá CÙNG một chỗ — phát hiện trước khi merge, không phải sau.**

Cả hai cùng gác trùng key ở `to_pairs`. Hoà giải chứ không chọn một bên: giữ docstring của #48 (kể hậu quả đầu-cuối — hai field-proof cùng khoá `diagnosis`, một trả `aa` một trả `bb`, cùng `state_root` đã neo, non-repudiation sụp), giữ hiện thực của #50 (`find_duplicate_key` đặt ở `src/state.rs` cạnh `build_state_root`, `pub`), gộp thêm lẽ **độc lập** về đảo-thứ-tự-đổi-root.

Một chỗ cố ý **không** viết cho gọn: nợ ở lõi **không** khép lại bằng việc có `find_duplicate_key`. Docstring nói thẳng — đội tích hợp gọi thẳng crate nay *có sẵn hàm để gọi*, nhưng **gọi hay không vẫn là lựa chọn của họ**. Viết "đã có hàm" mà bỏ vế sau thì lần đọc sau sẽ tưởng `#39` đóng được.

Và một bẫy nhỏ đi kèm: thông điệp lỗi thống nhất theo từ ngữ #48 ("khoá trùng") ⇒ **2 assertion trong test #50 phải đổi theo**. Không đổi thì test vẫn xanh — `expect_err` bắt đúng lỗi, chỉ có `assert!(err.contains(...))` là so vào một chuỗi không còn tồn tại. Đúng lớp *test mang tên mạnh hơn nội dung*.

### 9.3 CHỐT câu 2 của MB-6 — đội cây OriLife đi **batch-root/Settlement**

Chốt 2026-08-14 (phía VeData). Mosaic-A **không** dùng cho đội cây; giữ cho **hồ sơ giá-trị-cao lẻ**.

Căn cứ, theo thứ tự sức nặng:

1. **Chi phí, đo thật.** 100 cây: `~0,896 tADA` (batch-root/Settlement, 1 tx / N record) vs `~89,6 tADA` (Mosaic-A, 1 tx / 1 lineage) — **100×**, tăng **tuyến tính**. Nguồn: `VEDATA-MOSAIC-LOAD-FEE-REPORT.md`, 18 mẫu, `0,8948–0,9003 tADA`. Quy mô đích của đội cây là **100k**.
2. **Thứ đắt tiền đó mua được ÍT HƠN quảng cáo.** Lý do duy nhất chịu giá 100× là "INV-E7 độc lập khoá" — validator ép on-chain, không tin khoá publisher. Nhưng validator **chỉ kiểm ĐỘ DÀI `mmr_root'`**, nên chuỗi chống **tụt-lùi-seq**, **không** chống **rewrite**. Cái mua được hẹp hơn cái trả tiền.
3. **Và nó chưa đóng ở đầu vào.** `strata/anchor.ak` gác SPEND, **không gác CREATE**, chưa có thread-NFT one-shot (mục 6 §5, `Core#50` MB-5/P0b). Người lạ đặt sẵn luồng neo cho một cây **trước khi** cây đó được neo lần đầu — lúc đó chưa có luồng thật nào để đối chiếu. Trả giá 100× cho một bất biến còn hở ở cửa vào là trả trước cho thứ chưa giao.

**Hệ quả lên câu 1 — đúng như §7.4 dự đoán, và theo chiều hạ mức.** Câu 1 hỏi đường production là (A) `impl MosaicBackend` Rust hay (B) Strata → Mosaic intake. `MosaicBackend` là seam của **đường Mosaic-A**. Đội cây không đi đường đó nữa ⇒ (A) mất người tiêu thụ ở quy mô, và **viết `MosaicBackend` trước là công cốc** đúng như `Strata-API.md:421` gợi ý. Câu 1 **không còn chặn** đường cây; nó chỉ còn chặn nhánh hồ sơ giá-trị-cao lẻ. Vẫn cần anh Đức chốt, nhưng nó rời khỏi đường găng.

⚠️ **Điều kiện đi kèm, ghi ra để không ai đọc chốt này thành "Mosaic-A bỏ đi":** Mosaic-A vẫn là đường **duy nhất** cho INV-E7 độc lập khoá. Trước khi nhánh giá-trị-cao dùng nó ở quy mô, phải đóng **cả hai** lỗ mục 2 và 3 ở trên — nếu không thì cái giá 100× mua về một bất biến chưa chạy đủ.

### 9.4 Ba PR còn mở, và mỗi cái chặn bởi thứ gì

| PR | Chặn bởi | Ghi chú |
|---|---|---|
| **#42** | anh Đức chưa đẩy 3 sửa đã hứa 13/08 | `resolve()` thay `on_chain_seq()` · nhánh `None` fail cứng · chặn `AnchorPriority` thưa ở constructor. Không tự đẩy thay vì anh nói rõ *"anh cập nhật vào nhánh này, em đợi bản cập nhật rồi hẵng bấm merge"* |
| **#40** | phải gộp 3 mục spec | `TimestampTooFarFuture` (lỗi cửa **thứ 7**, ngoài 6 biến thể #40 đang chốt thành danh sách đóng) · route `_canonical` (route **thứ 9**, #40 liệt 8) — **cả hai nay đã xác định vì #48 đã land**; mục thứ 3 là `SeqGap` của #42, **chờ #42** |
| **#31** | secret `ANCHOR_DEPLOY_KEY` | Workflow tự báo lỗi đúng nguyên nhân và tự in 3 bước thêm khoá. **Cổng kêu to là hành vi đúng**, không phải CI hỏng — nhưng nó cần quyền admin repo mà `lrybi` không có |

Thứ tự land đã tuân: **#31 cuối** (vì secret chưa có), **#40 cuối nhóm spec** (vì phải phản ánh #42 và #48).

Một chỗ **lệch có chủ ý** so với đề nghị ở body #48: anh Đức đề nghị land #48 **sau #42**. Đã land **trước**, vì #42 đang đợi chính anh, mà ràng buộc thật chỉ là *#40 land cuối* — ràng buộc đó không đổi khi #48 vào trước. Đã ghi lý do lên #48 kèm lời hoàn lại nếu anh thấy có lý do khác.

### 9.5 Lưu vết phương pháp — bổ sung cho §8

- **Hai lớp gác trùng miền thì chỉ có MỘT lớp được canh.** Đọc mã thấy hai lớp; chạy mutation mới biết bộ kiểm chỉ chạm được một. Khi viết phòng vệ nhiều lớp, phải hỏi ngay: *có đầu vào nào phân biệt được lớp này với lớp kia không* — không có thì lớp thừa sẽ bị gộp mất trong lần dọn mã tới, và CI sẽ đồng ý với người gộp.
- **Bài kiểm phủ định cần bài kiểm khẳng định đứng cạnh.** `check_ts_at(x, None) -> Err` một mình không phân biệt được "gác đúng" với "từ chối tất". Cặp đôi mới khoá được hành vi.
- **Hai PR vá cùng một chỗ là chuyện bình thường khi hàng chờ dài — nhưng chỉ lộ khi ĐỌC diff, không lộ khi đọc tiêu đề.** #48 tiêu đề "6 lỗ cổng daemon", #50 tiêu đề "gác trùng key"; nghe như hai việc. Trước khi land một hàng chờ, so **danh sách file** của mọi PR mở trước, không so tiêu đề.
- **Đổi thông điệp lỗi là đổi hợp đồng của test.** Hoà giải hai bản vá xong thì phải soi lại mọi `assert!(err.contains(...))` — chúng so vào chuỗi, và chuỗi vừa đổi.

---

### 9.6 CHỐT câu 1 — hướng **(B)**, dạng **B1′**

Chốt 2026-08-14 (phía VeData). Đường submit production là **(B) Strata đẩy lô sang Mosaic**, không phải (A) `impl MosaicBackend` bằng Rust.

**Căn cứ đo được, không phải sở thích kiến trúc:**

| | (A) `impl MosaicBackend` | (B) đẩy lô sang Mosaic |
|---|---|---|
| Hôm nay tồn tại | `impl MosaicBackend for` khớp **đúng 1** kết quả — `MockMosaic` (`tests/anchor_sink.rs:111`). Daemon cắm `DisabledSink` ⇒ neo thật **501** (`node/src/bin/strata_node.rs:9-10`) | Mosaic có `tx-builder/` chạy thật (`meshAnchorChain.ts`, `emergencySubmitter.ts`) + intake M2 |
| Batch | ❌ `submit_anchor(datum: &PlutusData)` nhận **một** datum ⇒ 1 tx / 1 lineage | ✅ có `BatchCoordinator` |
| Khoá ví | Strata phải cầm khoá ký — thêm một chỗ giữ secret | Mosaic giữ — đúng ranh giới *"Mosaic giữ tx"* |
| Chain-index | Strata phải tự có đường trả **mọi UTxO ứng viên kèm asset** (`anchor_sink.rs:479-486`) | dùng lại hạ tầng Mosaic |

Cộng thêm: sau chốt 2a, (A) **mất người tiêu thụ ở quy mô** — nó là seam của đường Mosaic-A mà đội cây không đi nữa.

**Dạng B1′ — Mosaic quyết lô, Strata kiểm + encode, Mosaic dựng tx.**

> ⚠️ Bản đầu của mục này chốt **B1** ("Strata gửi payload đã encode, Mosaic chỉ submit"). **Đã sửa 2026-08-14 trong cùng ngày** — B1 đặt việc **quyết thành phần lô** vào Strata, tức bỏ không `BatchCoordinator` của Mosaic. Giữ nguyên vết để thấy chỗ trượt: khi chia việc theo *"ai giữ byte-format"* thì rất dễ kéo luôn *"ai quyết lô"* đi theo, mà hai thứ đó **không cùng một câu hỏi**.

| Bước | Ai làm | Vì sao |
|---|---|---|
| Quyết **khi nào** bắn lô + lô gồm **anchor nào** | **Mosaic** `BatchCoordinator` (`ts/src/coordinator.ts`) | đây là việc **neo**; Mosaic đã có hàng đợi ưu tiên + ngưỡng depth/age + 4 strategy §7.4 |
| Kiểm **INV-E7 chống rollback** từng anchor, rồi encode label 1234 | **Strata** `publish_batch` (`src/anchor_sink.rs:405`) + `encode_records` (`src/settlement.rs:159`) | `resolve()` là **chain logic**; encoder giữ **một** bản, đã ghim fixture |
| Dựng tx + ký + submit + trả `txid` | **Mosaic** | *"Mosaic giữ tx"* — và `submit.ts` **rời khỏi Strata** (§9.7) |

```
Mosaic BatchCoordinator  ──(N anchor đã chọn)──▶  Strata: publish_batch → kiểm INV-E7 + encode_records
                         ◀──({payload_cbor, ref_ids})──
                         ──▶  dựng tx + ký + submit  ──▶  { txid, policy_id }  ──▶  Strata ghi receipt
```

**Chỗ suýt bỏ sót, và nó là chỗ mất bất biến:** `publish_batch` chạy `resolve()` cho **từng** anchor để chặn rollback. Nếu Mosaic tự gộp lô rồi submit thẳng **không đi qua `publish_batch`**, thì **mất luôn gác INV-E7** — lô vẫn lên chuỗi, vẫn trông đúng, chỉ là không còn ai chặn một anchor tụt-lùi-seq. Nên lô phải đi qua Strata **một nhịp** để kiểm, dù Mosaic là bên quyết thành phần lô.

**Đánh đổi phải nói thẳng:** B1′ có một round-trip Mosaic → Strata → Mosaic, tức **Strata phải sống lúc Mosaic bắn lô**. Đổi lại: một encoder duy nhất, INV-E7 còn nguyên, và không module nào làm việc của module kia.

Vì sao **không** chọn B2 (Mosaic tự encode label 1234 bằng TS): B2 sinh **bản encoder thứ hai** phải giữ parity byte với bản Rust — đúng lớp vấn đề mà `#47`/`#49` sinh ra để quản, và là lớp lỗi đã cắn ở `stamp_id` 32-vs-36 byte. **Và** nó vẫn phải gọi Strata riêng một nhịp để kiểm INV-E7, nên nó **không** tiết kiệm được round-trip — chỉ thêm một encoder.

⚠️ `payload_cbor` **không đục hoàn toàn**: beacon cần `ref_ids` vì `unit = policyId ‖ ref_id`. Nói rõ để bên hiện thực không thiết kế cửa theo giả định "Mosaic không cần biết gì về nội dung".

**Một chỗ Mosaic có sẵn nhưng KHÔNG dùng được:** `tx-builder/src/strataAnchorPlan.ts` + `strataAnchorDatum.ts` đã mirror `strata_anchor.ak` — nhưng đó là đường **Mosaic-A CIP-68**, tức đúng đường 89,6 tADA mà 2a vừa loại. Phần Mosaic đã có sẵn cho Strata lại **không phải** phần đội cây cần. Mosaic **chưa có** encoder label 1234 (nó chỉ biết `ANCHOR_METADATA_LABEL = 7368` ở `ts/src/params.ts:118`, và label 0 legacy) — dưới B1′ nó **không cần có**.

#### 9.6.1 Hai cây Merkle, hai trục — không phải một việc làm hai lần

Câu hỏi đặt ra khi rà B1: *Mosaic mới là nơi xây cây Merkle, sao lại nói nó không cần hiểu merkle root?* Câu đó đúng, và bản đầu của mục này phát biểu **quá rộng**. Đo lại thì có **ba** cây, ở **hai trục khác nhau**:

| Cây | Leaf | Cam kết gì | Thuộc ai |
|---|---|---|---|
| MMR lineage — `src/chain.rs` | `version_hash` | lịch sử version của **một** hồ sơ (INV-E3/E8) | **là chính chuỗi Strata** — gỡ đi thì không còn Strata |
| Checkpoint sub-MMR — `src/batch.rs` (S3) | entry | gộp N entry **trong cùng một lineage** theo epoch | chain logic — Strata |
| Cây lô — `Core/mosaic/merkle-builder/` (M4) | `anchor_request` | gộp **nhiều đối tượng khác nhau** vào một tx | **việc neo — Mosaic** |

Hai trục: Strata gộp **theo thời gian trong một hồ sơ**; Mosaic gộp **nhiều hồ sơ vào một tx**. Không trùng việc.

Mosaic **có** thư viện Merkle đầy đủ — MMR §9.1, SMT compact-pruned §9.3, Lazy-MMR + WAL §9.2, proof 33-byte + gói CBOR §12.4 (`merkle-builder/src/`). Phát biểu đúng phải hẹp lại: *trong luồng B1′, Mosaic không phải **tính lại cây MMR lineage của Strata*** — vì cây đó không phải sản phẩm của việc neo, nó là **cấu trúc dữ liệu của chính Strata**.

Ghi ra vì cái sai này có hình dạng dễ lặp: **từ "module X không cần làm việc này trong luồng này" nhảy sang "module X không làm việc này"** — một câu về *luồng* bị đọc thành một câu về *năng lực*, và lần đọc sau sẽ dùng nó để kết luận sai về ranh giới module.

### 9.7 Luật `Strata#1` đã bị vượt trên thực tế — ghi ra thay vì lờ đi

Luật ranh giới từ `Strata#1`: *"Strata giữ logic chain; **Mosaic giữ tx; KHÔNG dựng tx neo trong Strata**"*.

Nhưng `anchor-io/submitter/submit.ts` **đang dựng tx ngay trong repo Strata** — build + sign + submit qua Lucid, và đó là **đường đã chạy thật**, đã nghiệm thu on-chain Preview. Hai khả năng: hoặc luật chỉ nhắm tx **script** CIP-68 chứ không nhắm metadata label 1234, hoặc luật đã bị vượt mà không ai ghi. **Xin anh Đức khoanh phạm vi.**

**ĐÍCH đã chốt phía VeData: gỡ hẳn việc dựng tx neo ra khỏi Strata.** Không phải "hợp thức hoá chỗ đã vượt" — luật `Strata#1` giữ nguyên hiệu lực, và `submit.ts` là thứ phải đi.

**Nhưng không xoá ngay**, vì nó là bằng chứng đường này chạy được thật — thứ đắt nhất trong cả mối nối. Điều kiện chuyển giao viết thành luật:

> `submit.ts` là **đường sống TẠM**. Nó chỉ được xoá sau khi bản Mosaic **(a)** qua đúng bộ fixture chung `apis/settlement-metadata.json`, **và (b)** submit được một tx thật trên Preview. Đủ cả hai ⇒ **xoá**, không giữ song song.

Hai vế đó không thừa. Vế (a) chặn port sai byte; vế (b) chặn port đúng byte mà không bao giờ chạy — *một đường chỉ dùng khi hỏng mà không chạy thật thì chỉ TRÔNG như tồn tại*, đúng vụ fallback L1 từng là stub. Và mệnh đề "đủ cả hai ⇒ **xoá**" cũng không thừa: giữ hai đường submit song song là có hai chỗ cầm khoá ví, tức nhân đôi đúng thứ đang muốn gom về một nhà.

### 9.8 Đích là **ví của chính người dùng, ráp qua PhoenixKey** — và điều đó phá giả định của beacon

Hướng dài hạn (Thịnh chốt 2026-08-14): ví submit **thuộc về chính người dùng**, không phải ví nền tảng. Đường ráp là **PhoenixKey** — một nền tảng ví/tài khoản riêng, **anh Tuân phụ trách**. Thịnh nói rõ **"cái đó là sau này"**, nên nó **KHÔNG nằm trong phạm vi đợt này**; ghi ở đây vì nó đổi cách đọc hai thứ đang thiết kế hôm nay, và thiết kế mà không biết đích thì sẽ phải làm lại.

**Hạ tầng phía Mosaic đã dựng sẵn đường này — khi PhoenixKey tới là RÁP VÀO, không phải làm mới:**

| Có sẵn | Ở đâu | Làm gì |
|---|---|---|
| Phân giải DID qua PhoenixKey | `Core/mosaic/ts/src/intake/phoenixkey.ts` | resolve `owner_did` → DID document, **fail-closed** |
| Ràng chữ ký owner theo **khoá** | `Core/mosaic/ts/src/intake/signature.ts` | CIP-30 owner-UT binding, domain-tag `MOSAIC-ANCHOR-REQ-v1`, verifier **injectable**, không có default không-an-toàn |

`signature.ts` đã tách sẵn `OwnerSignatureVerifier` thành interface, và tự khai rằng bản MVP (`Ed25519BindingVerifier`) sẽ được **thay bằng verifier Mesh.js / COSE_Sign1 (CIP-30) sau CÙNG một interface**. Tức chỗ cắm PhoenixKey đã có hình dạng, chỉ chưa có bên cắm vào. Cùng nguồn canonical DID mà OriLife-Core đang dùng (`author_did = blake2b_256` của DID canonical).

**Hệ quả một — nó đảo lại một nhận định của chính báo cáo này.** Ở vòng đo đầu, việc intake bắt buộc chữ ký CIP-30 của **owner** bị xếp là **điểm vênh** của (B): Strata-với-tư-cách-dịch-vụ không cầm khoá owner nên không ký được. Nhận định đó đúng với **hôm nay**, nhưng dưới mô hình ví-người-dùng thì chữ ký owner **sẽ có** — và cửa intake hiện tại lại đúng là hình dạng dài hạn. ⇒ Cửa service-to-service của B1′ là **bản CHUYỂN TIẾP**, không phải bản cuối.

**Việc phải làm ngay, dù PhoenixKey là chuyện sau này:** ghi câu đó vào **docstring của cửa `strata-anchor-batch`** lúc hiện thực, kèm điều kiện thu hồi (*"khi owner-signature qua PhoenixKey có hiệu lực, cửa này gộp về cửa intake chính"*). Không ghi thì nó sẽ sống lâu như mọi bản trung gian được trình bày như bản cuối — đúng bài học `threadPin` ở `VEDATA-MOSAIC-M15-REPORT.md §11.4`.

**Hệ quả hai — beacon (issue `#14`) mất giả định nền.** `submit.ts:101-103` dựng policy native `sig(pkh)`, nên

```
policyId = mintingPolicyToId(scriptFromNative({type:"sig", keyHash: pkh}))
unit     = policyId ‖ ref_id            (submit.ts:195-201)
```

`policyId` là **hàm của khoá ví publisher**. Beacon được thiết kế cho **một** publisher cố định. Ví của chính người dùng ⇒ **policyId khác nhau theo từng người ghi** ⇒ `resolve()` tra một `policyId` cố định là **sai hẳn**: nó sẽ không thấy beacon của bất kỳ lineage nào do người khác ghi, và tính chất chống flood-eviction của `#14` **im lặng ngừng hoạt động**. Không lỗi nào bật ra.

Cách duy nhất giữ được tính chất đó là **ghim publisher theo từng lineage** — tức reader phải biết trước "lineage này của pkh nào" rồi mới suy ra policyId để tra. Đó **đúng cùng hình dạng bài toán** `threadPin` / thread-NFT one-shot bên Mosaic (`Core#50` MB-5, `VEDATA-MOSAIC-M15-REPORT.md §11`): một định danh phải được **khai trước**, không được **đoán từ dữ liệu on-chain**.

**Trước mắt** (giai đoạn chuyển tiếp): giữ ví chung hiện tại ⇒ `policyId` không đổi ⇒ **không phải di trú beacon**. Đánh đổi phải nói thẳng: nó giữ nguyên hình dạng "một secret cho cả hệ" mà `VeDataIO/Core#87` đang chất vấn (`wallet.rs:3` tự khai *"secret DUY NHẤT của hệ thống"*). Đây là **nợ có điều kiện mở**, không phải một chốt đóng.

⚠️ **Và đây là chỗ dễ mất tiền nhất nếu làm sai thứ tự:** ngày nào đổi ví submit — dù là sang ví riêng của Mosaic hay sang ví người dùng — thì **phải quyết di trú beacon trong CÙNG lượt**. Đổi ví trước rồi tính beacon sau nghĩa là có một quãng thời gian mà beacon mới nằm dưới policy mới, beacon cũ nằm dưới policy cũ, và không đường đọc nào thấy đủ cả hai.

