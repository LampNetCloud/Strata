# Strata — Báo cáo mối nối neo on-chain (OriLife-Core ↔ Strata ↔ Mosaic)

> **Repo:** `LampNetCloud/Strata` · **Mở:** 2026-08-13
> **Phạm vi:** đường neo đầu-cuối từ tầng ứng dụng xuống L1 Cardano — ranh giới module, hợp đồng byte đã ghim, và các mảnh còn thiếu.
> **Vì sao gộp một file:** ba việc dưới đây (review PR #42, trả YC-6 cho OriLife, đo khoảng trống `MosaicBackend`) đều nằm trên **cùng một mối nối**; tách file theo từng PR/issue sẽ làm mất chính bức tranh đó.
>
> 🔴 **§12 (2026-08-20) — BẢN CÓ HIỆU LỰC: gom lô LIÊN HỘ.** Phán quyết `VeDataIO/Specs#32` (2026-08-19) **lật ba chốt** của §11/§12 bên Core: gom **liên hộ** thay gom theo hộ · **bỏ** cò 100 lượt · **bỏ** van đáy 90 ngày (nhịp tham số hoá, cận trên ≤ 24 h). Bốn việc rơi vào kho NÀY: `fval_hash` nhận **salt** (§12.3) · route `_dirty` đổi hình dạng (§12.2) · label **674** (§12.4) · điều kiện thu hồi cửa lên **spec** (§12.5). §11 giữ nguyên làm vết; chỗ nào chỏi nhau thì **§12 thắng**.
>
> 🆕 **§11 (2026-08-17) — chốt GOM LÔ THEO HỘ; ba điều thuộc về kho NÀY.** Việc P0 phía Strata là route đọc `_dirty` (§11.3 — **không cần state mới**, `store.rs` đã đủ dữ liệu). Kèm hai nợ vừa lộ: `SinkConfig` chỉ biết **một publisher toàn cục** nên ví-mỗi-hộ làm vỡ (§11.1), và `AnchorPriority` có ba nhánh **không phân biệt được ở đâu** trên đường Settlement (§11.2). Phần Mosaic + số đo chi phí: `VeDataIO/Core: docs/VEDATA-MOSAIC-STRATA-SEAM-REPORT.md` §12 (`VeDataIO/Core#99`).

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
| 2a | **Đội cây OriLife đi đường nào** | ✅ **TRƯỚC MẮT — batch-root/Settlement.** Mosaic-A giữ cho hồ sơ giá-trị-cao lẻ. Xem §9.3 |
| 2b | Chọn hướng (A) `impl MosaicBackend` Rust vs (B) Strata → Mosaic intake | ✅ **TRƯỚC MẮT — (B), dạng B1′**: Mosaic quyết lô · Strata kiểm INV-E7 + encode · Mosaic dựng tx + submit. Xem §9.6 |
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
| 11 | 🆕 **Route `GET /v1/strata/_dirty`** — cửa vào của coordinator gom lô theo hộ | **P0, việc của kho này**; không cần state mới — xem §11.3 |
| 12 | ~~`SinkConfig.publisher_address` là MỘT publisher toàn cục — ví-mỗi-hộ làm vỡ~~ | ⛔ **KHÔNG còn là nợ** (2026-08-17): nền tảng ký ⇒ publisher = nền tảng ⇒ `SinkConfig` giữ nguyên. Lỗ vẫn còn trong mã, chỉ là thiết kế không đụng — xem §11.1 |
| 13 | 🆕 `AnchorPriority` — `Immediate`/`Milestone`/`BatchDaily` **không phân biệt được ở đâu** trên đường Settlement | doc-comment hoặc gộp khi `#40` chốt danh sách đóng — xem §11.2 |

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

## 9. Đợt 2026-08-14 — phiên critical path: dọn hàng chờ PR + trả câu 2

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

### 9.3 Câu 2 của MB-6 — trước mắt đội cây OriLife đi **batch-root/Settlement**

Trước mắt: Mosaic-A **không** dùng cho đội cây; giữ cho **hồ sơ giá-trị-cao lẻ**.

Căn cứ, theo thứ tự sức nặng:

1. **Chi phí, đo thật.** 100 cây: `~0,896 tADA` (batch-root/Settlement, 1 tx / N record) vs `~89,6 tADA` (Mosaic-A, 1 tx / 1 lineage) — **100×**, tăng **tuyến tính**. Nguồn: `VEDATA-MOSAIC-LOAD-FEE-REPORT.md`, 18 mẫu, `0,8948–0,9003 tADA`. Quy mô đích của đội cây là **100k**.
2. **Thứ đắt tiền đó mua được ÍT HƠN quảng cáo.** Lý do duy nhất chịu giá 100× là "INV-E7 độc lập khoá" — validator ép on-chain, không tin khoá publisher. Nhưng validator **chỉ kiểm ĐỘ DÀI `mmr_root'`**, nên chuỗi chống **tụt-lùi-seq**, **không** chống **rewrite**. Cái mua được hẹp hơn cái trả tiền.
3. **Và nó chưa đóng ở đầu vào.** `strata/anchor.ak` gác SPEND, **không gác CREATE**, chưa có thread-NFT one-shot (mục 6 §5, `Core#50` MB-5/P0b). Người lạ đặt sẵn luồng neo cho một cây **trước khi** cây đó được neo lần đầu — lúc đó chưa có luồng thật nào để đối chiếu. Trả giá 100× cho một bất biến còn hở ở cửa vào là trả trước cho thứ chưa giao.

**Hệ quả lên câu 1 — đúng như §7.4 dự đoán, và theo chiều hạ mức.** Câu 1 hỏi đường production là (A) `impl MosaicBackend` Rust hay (B) Strata → Mosaic intake. `MosaicBackend` là seam của **đường Mosaic-A**. Đội cây không đi đường đó nữa ⇒ (A) mất người tiêu thụ ở quy mô, và **viết `MosaicBackend` trước là công cốc** đúng như `Strata-API.md:421` gợi ý. Câu 1 **không còn chặn** đường cây; nó chỉ còn chặn nhánh hồ sơ giá-trị-cao lẻ. Vẫn cần anh Đức chốt, nhưng nó rời khỏi đường găng.

⚠️ **Điều kiện đi kèm, ghi ra để không ai đọc điều này thành "Mosaic-A bỏ đi":** Mosaic-A vẫn là đường **duy nhất** cho INV-E7 độc lập khoá. Trước khi nhánh giá-trị-cao dùng nó ở quy mô, phải đóng **cả hai** lỗ mục 2 và 3 ở trên — nếu không thì cái giá 100× mua về một bất biến chưa chạy đủ.

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

### 9.6 Câu 1 — trước mắt đi hướng **(B)**, dạng **B1′**

Trước mắt, đường submit production là **(B) Strata đẩy lô sang Mosaic**, không phải (A) `impl MosaicBackend` bằng Rust.

**Căn cứ đo được, không phải sở thích kiến trúc:**

| | (A) `impl MosaicBackend` | (B) đẩy lô sang Mosaic |
|---|---|---|
| Hôm nay tồn tại | `impl MosaicBackend for` khớp **đúng 1** kết quả — `MockMosaic` (`tests/anchor_sink.rs:111`). Daemon cắm `DisabledSink` ⇒ neo thật **501** (`node/src/bin/strata_node.rs:9-10`) | Mosaic có `tx-builder/` chạy thật (`meshAnchorChain.ts`, `emergencySubmitter.ts`) + intake M2 |
| Batch | ❌ `submit_anchor(datum: &PlutusData)` nhận **một** datum ⇒ 1 tx / 1 lineage | ✅ có `BatchCoordinator` |
| Khoá ví | Strata phải cầm khoá ký — thêm một chỗ giữ secret | Mosaic giữ — đúng ranh giới *"Mosaic giữ tx"* |
| Chain-index | Strata phải tự có đường trả **mọi UTxO ứng viên kèm asset** (`anchor_sink.rs:479-486`) | dùng lại hạ tầng Mosaic |

Cộng thêm: khi đội cây đi Settlement (2a), (A) **mất người tiêu thụ ở quy mô** — nó là seam của đường Mosaic-A mà đội cây không đi nữa.

**Dạng B1′ — Mosaic quyết lô, Strata kiểm + encode, Mosaic dựng tx.**

> ⚠️ Bản đầu của mục này đi theo **B1** ("Strata gửi payload đã encode, Mosaic chỉ submit"). **Đã sửa 2026-08-14 trong cùng ngày** — B1 đặt việc **quyết thành phần lô** vào Strata, tức bỏ không `BatchCoordinator` của Mosaic. Giữ nguyên vết để thấy chỗ trượt: khi chia việc theo *"ai giữ byte-format"* thì rất dễ kéo luôn *"ai quyết lô"* đi theo, mà hai thứ đó **không cùng một câu hỏi**.

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

**Luật này đúng và áp dụng đầy đủ — không có gì phải khoanh lại.** Nó nhắm **mọi** tx neo, không phân biệt tx script CIP-68 hay tx metadata label 1234: việc dựng tx neo thuộc Mosaic.

Nhưng `anchor-io/submitter/submit.ts` **đang dựng tx ngay trong repo Strata** — build + sign + submit qua Lucid. Tức đây là **một chỗ đã vượt luật**, không phải một vùng xám cần diễn giải. Điều đáng giữ là: **đó là đường đã chạy thật**, đã nghiệm thu on-chain Preview.

**ĐÍCH: gỡ hẳn việc dựng tx neo ra khỏi Strata.** Không phải "hợp thức hoá chỗ đã vượt" — luật `Strata#1` giữ nguyên hiệu lực, và `submit.ts` là thứ phải đi.

**Nhưng không xoá ngay**, vì nó là bằng chứng đường này chạy được thật — thứ đắt nhất trong cả mối nối. Điều kiện chuyển giao viết thành luật:

> `submit.ts` là **đường sống TẠM**. Nó chỉ được xoá sau khi bản Mosaic **(a)** qua đúng bộ fixture chung `apis/settlement-metadata.json`, **và (b)** submit được một tx thật trên Preview. Đủ cả hai ⇒ **xoá**, không giữ song song.

Hai vế đó không thừa. Vế (a) chặn port sai byte; vế (b) chặn port đúng byte mà không bao giờ chạy — *một đường chỉ dùng khi hỏng mà không chạy thật thì chỉ TRÔNG như tồn tại*, đúng vụ fallback L1 từng là stub. Và mệnh đề "đủ cả hai ⇒ **xoá**" cũng không thừa: giữ hai đường submit song song là có hai chỗ cầm khoá ví, tức nhân đôi đúng thứ đang muốn gom về một nhà.

### 9.8 Đích là **ví của chính người dùng, ráp qua PhoenixKey** — và điều đó phá giả định của beacon

Hướng dài hạn: ví submit **thuộc về chính người dùng**, không phải ví nền tảng. Đường ráp là **PhoenixKey** — một nền tảng ví/tài khoản riêng, **anh Tuân phụ trách**. Đó là **chuyện sau này**, **KHÔNG nằm trong phạm vi đợt này**; ghi ở đây vì nó đổi cách đọc hai thứ đang thiết kế hôm nay, và thiết kế mà không biết đích thì sẽ phải làm lại.

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

**Trước mắt** (giai đoạn chuyển tiếp): giữ ví chung hiện tại ⇒ `policyId` không đổi ⇒ **không phải di trú beacon**. Đánh đổi phải nói thẳng: nó giữ nguyên hình dạng "một secret cho cả hệ" mà `VeDataIO/Core#87` đang chất vấn (`wallet.rs:3` tự khai *"secret DUY NHẤT của hệ thống"*). Đây là **nợ có điều kiện mở**, không phải một kết luận đóng.

⚠️ **Và đây là chỗ dễ mất tiền nhất nếu làm sai thứ tự:** ngày nào đổi ví submit — dù là sang ví riêng của Mosaic hay sang ví người dùng — thì **phải quyết di trú beacon trong CÙNG lượt**. Đổi ví trước rồi tính beacon sau nghĩa là có một quãng thời gian mà beacon mới nằm dưới policy mới, beacon cũ nằm dưới policy cũ, và không đường đọc nào thấy đủ cả hai.

### 9.9 NFT / CIP-68 — ba thứ khác nhau đang bị gọi chung, và mức cần thiết khác hẳn nhau

Câu hỏi phát sinh khi khoanh phạm vi: *chọn Settlement rồi thì NFT/CIP-68 còn cần không?* Trả lời được thì phải tách ba thứ ra trước — chúng hay bị gộp làm một.

| # | Thứ | Làm gì | Trạng thái thật |
|---|---|---|---|
| 1 | **Datum CIP-68** (`strata_anchor.ak`) | anchor là **UTxO sống**, spend-recreate `seq+1`, validator ép đơn điệu ⇒ INV-E7 **độc lập khoá** | có, đã chạy Preprod |
| 2 | **Thread-NFT one-shot** | thứ làm cho lời hứa của (1) **thật sự đứng** — chặn người lạ CREATE UTxO giả cùng `ref_id` | ❌ **CHƯA CÓ** |
| 3 | **Beacon NFT** (Settlement, `#14`) | cho `resolve()` một **asset để tra** thay vì quét metadata ⇒ miễn nhiễm flood-eviction | có, **tuỳ chọn** (`beacon?: boolean`) |

Mục (2) không phải suy luận — **chính header validator tự khai** (`VeDataIO/Core: mosaic/aiken/validators/strata_anchor.ak:22-35`):

> *"RANH GIỚI TIN CẬY (đọc kỹ trước khi tích hợp — **chưa có thread-NFT**): Validator này **KHÔNG** bind một thread identity token duy nhất (không one-shot mint kiểu RT-100/UT-222 của `mosaic_genesis`)… cơ chế cụ thể **CHƯA** quyết."*

`mosaic_genesis.ak` **có** one-shot RT-100/UT-222 — nhưng đó là thread của **Mosaic**, không phải thread anchor của Strata. Đừng đọc "Mosaic đã có one-shot" thành "anchor Strata đã được gác".

**Mức cần thiết, theo từng nhánh:**

- **Đường găng (đội cây → Settlement): CIP-68 không cần chút nào.** Label 1234 là metadata thuần — không UTxO, không datum, không NFT. Đó **chính là** lý do nó rẻ 100×.
- **Nhánh hồ sơ giá-trị-cao (Mosaic-A): CIP-68 là toàn bộ lý do nhánh đó tồn tại** — nhưng hôm nay nó **thiếu chân**. Không có (2) thì kẻ lạ tốn một min-ADA cấy UTxO `seq = 2^63` mang `ref_id` nạn nhân, operator thật kẹt vĩnh viễn ở `RollbackAttempt`. Tức đang trả giá 100× cho một bất biến **chưa chạy đủ**. Che tạm hôm nay chỉ có `threadPin` off-chain trong `.env` — mà `VEDATA-MOSAIC-M15-REPORT.md §11.4` tự ghi đó là **bản trung gian**.

**Vì sao beacon tồn tại — và nó không phải đồ trang trí.** CIP-68 cho ta **một UTxO sống** để hỏi thẳng *"đỉnh lineage này ở đâu"*. Settlement chỉ để lại **vết lịch sử trong metadata**, muốn tìm đỉnh thì phải **quét** — mà quét thì làm mù được bằng tx rác rẻ tiền (đúng `#14`). Beacon chính là **bản thay thế rẻ tiền của CIP-68** cho đường Settlement: mint một asset `policyId ‖ ref_id` để có thứ **tra chỉ mục asset** thay vì quét. ⇒ Chọn Settlement thì beacon là thứ giữ cho đường **ĐỌC** còn dùng được ở quy mô, không phải tuỳ chọn cho vui. Và nó nối thẳng vào vướng mắc `policyId = f(pkh)` ở §9.8.

**Phạm vi trước mắt: làm theo đường găng — đội cây → Settlement. NFT/CIP-68 để vào một hoàn cảnh thích hợp khác.**

Đây là **để dành cho hoàn cảnh khác**, không phải **bỏ** — và khác biệt đó phải giữ nguyên trong mọi lần đọc lại:

- ✅ **Giữ nguyên**, không xoá: `strata_anchor.ak`, `strataAnchorPlan.ts`, `strataAnchorDatum.ts`, `MosaicAnchorSink`, `trait MosaicBackend`. Chúng đã chạy Preprod thật; xoá đi là vứt một thứ đã nghiệm thu để rồi viết lại.
- ⛔ **Không đầu tư thêm** vào nhánh này trong lúc đường găng đang chạy — cụ thể là **không** hiện thực `MosaicBackend` production (đúng §9.6), và **không** dựng thread-NFT one-shot chỉ vì nó đang thiếu.
- 🔓 **Điều kiện mở lại:** khi có ca dùng thật đòi **INV-E7 độc lập khoá** cho một hồ sơ lẻ mà giá ~0,9 tADA/lineage chấp nhận được. Lúc đó việc **đầu tiên** phải làm là mục (2) thread-NFT one-shot — không phải nối `MosaicBackend`. Nối trước (2) là dựng đường sống lên trên một bất biến còn hở ở cửa vào.

⚠️ **Câu dễ đọc nhầm nhất, ghi ra để chặn:** *"chọn Settlement rồi thì bỏ hết NFT"* — **sai**. Bỏ được (1) và (2); **(3) beacon thì không**, vì nó phục vụ đường Settlement chứ không phục vụ CIP-68.


---

## 10. Đợt 2026-08-15 — mối nối B1′ ĐÃ CHẠY THẬT, và `submit.ts` đã rời khỏi kho này

Phần việc phía Mosaic ghi ở `VeDataIO/Core: docs/VEDATA-MOSAIC-STRATA-SEAM-REPORT.md` §8–§10. Mục này chỉ ghi **những gì đổi trong kho Strata** và **vì sao**.

### 10.1 Đã thêm

| Thứ | Ở đâu | Vai |
|---|---|---|
| `MosaicDoorSubmitter` | `anchor-io/src/mosaic_door.rs` | impl `Submitter` — đẩy lô sang cửa Mosaic; **không** dựng tx |
| `AnchorSink::publish_many` | `src/anchor_sink.rs` | đường LÔ, mặc định **fail-closed** |
| `SettlementSink::resolve_many` | `src/settlement.rs` | một lượt quét cho **cả** lô (xem §10.3) |
| Route `POST /v1/strata/_anchor_batch` | `node/src/routes.rs` | cửa vào của `BatchCoordinator` phía Mosaic |
| `sink_config` | `node/src/sink_config.rs` | cắm sink **thật** cho daemon từ ENV |
| `orilife_e2e` · `resolve_settlement` | `anchor-io/examples/` | lượt chạy đầu-cuối + nghiệm thu đường ĐỌC |

**Daemon trước đợt này cắm cứng `DisabledSink`** — tức mọi neo thật trả **501**, và câu *"cắm sink thật là việc của bản triển khai"* nghĩa là **không có** bản triển khai nào. Nay có, và mọi cấu hình thiếu là **lỗi khởi động**, không phải cảnh báo: một daemon lên xanh với sink nửa-cấu-hình chỉ lộ ra ở lượt neo đầu tiên, tức **sau khi** dữ liệu đã đi vào.

Một gác đáng nêu riêng: bật `STRATA_BEACON_POLICY` (đọc theo beacon) mà tắt `STRATA_ANCHOR_BEACON` (ghi không mint beacon) ⇒ `resolve` trả `None` cho **mọi** ref ⇒ gác idempotency/rollback của `publish_batch` **im lặng ngừng hoạt động**. Cấu hình đó nay bị chặn ngay lúc khởi động.

### 10.2 `submit.ts` — XOÁ, đúng luật chuyển giao §9.7

Hai điều kiện đã đủ:

- **(a)** bản Mosaic qua đúng bộ fixture chung `apis/settlement-metadata.json` — **8 dương + 6 âm + 1 bỏ-qua** (`Core: mosaic/l1/tests/settlement_fixture.rs`);
- **(b)** submit tx **thật** trên Preprod: `d9975f60…` (3 anchor), `7e78cfaa…` (10 anchor); và `resolve()` của chính kho này đọc lại **3/3**, khớp từng byte.

⇒ Đã xoá `anchor-io/submitter/` trọn thư mục, `TsSubmitter` + 3 test của nó, và job CI `submitter (tsc)`. **Không giữ song song** — hai đường submit là hai chỗ cầm khoá ví.

Điều kiện + bằng chứng (kèm txid) ghi thẳng vào doc-header `anchor-io/src/lib.rs`, không chỉ ở báo cáo: người đọc mã sau này cần biết **vì sao một đường đã chạy thật lại bị xoá**, mà họ đọc mã chứ không đọc báo cáo.

### 10.3 🪤 Đường lô suýt vô dụng — và nó chỉ lộ khi CHẠY, không lộ khi đọc mã

`publish_batch` gọi `resolve()` **cho từng** anchor. Ở chế độ legacy, mỗi `resolve()` quét **cùng một** cửa sổ tx của **cùng một** ví publisher và đọc **cùng những** metadatum ấy — chỉ khác mỗi `ref_id` đem so. Lặp N lần = `N × resolve_scan_limit` lượt gọi mạng.

Đo thật (`scan_limit = 500`): lô **3** ref chạy được; lô **10** ref **vượt 180 giây** timeout client — daemon vẫn hoàn tất và **tx vẫn lên chuỗi** (`6cc6ab6e…`), nhưng bên gọi đã bỏ cuộc và **không còn biết txid của lô mình vừa bắn**. Hỏng đúng chỗ đau: lô đã neo mà bên quyết lô coi là thất bại, rồi bắn lại.

Vá: `resolve_many` — quét một lượt, gộp cho cả tập. Lô-10 nay xong trong **31 giây** cả lượt. Khoá bằng bài kiểm **đếm số lượt quét**, đã **cố ý phá** để xác nhận nó cắn (trả về vòng lặp ⇒ đỏ, 3 ≠ 1), kèm một bài **khẳng định** đứng cạnh: gộp quét không được làm mất gác — một anchor tụt-lùi-seq vẫn phải giết cả lô.

> Đây cũng là **số đo** cho luận điểm beacon ở §9.9: chế độ beacon tra asset-index nên đã O(1) theo từng ref. Cửa sổ quét legacy vừa là điểm yếu chống flood (`#14`) vừa là **trần thông lượng** của chính đường lô.

### 10.4 `publish_many` fail-closed — vì sao không lặp `publish()` hộ

Backend nào không batch được thì **nói ra**. Một vòng lặp mặc định trông vô hại nhưng biến *1 tx / N anchor* thành *N tx* — đo thật là `~0,896` so với `~89,6` tADA cho 100 cây. Chênh 100× **mà không lỗi nào bật ra** là đúng loại hỏng phải chặn ở tầng trait, không phải ở tài liệu.

### 10.5 Trạng thái bộ kiểm

`cargo test --workspace` → **221 pass** (nền 208 − 3 test `TsSubmitter` + 16 mới), `cargo clippy --workspace --all-targets -D warnings` sạch.

### 10.6 Beacon đã chạy thật — cả nhánh mint lẫn nhánh di-chuyển

Beacon walk được **viết lại** bằng pallas ở phía Mosaic (bản cũ là `submit.ts` +
Lucid, đã xoá). `policyId` suy ra **trùng** bản cũ (`f84ac406…e8ede3`) ⇒ di trú
submitter **không làm mồ côi** beacon đã mint trước đây.

Ba vòng neo liên tiếp trên cùng 2 lineage (Preprod): vòng 1 `b1a87172…` **mint 2**
beacon; vòng 2 `4e159a44…` và vòng 3 `88f32654…` **mint 0** — tức **di chuyển**, tx
nhỏ hơn (834 vs 937 B). `resolve()` chạy chế độ beacon (tra asset-index, **không
quét**) trả đúng đỉnh mới nhất `seq=3` cho **2/2** ref.

Vòng 2–3 mới là phần nghiệm thu thật: nhánh *di chuyển* khác hẳn nhánh *mint* ở
bookkeeping value, và nó **không bao giờ chạy ở lượt đầu tiên**. Chạy một vòng nhiều
lần không thay được việc chạy nhiều vòng.

### 10.7 🪤 Bẫy thứ ba — và một bản vá SAI trước khi có bản vá đúng

Bắn hai lô liền nhau: lô sau chọn UTxO mà lô trước **đã tiêu** ⇒ ledger trả
`ConwayMempoolFailure "All inputs are spent"`. Blockfrost trả **trạng thái đã
index**, không phải mempool.

Với beacon thì hậu quả **tệ hơn và im lặng**: chưa thấy beacon vừa mint ⇒ cửa kết
luận *"chưa tồn tại"* ⇒ **mint lần hai** cùng `policyId ‖ ref_id`. Một `unit` nằm ở
hai UTxO thì beacon thôi là **con trỏ duy nhất** — đúng tính chất `resolve_via_beacon`
của kho này dựa vào.

Vá ở phía Mosaic bằng sổ **UTxO-đang-bay**. Đáng ghi lại là **bản vá đầu tiên
KHÔNG chạy**: nó dọn sổ theo `/txs/{hash}` (*"tx đã index chưa"*) trong khi sổ phục
vụ `/addresses/{addr}/utxos`. Preprod rớt y nguyên lỗi cũ — Blockfrost thấy tx
**trước khi** cập nhật danh sách UTxO.

> **Luật rút ra:** *điều kiện dọn một bộ nhớ đệm phải đọc **đúng cái nguồn** mà nó
> phục vụ.* Hai chỉ mục là hai tốc độ; hỏi cái này để quyết định cho cái kia là một
> giả định không ai bảo đảm.

---

## 11. Đợt 2026-08-17 — chốt **gom lô theo hộ**: ba điều thuộc về kho NÀY

> ⛔ **CHỐT "GOM THEO HỘ" ĐÃ BỊ LẬT (2026-08-19, `VeDataIO/Specs#32`) — đọc §12 trước.** Giữ mục này
> làm vết. Hai phần **vẫn đúng và vẫn dùng**: §11.2 (`AnchorPriority` ba nhánh không phân biệt được) và
> §11.3 (route `_dirty` — kho này đã có sẵn dữ liệu, không cần state mới).

Phiên thảo luận (không code) trên critical path đã chốt hình dạng của mảnh còn thiếu lớn nhất — **ai
quyết lô**. Phần việc phía Mosaic + toàn bộ số đo chi phí ghi ở
`VeDataIO/Core: docs/VEDATA-MOSAIC-STRATA-SEAM-REPORT.md` **§12** (PR `VeDataIO/Core#99`); phần Hydra ở
`VEDATA-MOSAIC-HYDRA-DECISION.md` **§15**. Mục này **không lặp lại** hai file đó — nó ghi đúng ba thứ nằm
trong **kho Strata**, để người mở kho này không phải đi tìm báo cáo bên kho khác mới hiểu.

**Bối cảnh một dòng, vì nó đổi mọi con số:** *"100k cây"* là **tổng toàn hệ**, mỗi hộ chỉ vài chục cây; và
ví thực hiện tx neo sẽ là **ví của chính hộ** (mọi thứ do người dùng tự ký). ⇒ Lô neo **không trộn hộ**.

### 11.1 🔺 `SinkConfig` chỉ biết **MỘT publisher toàn cục** — ví-mỗi-hộ làm vỡ

> ⛔ **CẬP NHẬT CÙNG NGÀY 2026-08-17 — mục này KHÔNG còn là nợ đang mở.** Thiết kế chốt lại: **nền tảng
> trả phí VÀ nền tảng ký**, bằng **ví của nền tảng** ⇒ publisher = nền tảng ⇒ `SinkConfig` **giữ nguyên**,
> không phải sửa gì. Chi tiết: `VeDataIO/Core: docs/VEDATA-MOSAIC-STRATA-SEAM-REPORT.md` **§12.13**.
>
> ⚠️ **Và một câu trong mục này phải đính chính vì NÓI QUÁ:** *"nền tảng **không giả được** anchor cho cây
> của hộ, dù có toàn quyền server"* — sai mức độ. Nền tảng chạy **chính daemon Strata giữ `ChainStore`**,
> nên nó vốn đã quyết `head_version_hash`/`mmr_root` báo ra. Ví-mỗi-hộ chỉ chặn được **một ca hẹp**: nền
> tảng **bịa** một anchor on-chain mang tên hộ. Nó **chưa bao giờ** chặn *không neo* hay *neo seq cũ*.
>
> ✅ **Thứ không đổi dù ai ký tx:** hộ **vẫn ký từng `version`** bằng khoá Ed25519 của họ, daemon
> `verify_strict` từ chối nếu sai (INV-E4 + `policy_hash`). *Hộ ký **NỘI DUNG** · nền tảng ký **VIỆC
> NEO*** — hai chữ ký, hai tính chất; chốt mới chỉ bỏ cái thứ hai.
>
> **Lỗ mô tả dưới đây VẪN TỒN TẠI trong mã** (`publisher_address` vẫn là hằng toàn cục) — chỉ là thiết kế
> hiện tại không đụng vào nó. Ngày nào quay lại ví-mỗi-hộ thì nó bật lại nguyên vẹn, nên giữ nguyên phần
> phân tích bên dưới làm vết.

`src/settlement.rs:339-342`:

```rust
pub struct SinkConfig {
    /// TRUST PIN v1: anchor chỉ hợp lệ nếu tx phát (input) từ ví này.
    pub publisher_address: String,       // MỘT, toàn cục
    …
    pub beacon_policy: Option<String>,   // cũng MỘT, cũng toàn cục
}
```

Dùng ở **cả hai chiều**:

| Chiều | Dòng | Làm gì |
|---|---|---|
| GHI | `:436` | tx vừa submit phải có `outcome.address == cfg.publisher_address`, lệch ⇒ từ chối |
| ĐỌC legacy | `:495`, `:504`, `:532`, `:541` | quét tx **của địa chỉ đó**, rồi lọc `input == địa chỉ đó` |
| ĐỌC beacon | `:567` | tx mới nhất chạm beacon phải do **địa chỉ đó** chi |

⇒ Một daemon hôm nay phục vụ được **đúng một** publisher. Ví-mỗi-hộ ⇒ hoặc mỗi hộ một daemon (không đời
nào), hoặc **`publisher` phải chuyển từ cấu hình toàn cục sang thuộc tính THEO LINEAGE**.

**"Publisher" không phải một vai trò do ai bổ nhiệm — nó rơi ra từ chính luật lọc trên.** Ví nào ký và
**chi** tx neo, ví đó *là* publisher theo định nghĩa. Nên "mỗi hộ một publisher" là **hệ quả** của quyết
định ví, không phải một quyết định thứ hai.

**Lấy publisher của một lineage ở đâu ra — không suy được, phải KHAI TRƯỚC:**

- `ref_id = H_dom("LN/STRATA/ref/v1", author_did ‖ nonce)` (`src/refid.rs:20`) — hàm **một chiều**;
- `author_did` là **DID đã băm** (CHỐT-5), không phải pubkey;
- khoá ký **version** (Ed25519 thuần) **≠** khoá ví Cardano (CIP-1852).

⇒ Đúng cùng hình dạng bài toán `threadPin` / thread-NFT one-shot của **`Core#50` MB-5**: *một định danh
phải được khai trước, không được đoán từ dữ liệu on-chain*. Khác biệt: trước đây nó ở mục "chuyện sau
này"; với ví-mỗi-hộ thì nó **bước lên đường găng**.

🎁 **Nhưng chỗ cắm đã có sẵn trong kho này** — `node/src/registry.rs:1-6` tự khai:

> *"Key-registry — phân giải `Did → VerifyingKey` (**CHỐT-5**: `Did` là **băm DID PhoenixKey**)… Bản trong
> repo là in-memory để chạy được đầu-cuối; bản thật cắm **PhoenixKey resolver qua chính trait này**."*

⇒ Việc không phải dựng registry mới, mà **nới trait**: `resolve(did) → VerifyingKey` thành trả thêm địa
chỉ thanh toán; rồi `SinkConfig.publisher_address` toàn cục đổi thành tra theo `author_did` của lineage.

⚠️ **Đây là nợ P1, KHÔNG chặn P0** — P0 tạm giữ ví nền tảng hiện có. Nhưng ghi ra vì đổi lại nó là một
**nâng cấp bảo mật thật**: hôm nay trụ tin cậy của **mọi** cây là **một** ví — ai cầm khoá đó ký được
anchor hợp lệ cho **bất kỳ** `ref_id` nào, kể cả cây của người lạ, và `resolve()` sẽ tin. Sau khi ráp, trụ
tin cậy của cây hộ A là **ví hộ A**; nền tảng **không giả được**, dù có toàn quyền server.

⚠️ **Giá phải trả:** hộ **mất ví ⇒ luồng neo của họ đứt** — neo bằng ví mới sẽ không được `resolve()` tin
(khác địa chỉ), và cây đó có một quá khứ nằm dưới ví cũ vĩnh viễn. Cần câu chuyện **xoay khoá publisher**;
chưa có nhà, phải có đáp án **trước khi có hộ thật**.

### 11.2 `AnchorPriority` — ba nhánh không phân biệt được ở đâu trên đường Settlement

`src/anchor_sink.rs:24-33` khai bốn cadence: `Immediate` / `Milestone` / `BatchDaily` / `NoAnchor`. Nhưng
trên đường Settlement, giá trị đó chỉ được kiểm **một** chỗ duy nhất — `== NoAnchor` thì bỏ qua
(`:113` trong `publish_many` mặc định, `:594` trong `MosaicAnchorSink::publish`).

⇒ **`Immediate`, `Milestone`, `BatchDaily` là ba giá trị cho ra hành vi y hệt nhau.** Không phải lỗi —
cadence vốn là việc của tầng **quyết lô**, mà tầng đó nằm bên Mosaic (B1′). Nhưng nó có hai hệ quả đáng
ghi:

1. **Một enum công khai hứa một sự phân biệt không tồn tại** ở backend mặc định. Người tích hợp đọc
   `Strata-API.md` sẽ tưởng đặt `BatchDaily` là có gom ngày; thực tế nó chỉ đổi một nhãn.
2. Bảng ánh xạ `anchor_priority` ↔ Strategy A/B/C/D (Mosaic-Math §7.4) là taxonomy của **đường Mosaic-A /
   CIP-68**, không phải của Settlement. Mà `§9.3` đã chốt đội cây đi Settlement. ⇒ **Trên đường găng hôm
   nay, A/B/C/D không đóng vai trò nào.**

Việc cần làm: hoặc doc-comment nói rõ ba nhánh đó không phân biệt ở backend Settlement, hoặc gộp lại khi
`#40` chốt danh sách đóng. **Chưa làm** — ghi vào §5 mục 11.

### 11.3 🎁 Route `_dirty` — kho này **đã có sẵn** dữ liệu, không cần state mới

Việc P0 phía Strata: một route đọc để coordinator bên Mosaic biết **cây nào của hộ nào đang chờ đóng dấu**.

Soi `node/src/store.rs:28-38` thì dữ liệu đã đủ:

```rust
pub struct AnchorState { pub seq: u64, pub txid: Option<String>, pub backend: Option<String> }
pub struct ChainEntry  { pub chain: StrataChain, …, pub anchored: Option<AnchorState> }
```

`chain` cho `head_seq`, `anchored.seq` cho seq đã neo ⇒ chỉ cần duyệt `ChainStore.refs` và so hai số:

```
GET /v1/strata/_dirty
→ [{ ref_id, author_did, head_seq, anchored_seq, oldest_unanchored_ts }, …]
```

Từ **một** lượt gọi đó, coordinator **tính** ra cả ba đại lượng nó cần — không cần bộ đếm ở đâu cả:

```
dirty_refs       = { ref : head_seq > anchored_seq }     ← thành phần của lô
pending_versions = Σ (head_seq − anchored_seq)           ← cò "100 lượt cập nhật/hộ"
oldest_pending   = min(oldest_unanchored_ts)             ← van thời gian
```

> **Luật rút ra: đừng ĐẾM, hãy TÍNH.** Một bộ đếm tăng dần là *trạng thái* — lệch được, đếm trùng được,
> mất khi restart được, và không có gì để đối chiếu. Một tổng suy ra từ nguồn sự thật thì **không lệch
> được**. Cùng họ bài học §10.7 (*điều kiện dọn cache phải đọc đúng nguồn nó phục vụ*).

**Hai chi tiết phải đúng ngay từ bản đầu:**

- **Đếm `version`, KHÔNG đếm `event`.** Kho này có hai cửa ghi (`node/src/routes.rs:59-60`):
  `POST /:ref/version` tiến `seq` + đổi `head_version_hash` + đẩy MMR; `POST /:ref/event` là tầng (b),
  **không đụng chuỗi version**. Anchor cam kết đúng ba thứ (`head_version_hash`/`mmr_root`/`seq`) mà **chỉ
  `version` mới làm lệch với on-chain**. Đưa `event` vào phép đếm là bắn lô sớm hơn cần ⇒ đắt hơn mà không
  neo thêm được gì.
- **`author_did` phải nằm trong response.** Nó là khoá nhóm "hộ" — `ref_id = H(author_did ‖ nonce)` là một
  chiều nên coordinator **không suy ngược được**. *(Hạn chế đã biết: một hộ có thể có nhiều `author_did` —
  vợ/chồng, người làm; ánh xạ tường minh là việc sau, chưa chặn P0.)*

⚠️ **Phụ thuộc phải nói ra:** `ChainStore` là `RwLock<HashMap<…>>` — **in-memory**, đúng một trong hai lỗ
hở còn mở của kho này. Daemon restart là mất sạch chain, không riêng trạng thái neo. Không phải việc P0 đẻ
ra, nhưng nó **chặn trần độ bền** của cả vòng gom lô, và phải vá trước khi có hộ thật. *(Điểm nhẹ nhõm:
nếu mất `anchored` mà còn chain thì `publish_batch` chạy `resolve()` đọc on-chain sẽ trả idempotent — neo
lại **không tốn tiền**.)*

### 11.4 Hydra — đã rà lại ở một hình dạng mới và BÁC, không ảnh hưởng kho này

Ghi một dòng để khỏi ai mở lại: đề xuất **một Hydra head cho mỗi hộ** (contract gom lô trong head, head
ngủ phần lớn thời gian) đã được rà bằng tài liệu upstream và **bác**. Lý do gọn: *cập nhật hồ sơ cây hôm
nay **không phải tx Cardano** — nó là ký Ed25519 → POST daemon → append MMR, tốn 0 ADA — còn tx neo thì vẫn
phải đi L1 dù có head hay không*, nên không có tx L1 nào để amortise. Đầy đủ: `VeDataIO/Core:
docs/VEDATA-MOSAIC-HYDRA-DECISION.md` §15. **Kho Strata không đổi gì vì việc này.**

### 11.5 Lưu vết phương pháp — bổ sung cho §9.5

- **Một ngưỡng đúng ở miền này có thể không bao giờ chạm tới ở miền khác.** Ngưỡng gom lô `depth ≥ 100`
  đúng cho hàng đợi trộn-toàn-hệ; chuyển sang gom-theo-hộ mà vẫn đếm **cây** thì hộ 30 cây **không bao giờ
  chạm 100** ⇒ không bao giờ nhắc ⇒ cây không bao giờ được đóng dấu. **Không lỗi nào bật ra.** Đổi miền
  thì phải kiểm lại **trần của đại lượng**, không chỉ đổi con số.
- **Một enum công khai có thể hứa một sự phân biệt mà backend không hiện thực** (§11.2). Trước khi để một
  giá trị cấu hình vào API public, phải hỏi: *có đầu vào nào phân biệt được nhánh này với nhánh kia không?*
  — cùng câu hỏi đã dùng cho hai lớp gác trùng miền ở §9.5.
- **Cấu hình toàn cục là một giả định về số lượng chưa từng được viết ra.** `publisher_address` là `String`
  chứ không phải hàm theo lineage — đó là câu *"hệ này chỉ có một người ghi"* nói bằng kiểu dữ liệu, và nó
  im lặng cho tới ngày giả định đổi.

---

## 12. Đợt 2026-08-19/20 — **gom lô LIÊN HỘ**: bốn việc thuộc về kho NÀY

**Nguồn:** `VeDataIO/Specs#32`, comment `@GreenSun-Tech` 2026-08-19. Phần Mosaic + toàn bộ số học chi
phí: `VeDataIO/Core: docs/VEDATA-MOSAIC-STRATA-SEAM-REPORT.md` **§13**. Mục này ghi đúng phần nằm trong
kho Strata.

### 12.1 Cái gì đổi, và vì sao nó chạm tới kho này

| | §11 (08-17) | **Chốt 08-19** |
|---|---|---|
| Ranh giới lô | `author_did` (một hộ một tx) | **kích cỡ lô** — nhiều tác giả trong một tx |
| Cò neo | 100 lượt cập nhật/hộ | bỏ; cò là **một hồ sơ của `BatchPolicy`** (§5.3 spec kho này) |
| Van đáy | 90 ngày | bỏ; **cận trên ≤ 24 h**, tham số hoá tại điểm cấu hình |

Bằng chứng gom liên hộ **đã chạy trên kho này**: tx `b35ec3a5…` mang **30 lineage của 30 `author_did`
khác nhau, 30 khoá Ed25519 khác nhau** — `_anchor_batch` vốn **chưa bao giờ** đòi các ref cùng tác giả
(`node/src/routes.rs:662-747` chỉ đòi: không rỗng, không trùng ref, khoá theo thứ tự `ref_id` đã sắp).
⇒ Phía kho này **không phải gỡ ràng buộc nào** để gom liên hộ; cái phải đổi là *ai quyết lô* ở bên Mosaic.

⚠️ **Cái phải đổi ở kho này là trần lô, và nó đang chặn thấp hơn số đo của `Specs#32`.**
`Specs#32` suy `(16384 − 272)/110 ≈ 146 anchor/tx` từ `maxTxSize`; nhưng `src/settlement.rs:366`
`SinkConfig::max_metadatum_bytes = 8 KiB` từ chối **trước khi** cửa Mosaic kịp thấy lô, và cửa Mosaic có
một trần cùng bậc (`door.rs:56`, cố ý trùng để hai bên từ chối cùng chỗ). ⇒ trần thật ~**74**. Muốn chạm
146 phải nới **cả hai kho** — bảng giá chênh ~20,6 % (~22 200 → ~26 800 tADA/năm ở nhịp ngày). Hôm nay
**giữ 8 KiB**; đã báo lại ở `Specs#32`.

### 12.2 Route `_dirty` — hình dạng §11.3 **giữ nguyên**, chỉ đổi cách đọc

Ba đại lượng §11.3 nêu vẫn tính từ cùng một lượt gọi, nhưng hai trong ba đổi nghĩa:

| Đại lượng | §11.3 (theo hộ) | **Nay (liên hộ)** |
|---|---|---|
| `dirty_refs` | thành phần lô **của một hộ** | thành phần **hàng đợi toàn hệ**; lô là một lát cắt theo kích cỡ |
| `pending_versions` | cò 100 lượt/hộ | **không còn là cò**; là số liệu bậc SLA (độ sâu hàng đợi) |
| `oldest_unanchored_ts` | van 90 ngày | **cò tuổi** — cận trên ≤ 24 h (`N-1`) |

**✅ Đã land 2026-08-20** — `GET /v1/strata/_dirty` (`node/src/routes.rs`, `dirty`/`dirty_blocking`):
`ChainStore::all()` liệt kê ref (không giữ khoá đọc suốt lượt duyệt — route CHỈ ĐỌC không được làm
đường ghi đứng lại), mỗi ref trả `{ref_id hex32, author_did, head_seq, anchored_seq, pending_versions,
oldest_unanchored_ts}`, sắp **cũ trước** (`ref_id` phá hoà ⇒ tất định). `?limit=` cắt bớt nhưng đặt
`truncated = true`; `limit=0` bị **từ chối** thay vì trả rỗng.

Hai chỗ dễ sai đã canh bằng test: (a) **chưa neo lần nào ⇒ genesis cũng đang chờ**, `pending_versions =
head_seq + 1` chứ không phải `head_seq`; (b) **mốc tuổi là version chưa neo cũ nhất, không phải
genesis** — lấy nhầm genesis thì mọi lineage đã neo một lần sẽ luôn trông như quá hạn ⇒ cò tuổi bắn
liên tục ⇒ mỗi lượt một tx, đúng hành vi đắt nhất.

`author_did` **vẫn phải nằm trong response**, nhưng đổi vai: hết là khoá nhóm của lô, còn là nhãn báo
cáo / hạn mức Lamp và là chỗ P1 ráp `lineage → địa chỉ`. Lý do §11.3 nêu vẫn nguyên: `ref_id` là hàm một
chiều, coordinator **không suy ngược được**.

### 12.3 🔴 `fval_hash` không nhận salt — **lối thoát spec chừa sẵn chưa ai xây**

Đây là mục `Specs#32` xếp ưu tiên cao nhất trong ba mục "nhẹ hơn", và nó phải nói lại cho đúng bản chất:
`Strata-Math.md:292` **KHÔNG đòi** salt — nguyên văn xếp blinding vào *"Giải pháp khi cần giấu cả số
trường và chống so-khớp"*, đóng bằng *"Đây là sự đánh đổi có chủ đích… khi cần kín hơn thì bật padding +
blinding."* Tức đó là **tuỳ chọn có điều kiện**, không phải luật.

**Nhưng hồ sơ cây đúng là điều kiện kích hoạt cái tuỳ chọn đó.** Các trường *"đã phun thuốc: có/không"*,
*"giai đoạn: ra hoa"* đều **miền nhỏ**, dò cạn được bằng vét cạn tiền ảnh. Mà `src/state.rs:29` —
`fval_hash(field_value_bytes)` — **không nhận salt**, nên hôm nay **không có cách nào bật nó lên**.

⇒ Việc không phải *"code lệch spec"*, việc là **xây lối thoát spec đã chừa**. Ràng buộc kèm theo: bật
salt làm đổi `state_root` ⇒ đổi `version_hash` ⇒ **không tương thích ngược** với chain đã ghi ⇒ phải là
**chọn-tham-gia theo `Policy`**, không phải đổi hành vi mặc định.

### 12.4 Label **674** — thiếu thật, rẻ, làm

`spec/Strata-API.md:294` đã ghi kênh message người-đọc **674 CIP-20** cạnh label 1234; đường neo hôm nay
**chưa phát**. Ràng buộc: 674 là *message cho người đọc*, **không** được mang dữ liệu mà đường đọc máy
dựa vào — `resolve()` phải tiếp tục chỉ đọc 1234, nếu không ta vừa tạo một nguồn sự thật thứ hai.

### 12.5 Điều kiện thu hồi cửa `strata-anchor-batch` — đưa lên spec

Hôm nay điều kiện thu hồi chỉ nằm ở khối doc đầu `mosaic/l1/src/door.rs` (kho Core). Docstring là chỗ
**người viết cửa** đọc, không phải chỗ **người dùng cửa** đọc; và cửa này là bản **chuyển tiếp** — thứ
được trình bày như bản cuối sẽ sống rất lâu. ⇒ Điều kiện thu hồi phải nằm trong `spec/Strata-API.md`
cạnh chỗ đã chốt *"giữ cửa riêng thay vì intake"* (`:421`).

### 12.7 Đã dựng — salt cho `fval_hash` (mục 9) và §4.4 spec cửa (mục 7)

**Mục 9 — lối thoát đã xây xong ở LÕI.** `fval_hash_salted(salt, value)`, `SaltedField`,
`build_state_root_salted`, `prove_field_salted`, và `FieldProof.salt` (rỗng = không làm
mù). Salt rỗng cho kết quả **trùng từng bit** với đường cũ — bài kiểm đầu tiên của tính
năng canh đúng chỗ đó, vì `state_root` nằm trong `version_hash` **đã được ký**: một thay
đổi làm lệch root là làm hỏng chữ ký của toàn bộ lịch sử.

🔺 **Một chỗ đi khác câu chữ spec, ghi ra thay vì sửa lặng lẽ.** `Strata-Math.md:292`
viết `fvh = H_dom(TAG, salt ‖ value)`. Nối trần như vậy **nhập nhằng biên**:
`(salt="ab", value="c")` và `(salt="a", value="bc")` cho **cùng một `fvh`**. Cả hai đều do
**người ghi** chọn, còn proof thì công khai `salt` + `value` để verifier băm lại ⇒ người
ghi **đổi được lời khai về giá trị sau khi đã ký**, chỉ bằng cách dịch biên — và đúng ở
miền dữ liệu cần blinding (chuỗi ngắn) thì dựng hai cặp cùng có nghĩa là dễ. Bản này
**length-prefix** salt. Đây là điểm cần anh Đức xác nhận vào `Strata-Math` §6.3.

⚠️ **Chưa chạy trong sản xuất, nói thẳng:** còn thiếu daemon lưu salt theo từng version,
schema HTTP mang salt trên đường **ghi** (đường **đọc** đã có: `FieldProofResp.salt`), và
chỗ chọn-tham-gia theo chính sách trường. Lõi **bật được** blinding; hệ thì chưa bật.

**Mục 7 — `spec/Strata-API.md` §4.4 mới, đi PR RIÊNG chờ anh Đức** (không bọc spec chung
PR với code — tiền lệ `Strata#21`/`#27`): hợp đồng cửa `strata-anchor-batch` (phân vai,
`ref_ids` không đục, vì sao có cửa riêng) + **điều kiện thu hồi normative** (hai vế: chữ
ký owner PhoenixKey có hiệu lực **và** đường intake đã chạy thật đầu-cuối) + bảng ràng
buộc vận hành (`N-1`, `K-1`, luật thứ tự F-2/beacon, trần lô thật ~74).

Vì sao vế thứ hai của điều kiện thu hồi tồn tại: một đường thay thế chưa từng chạy thật
thì chỉ **trông như** tồn tại — đúng bài học fallback L1 của M16.

### 12.8 🪤 Chỉ lộ khi CHẠY THẬT — `no_anchor` xem trước mà **bỏ qua gác đắt nhất**

Phát hiện ở lượt chạy Preprod 2026-08-20, không phát hiện được khi đọc mã.

Đường neo có **hai** gác chống tụt-lùi-seq nằm ở hai chỗ:

| Gác | Ở đâu | Đọc gì |
|---|---|---|
| gương daemon | `routes.rs` — `g.anchored` | trạng thái **trong tiến trình** |
| gác thật | `settlement.rs:406-427` `publish_batch` | **on-chain**, qua `resolve_many` |

Nhánh `no_anchor` chỉ đi qua gác thứ nhất rồi trả sớm. Hai gác lệch nhau ở đúng ca nguy
hiểm: **daemon vừa khởi động (gương rỗng) trong khi trên chuỗi ref đã ở `seq` cao hơn**.
Khi ấy `no_anchor` trả *"lô ổn"*, còn lượt neo thật trả `RollbackAttempt`.

Hậu quả không dừng ở một câu trả lời sai: bên gọi dùng `no_anchor` để **tìm ref hỏng**
(coordinator `quarantine_probe` của Mosaic) sẽ kết luận *"không ref nào hỏng khi đứng
riêng"* rồi thử lại **y nguyên lô đó**, mãi mãi. Đúng lúc cần một câu trả lời thì nó nói
điều dễ chịu nhất.

**Vá:** nhánh `no_anchor` nay chạy `sink.resolve_many()` và áp cùng phép so `seq` như
`publish_batch`. Kèm `AnchorSink::resolve_many` lên **trait** (mặc định lặp `resolve`;
`SettlementSink` ghi đè bằng một lượt quét cho cả lô) — nếu để mặc định lặp thì một lô 74
ref là 74 lượt quét.

> **Luật rút ra:** một cửa *"xem trước"* phải chạy **đúng tập gác** mà cửa thật chạy. Chạy
> ít hơn thì nó không phải bản xem trước, nó là **một câu trả lời khác** — và nó sẽ được
> tin ở đúng lúc người ta cần sự thật nhất.

### 12.6 Lưu vết phương pháp — bổ sung cho §11.5

- **Một trần đặt "cho khớp bậc với bên kia" sẽ âm thầm trở thành trần THẬT của cả hệ.** `8 KiB` ở
  `SinkConfig` và `8 KiB` ở cửa Mosaic được đặt để *hai bên từ chối cùng chỗ* — đúng ý định, nhưng hệ quả
  là mọi phép tính dung lượng lô ở tầng trên phải lấy nó làm trần, không lấy `maxTxSize`. Ai tính bằng
  `maxTxSize` sẽ ra một con số **không sai công thức nhưng không với tới được**.
- **"Spec không đòi" ≠ "không cần làm".** `fval_hash` không salt là *hợp lệ theo câu chữ*; cái sai là hệ
  không có **đường nào** để bật blinding khi dữ liệu thật rơi vào đúng ca spec cảnh báo. Một tuỳ chọn
  không xây được thì trên thực tế là một tuỳ chọn không tồn tại.

## 13. Đợt 2026-08-24 — nguồn LÁ của checkpoint toàn cục thuộc kho NÀY

Phần Mosaic + số học: `VeDataIO/Core: docs/VEDATA-MOSAIC-STRATA-SEAM-REPORT.md` **§14**.

### 13.1 Vì sao đường đọc theo cửa sổ slot nằm ở kho này

`SEAM-REPORT` §13.16 để lại ba mảnh cho luồng checkpoint, mảnh đầu là *"nguồn lá quét
chuỗi theo cửa sổ slot"*, kèm một câu ràng buộc: **decoder label 1234 thuộc Strata,
đừng dựng bản thứ hai bên Mosaic.**

Lý do không phải gọn gàng kiến trúc. Tập lá được định nghĩa **thuần theo dữ liệu
on-chain** — *"mọi record anchor `t = 1` dưới label 1234, trong tx do publisher đã pin
CHI, có slot ∈ `[from, to)`"* — nên hai bộ giải mã cho cùng byte đó không chỉ *lệch
nhau*, mà làm **hai bên tính ra hai `root` khác nhau cho cùng một chu kỳ**. Khi ấy cam
kết on-chain thành thứ **không kiểm được**, tức mất đúng tính chất mà cả luồng sinh ra
để mua. Cùng lớp lỗi `stamp_id` 32-vs-36 byte, nhưng hậu quả nặng hơn một bậc.

### 13.2 Đã dựng — `GET /v1/strata/_settlement_window` (`#66`)

| Mảnh | Nội dung |
|---|---|
| `AnchorSink::scan_window` + `WindowAnchor`/`WindowScan` | mặc định **fail-closed** |
| `ChainQuery::tx_slot` · `tip_slot` | `BlockfrostQuery` cài `/txs/{h}` + `/blocks/latest` |
| `SettlementSink::scan_window` | luật quét |
| route `_settlement_window` | trả `tip_slot` + `scanned_txs` |

**Luật quét:** đi MỚI → CŨ; tx **trên** cửa sổ ⇒ bỏ qua rồi **đi tiếp**; tx đầu tiên
**dưới** `from_slot` ⇒ dừng — và chính nó là **bằng chứng đã phủ hết**.

#### 🔺 Gác quan trọng nhất: quét thiếu là LỖI, không phải danh sách ngắn

Hết trần quét mà chưa chạm tx nào dưới `from_slot` ⇒ `Rejected`. Vì sao nó đáng một
gác riêng: `root` tính trên tập thiếu **vẫn hợp lệ về hình thức**, vẫn chốt lên chuỗi
được, chuỗi `epoch` nhìn vẫn liên tục và cửa sổ vẫn khít — **không có gì bật ra** để
nói cam kết vừa ghi ít hơn sự thật. Một luồng sinh ra để chặn *"tin khoá vĩnh viễn"*
mà lại tự đẻ ra một cách nói dối không ai kiểm được thì nó tự huỷ.

Ba vế nhỏ hơn, mỗi vế chặn một cách hỏng:

- **Backend không quét được slot ⇒ nói ra, không trả rỗng.** Rỗng là một chu kỳ hợp
  lệ; *"không đọc được"* thì không. Gộp hai câu này thì một backend cấu hình sai sinh
  ra cả một chuỗi checkpoint toàn chu kỳ rỗng, tất cả đều "hợp lệ".
- **Lịch sử ngắn hơn trần ⇒ vẫn phủ hết.** Thiếu vế này thì chu kỳ **đầu tiên** —
  lúc ví chưa có gì cũ hơn cửa sổ — không bao giờ đóng được.
- **Tx chưa confirm không làm dừng lượt quét.** Dừng ở đó thì một tx đang chờ xác
  nhận che khuất toàn bộ cửa sổ bên dưới.

#### Hai ranh giới ghi ra thay vì để ngầm

- **Không khử trùng ở tầng đọc** — luật `(ref_id, seq)` thuộc bên tính `root`. Trộn
  hai việc thì bên đọc tự quyết cái mà cam kết on-chain phụ thuộc vào.
- **Route không quyết cửa sổ đã đủ SÂU để đóng chưa** — nó trả `tip_slot`, bên gọi tự
  quyết. Độ sâu an toàn là tham số của **mạng** và của **khẩu vị rủi ro**, không phải
  của phép đọc; ghim nó vào đây là đặt một hằng vào sai tầng.

**Chi phí:** `1 + n` lượt gọi cho `n` tx trong tầm quét — **giá của một chu kỳ**, không
phải giá của một lô, nên nó rơi vào nhịp checkpoint chứ không vào đường neo.
`scanned_txs` trả về để đo được thay vì ước.

**Nợ spec:** route này (cùng `_dirty`, `_anchor_batch`) **chưa** có trong
`spec/Strata-API.md`. `#64` vẫn đang chờ anh Đức cho §4.4; phần này đi cùng track spec
đó, không bọc chung PR với code.

**Bộ kiểm:** lib **135 pass** · node **33 pass** · fmt + clippy `-D warnings` sạch.

---

## 14. HỢP ĐỒNG TÍCH HỢP — cái `OriLife-Core` cần để cắm vào

> **Mục đích của chương này.** Đường neo đã chạy thật (§10 · §12 · §13 · `SEAM-REPORT`
> §13–§14), nhưng đầu OriLife tới nay luôn là `anchor-io/examples/orilife_e2e.rs` — một
> ví dụ **trong kho này**, do chính bên viết daemon viết. Nó chứng minh daemon chạy; nó
> **không** chứng minh một đội khác đọc tài liệu rồi cắm vào được.
>
> Chương này viết **trước** khi sửa mã, và viết ở mức *"đội OriLife-Core cầm nó là cài
> được"*: route nào, schema nào, ai ký gì, ai chạy tiến trình nào, và — phần dễ bị bỏ
> quên nhất — **chỗ nào còn là stub**.
>
> Phần Mosaic/on-chain của cùng hợp đồng: `VeDataIO/Core:
> docs/VEDATA-MOSAIC-STRATA-SEAM-REPORT.md` **§15**.

### 14.1 Ai chạy tiến trình nào — ba tiến trình, ba bộ khoá, KHÔNG gộp

Đây là mảnh hay bị hiểu sai nhất, vì cả ba đều tên "neo".

| # | Tiến trình | Kho | Ai vận hành | Khoá nó cầm | Cổng |
|---|---|---|---|---|---|
| 1 | `strata-node` | `LampNetCloud/Strata` | **VeData** | **KHÔNG có khoá bí mật nào** — chỉ pubkey trong registry | `:6690` |
| 2 | `mosaic-anchor-door` | `VeDataIO/Core` (`mosaic/l1`) | **VeData** | ví **publisher** (`MOSAIC_KEY_ANCHOR_*`) + vai beacon (`MOSAIC_KEY_BEACON_*`, đang TẮT) | `:6691` |
| 3 | `mosaic-anchor-coordinator` | `VeDataIO/Core` (`mosaic/l1`) | **VeData** | không cầm khoá — nó gọi (1) và (2) | — |
| 4 | *(bên trong 3)* lệnh `MOSAIC_CHECKPOINT_SUBMIT_CMD` | `VeDataIO/Core` (`mosaic/tx-builder`) | **VeData** | **ngưỡng operator 2-of-3** | — |

**`OriLife-Core` KHÔNG chạy tiến trình nào trong bảng trên.** Nó là **client HTTP** của
(1). Đó là toàn bộ nghĩa vụ vận hành của phía OriLife.

Và một hệ quả phải nói thẳng: **`OriLife-Core` không bao giờ nói chuyện với Cardano, với
Mosaic, hay với Blockfrost.** Không có env Blockfrost, không có ví, không có tADA. Nếu
một thiết kế phía OriLife cần một trong ba thứ đó thì thiết kế đó đã đi sai tầng — ranh
giới `Strata#1` (*"Strata giữ logic chain; Mosaic giữ tx"*) vẫn đứng.

🔑 **Vì sao (1) không cầm khoá bí mật, và vì sao điều đó là tính chất chứ không phải
tiện lợi.** Daemon **không ký** và **không băm hộ**: chữ ký do client gửi, daemon gắn
vào version rồi để **core** kiểm (`verify_strict`). Không có đường nào ghi mà bỏ qua
sig. ⇒ Một daemon bị chiếm **không** giả mạo được một version của OriLife; nó chỉ **từ
chối phục vụ**. Đó là ranh giới đáng giữ khi hai bên thuộc hai tổ chức.

### 14.2 Đường GHI — đúng ba lời gọi `OriLife-Core` phải cài

```text
  ┌─ OriLife-Core ──────────┐        ┌─ strata-node :6690 ─────────────────────┐
  │ 1. dựng state_fields    │        │                                          │
  │ 2. POST _canonical  ────┼───────▶│ dựng lại state_root + canonical_core     │
  │ 3. so BYTE, rồi KÝ      │◀───────┤ trả version_hash (32B) ← thứ phải ký     │
  │ 4. POST create/version ─┼───────▶│ verify_strict → ghi → trả seq/mmr_root   │
  └─────────────────────────┘        └──────────────────────────────────────────┘
```

#### Bước 2 — `POST /v1/strata/_canonical` (KHÔ, không ghi, không cần chữ ký)

Bước này **không bắt buộc về mặt giao thức nhưng bắt buộc về mặt thực tế**. Client phải
tự cài lại **hai cây băm** (state-tree + MMR) và **một encoding canonical** ở ngôn ngữ
của mình. Lệch một bit ⇒ đường ghi trả `403 BadSignature` — một thông điệp **không hề
nhắc tới `state_root`**, nên đội tích hợp không có manh mối nào để lần.

```jsonc
// req — genesis: seq=0, prev_hash = "00"×32, kèm genesis_nonce để nhận luôn ref_id
{ "seq":0, "prev_hash":"00…00", "content_cid":"<hex var>",
  "state_fields":[{"key":"diagnosis","value":"<hex>"}],
  "author_did":"<hex32>", "policy_hash":"<hex32>", "ts":1756100000,
  "genesis_nonce":"<hex32>" }              // optional
// 200
{ "canonical_core":"<hex>",   // so BYTE với bản mình dựng — chỗ lệch lộ ra ở đây
  "version_hash":"<hex32>",   // ← THỨ PHẢI KÝ
  "state_root":"<hex32>",     // chỗ lệch phổ biến nhất
  "ref_id":"lnref1…" }        // null khi không gửi genesis_nonce
```

Route này chạy **đúng bộ cổng** của đường ghi (kể cả gác `ts`), chỉ bỏ phần ghi.

#### `canonical_core` — byte-layout, để đội OriLife cài lại được mà không phải đọc mã Rust

`TLV length-prefix, KHÔNG phải CBOR` (đã trả ở §4.1, `OriLife-Core#161`):

| Thứ tự | Trường | Mã hoá |
|---|---|---|
| 1 | `seq` | `u64` **BE**, 8 B |
| 2 | `prev_hash` | 32 B thô (genesis: `00`×32) |
| 3 | `len(content_cid)` | `u32` **BE**, 4 B |
| 4 | `content_cid` | var, đúng `len` byte |
| 5 | `state_root` | 32 B |
| 6 | `author_did` | 32 B |
| 7 | `policy_hash` | 32 B |
| 8 | `ts` | `u64` **BE**, 8 B |

**`sig` KHÔNG nằm trong `canonical_core`** (CHỐT-1) — đổi `sig` không đổi `version_hash`.

```text
version_hash = H_dom(TAG_VER, canonical_core)
sig          = Ed25519_sign(sk, version_hash)     ← PureEdDSA TRỰC TIẾP trên 32 byte đó,
                                                     KHÔNG băm thêm lần nữa
```

`state_root`: sort khoá **tăng dần**, lá lẻ **carry** (không nhân đôi). Khoá **trùng là
400**, kể cả hai mục cùng giá trị (INV-E6 — trùng key làm root phụ thuộc thứ tự truyền
vào, mà root thì được ký).

#### Bước 4 — `POST /v1/strata/create` rồi `POST /v1/strata/:ref/version`

Schema đúng như §3 spec, cộng **một mở rộng ngoài spec** phải nói ra:
`create.policy_authors` (danh sách DID hex được phép ghi; vắng ⇒ policy một-thành-viên
`[author_did]`). **Mọi** DID trong đó phải phân giải được qua key-registry — xem §14.4.

Ba trường mà client **phải lấy từ `GET /:ref/head`** trước mỗi lần append, không được
đoán: `head_seq` (⇒ `prev_seq`), `ts` (phải gửi `ts >=` giá trị này), `policy_hash`
(sai ⇒ `403 PolicyHashMismatch`).

⚠️ **`ts` là giây, và gác này không có nút hoàn tác.** Gửi mili giây (`Date.now()`) một
lần là **khoá chết quyền ghi của ref đó tới năm 56000**: mọi version sau với `ts` giây
thật đều `TimestampRegress`, không có route sửa, không có rollback. Daemon chặn ở cửa
bằng hai gác — biên lệch đồng hồ `300 s` và một trần **tuyệt đối** `10^12` (không phụ
thuộc đồng hồ daemon, nên nó vẫn đúng khi đồng hồ container chưa kịp NTP).

### 14.3 Ai ký gì — bảng một dòng cho mỗi chữ ký trên toàn đường

| Chữ ký | Ai giữ khoá | Ký cái gì | Bên nào kiểm |
|---|---|---|---|
| `version.sig` | **OriLife-Core** (người dùng / nền tảng OriLife) | `version_hash` (32 B) | `strata-node` — `verify_strict`, low-S |
| chữ ký operator trên lô | **coordinator VeData** | payload lô gửi vào cửa | `mosaic-anchor-door` (`MOSAIC_DOOR_OPERATOR_KEYS`) |
| chữ ký tx neo | ví **publisher** VeData | tx Cardano | ledger |
| chữ ký tx checkpoint | **2-of-3 operator** VeData | tx Plutus | validator `strata_checkpoint` |

⇒ **Chỉ dòng đầu tiên thuộc về OriLife-Core.** Ba dòng dưới là việc của VeData, và cả
ba đều nằm sau ranh giới F-2.

### 14.4 🔴 `DID → pubkey` hôm nay là `InMemoryRegistry` nạp từ env — DID lạ ăn **424**

Đây là chỗ hợp đồng phải nói thẳng nhất, vì nó là **chỗ đầu tiên một lượt nối thật sẽ
hỏng**, và nó hỏng bằng một mã lỗi mà đọc tên không đoán ra nguyên nhân.

**Cơ chế hôm nay, đo bằng mã chứ không bằng trí nhớ:**

| Mảnh | Trạng thái |
|---|---|
| `node/src/registry.rs` — trait `KeyRegistry` | **có**, là chỗ cắm đúng |
| bản cài duy nhất | **`InMemoryRegistry`** — `BTreeMap<Did, VerifyingKey>` trong RAM |
| nguồn nạp duy nhất | env **`STRATA_NODE_KEYS`** = `did_hex32:pubkey_hex32` ngăn bởi dấu phẩy |
| PhoenixKey resolver | **chưa có dòng nào** |
| đường đăng ký lúc chạy | **KHÔNG có** — không route nào ghi vào registry |

⇒ **`OriLife-Core` gửi một DID mà `STRATA_NODE_KEYS` chưa có sẽ nhận `424 Failed
Dependency`, `{"error":"UnknownAuthor"}`** — ở `create` (kể cả cho từng DID trong
`policy_authors`) và ở mọi đường ghi sau đó. Fail-closed, đúng thiết kế (CHỐT-5: `Did`
là **băm** DID PhoenixKey, **không** phải pubkey, nên daemon **không được** suy khoá từ
chính `Did`).

**Ba hệ quả vận hành, cả ba đều phải nằm trong hợp đồng chứ không nằm trong đầu ai:**

1. **Đăng ký khoá là một bước THỦ CÔNG, ngoài băng.** Trước khi OriLife-Core gọi lần
   đầu, phía OriLife phải gửi cho VeData cặp `did_hex : pubkey_hex` của mọi author sẽ
   ghi; VeData đặt vào `STRATA_NODE_KEYS` rồi khởi động lại daemon.
2. **Thêm khoá = KHỞI ĐỘNG LẠI daemon.** Env chỉ đọc một lần lúc `main()`. Cộng với
   §14.5 (`store` in-memory) thì hôm nay khởi động lại còn **mất sạch hồ sơ cây** — nên
   *"thêm một author"* hiện là một thao tác **phá huỷ**. Đây là lý do daemon bền vững
   nằm ngay sau chương này trong hàng việc.
3. **Sai định dạng `STRATA_NODE_KEYS` ⇒ daemon KHÔNG khởi động** (fail-closed có chủ ý:
   chạy với registry thiếu khoá thì mọi lượt ghi đều 424, im lặng còn tệ hơn).

**❓ Chỗ KHÔNG tự quyết — xin anh Đức và anh Tuân.** `Did` là **băm** của DID PhoenixKey.
Nhưng *băm của cái gì, sau khi chuẩn hoá thế nào* thì chưa có văn bản nào chốt:

| Câu hỏi | Vì sao nó không tự quyết được |
|---|---|
| Chuỗi DID được canonicalize ra sao trước khi băm (chữ hoa/thường, `did:` prefix, phần `#fragment`, `%`-encoding, Unicode NFC) | **PhoenixKey là nguồn canonical của DID** — chốt ở đây rồi sau lệch với PhoenixKey thì mọi `ref_id` đã sinh đều trỏ sai chủ, và `ref_id` là **hàm một chiều**, không có route sửa |
| Domain-tag của phép băm đó | cùng lý do; và nó phải khớp cả hai đầu ngay từ lượt ghi đầu tiên |
| Một DID **xoay khoá** thì `Did` có đổi không | nếu đổi ⇒ policy của các lineage cũ trỏ vào một `Did` không còn ai phân giải ⇒ lineage **đóng băng**. Nếu không đổi ⇒ registry phải trả **pubkey nào** cho các version đã ký bằng khoá cũ |

Báo cáo này **ghi câu hỏi, không chốt đáp án**. Cho tới khi có đáp án, hợp đồng tạm thời
là: *`Did` là 32 byte do phía OriLife cấp, VeData nhận nguyên văn và chỉ dùng nó làm khoá
tra; hai bên giữ chung một bảng `did_hex : pubkey_hex` trao tay.* Bảng đó là **nợ**, và
nó có tên: điều kiện thu hồi của nó chính là PhoenixKey resolver cắm vào trait
`KeyRegistry`.

### 14.5 ~~🔴 `strata-node` hôm nay là IN-MEMORY~~ — **ĐÃ VÁ ở `#69`**, giữ lại làm hồ sơ

> ⚠️ **Mục này mô tả trạng thái tới 2026-08-25 và KHÔNG còn đúng.** `#69` thay `ChainStore`
> RAM thuần bằng **nhật ký ghi REQUEST** + replay qua đúng đường ghi — xem `§15`. Bảng
> "cái mất khi tiến trình dừng" dưới đây vẫn đúng **về lý do** (on-chain chỉ có
> `StrataAnchor` 104 byte, `state_root` một chiều), và chính lý do đó là thứ buộc `#69`
> phải ghi **request** chứ không ghi **trạng thái**. Giữ lại vì nó là lập luận, không
> phải một dòng trạng thái.


`node/src/store.rs`: `ChainStore { refs: RwLock<HashMap<Hash32, Arc<Mutex<ChainEntry>>>> }`.
Không có đường ghi xuống đĩa nào. Docstring của chính tệp ghi *"Bền vững (đĩa/Mirage) là
milestone sau; struct này là chỗ cắm."*

**Cái mất khi tiến trình dừng — và vì sao không dựng lại được từ chuỗi:**

| Mất | Dựng lại được từ chuỗi? |
|---|---|
| toàn bộ `StrataChain` (mọi version + `sig` + MMR) | **KHÔNG** — on-chain chỉ có `StrataAnchor` 104 byte (`ref_id ‖ head_version_hash ‖ mmr_root ‖ seq`), không có version nào |
| `policy` đang thực thi | **KHÔNG** |
| `fields` theo seq (nguồn của mọi `prove_field`) | **KHÔNG** — `state_root` là hàm một chiều |
| gương `anchored` | một phần: `resolve()` đọc lại được `seq` đã neo từ chuỗi |

⇒ Nói thẳng trong hợp đồng: **hôm nay, restart `strata-node` = mất hồ sơ cây.** Một lượt
nối thật với OriLife-Core mà không vá chỗ này thì thứ hai bên ráp được là một nguyên mẫu,
không phải một đường chạy.

Điểm nhẹ duy nhất: `ChainStore` đã là **chỗ cắm đúng** — mọi lối vào trạng thái đã đi qua
`insert` / `get` / `all`, và khoá đã là **theo từng ref**, không phải khoá toàn cục.

### 14.6 Đường NEO — `OriLife-Core` KHÔNG gọi, và gọi là hỏng

`POST /v1/strata/:ref/anchor` **tồn tại** và OriLife-Core **về mặt kỹ thuật gọi được**.
Hợp đồng vẫn nói: **đừng gọi.**

| | Ai gọi | Vì sao |
|---|---|---|
| `GET /v1/strata/_dirty` | coordinator VeData | *cái gì đã ghi mà chưa lên chuỗi* |
| `POST /v1/strata/_anchor_batch` | coordinator VeData | 1 tx / N anchor — **~0,896 tADA** thay vì ~89,6 |
| `POST /v1/strata/:ref/anchor` | *(không ai, trên đường sản xuất)* | 1 tx / 1 lineage |

Nhịp neo là **quyết định của Mosaic**, không phải của người ghi: gom lô **LIÊN HỘ**, chia
theo **kích cỡ** (chốt `Specs#32`, 2026-08-19). Một OriLife-Core tự gọi `:ref/anchor` cho
từng cây sẽ **chạy đúng** và trả về txid thật — rồi đội phí **100×** mà không lỗi nào bật
ra. Đây là chỗ *"chạy được"* và *"đúng"* tách nhau.

Nếu phía OriLife muốn **giữ nhịp** cho một hồ sơ giá-trị-cao lẻ, đại lượng để nói chuyện
là `anchor_priority` của Stamp (`immediate` · `milestone` · `batch_daily` · `no_anchor`),
không phải một lời gọi trực tiếp.

### 14.7 Đường ĐỌC — cái OriLife-Core lấy ra để chứng minh với bên thứ ba

| Route | Trả | Ghi chú hợp đồng |
|---|---|---|
| `GET /:ref/head` | `head_seq` · `mmr_root` · `ts` · `policy_hash` | **bắt buộc gọi trước mỗi append** |
| `GET /:ref/version?at=<unix_ts>` | version tại thời điểm t + `InclusionProof` | |
| `GET /:ref/proof/version/:seq` | `InclusionProof` — so với `mmr_root` **đã neo** | INV-E3 |
| `GET /:ref/proof/field/:key[?seq=]` | `FieldProof` | ⚠️ có trường **`salt`** — xem dưới |

⚠️ **`salt` là trường mà spec §3 chưa có, và bỏ qua nó làm MỌI proof đỏ ở phía client.**
Verifier phải băm theo **length-prefix** (`fval_hash` nhận salt từ `Strata#63`), không
phải nối trần `salt ‖ value` như `Strata-Math.md:292` đang viết. Rỗng = không làm mù.
Trường này **luôn có mặt kể cả khi rỗng**, đúng vì lý do trên: một trường vắng mặt lúc
blinding được bật sẽ làm mọi proof đỏ ở client mà nhìn từ server thì không có gì sai.

### 14.8 Bảng lỗi `OriLife-Core` phải map — và ba cái dễ hiểu sai nhất

Body lỗi luôn là `{ "error":"<tên biến thể>", "detail":{…} }`, tên giữ **nguyên văn**
tên biến thể lõi để client map ngược được.

| HTTP | `error` | Nghĩa với người tích hợp |
|---|---|---|
| **424** | `UnknownAuthor` | 🔴 **DID chưa đăng ký trong registry** — §14.4. Không phải "sai khoá", không phải "hết hạn" |
| **403** | `BadSignature` | ký sai — **99 % là lệch `canonical_core`/`state_root`**, không phải sai khoá. Dùng `_canonical` để tìm chỗ lệch |
| **403** | `PolicyHashMismatch` | chưa gọi `head` trước khi append |
| **403** | `PolicyDenied` | author không nằm trong policy của lineage |
| **409** | `HashLinkBroken` | `prev_hash` ≠ head — có người ghi chen vào |
| **422** | `SeqNotMonotonic` | `prev_seq` sai |
| **422** | `TimestampRegress` | `ts` < `ts` head |
| **422** | `TimestampTooFarFuture` | 🔴 **gửi mili giây** — đọc cảnh báo §14.2 |
| **400** | *(mức cửa)* | body hỏng, hex sai độ dài, **khoá trùng trong `state_fields`** |
| **409** | `AnchorRollback` | neo lại seq ≤ seq đã neo (đường neo, không phải đường ghi) |
| **501/502/503** | `AnchorNotConfigured` / `AnchorRejected` / `AnchorNetwork` | backend neo — **chỉ 503 là retry được** |

**Ba cái dễ hiểu sai nhất** — đáng nằm trong tài liệu tích hợp phía OriLife, không chỉ ở
đây: `424 UnknownAuthor` không nói gì về khoá; `403 BadSignature` không nhắc `state_root`
dù đó gần như luôn là nguyên nhân; `422 TimestampTooFarFuture` là gác **duy nhất** đứng
giữa một lỗi gõ và một lineage chết vĩnh viễn.

### 14.9 Bảng TRUNG THỰC — chỗ nào còn là stub

Đây là mục quan trọng nhất của cả chương, vì nó là thứ một hợp đồng tích hợp hay thiếu.

| # | Mảnh | Trạng thái đo được | Hậu quả nếu đội OriLife không biết |
|---|---|---|---|
| 1 | `KeyRegistry` → PhoenixKey | 🔴 **stub** — `InMemoryRegistry` + env `STRATA_NODE_KEYS`, không route đăng ký | lượt gọi đầu tiên ăn `424`, và không ai đoán ra vì sao |
| 2 | `ChainStore` → đĩa | ✅ **ĐÃ VÁ** (`#69`) — nhật ký ghi REQUEST, replay qua lõi | restart dựng lại đúng. Nợ còn lại: **nén/ảnh chụp**, có ngưỡng số ở `SEAM §17.6` |
| 3 | `fields` → Mirage | 🟠 **stub** — byte nằm thẳng trong RAM (§8.4 nói bản thật giữ CID) | ~~mất cùng lúc với (2)~~ — nay dựng lại được qua replay; nợ còn lại là **kích thước**, không phải mất mát |
| 4 | `canonicalize(DID)` | 🟡 **2/3 đã chốt** (`Specs#32` 2026-08-27): `blake2b_256(UTF-8(did))` không salt không domain-tag; xoay khoá **không** đổi `Did`. Câu 3 còn treo nhưng **không quan sát được** trong khuôn PhoenixKey — §17.2 | hai bên băm ra hai `Did` khác nhau ⇒ `424`; Strata bọc thêm `H_dom` là ra `Did` KHÁC |
| 5 | 4 route `_canonical` `_dirty` `_anchor_batch` `_settlement_window` | 🟠 `#64` **mở**, chờ anh Đức merge phần chữ | đội tích hợp đọc spec **không thấy** `_canonical` tồn tại. Đường vòng: `scripts/orilife_handshake.py` |
| 6 | trường `salt` của field-proof | 🟠 `#64` §3 **đã viết**, kèm vế CHẾ ĐỘ; chờ merge | verifier bỏ qua ⇒ không chỉ thiếu đầu vào mà **băm nhầm miền** (`#71`) |
| 7 | `Strata-Math §6.3` | ✅ **ĐÃ VÁ** (`#71`, MERGED) — sai **hai** chỗ chứ không một: nối trần **và** chung tag | ai cài theo spec cũ thì verify đỏ; chung tag thì proof **khai sai vẫn xanh** |
| 8 | beacon | 🟠 **TẮT** (`Specs#32`) — không ảnh hưởng OriLife | — |
| 9 | `N-1` nhịp neo | 🟠 chưa ghim (chờ `Mosaic-Math`) — hồ sơ sản xuất `epoch 8 h / tuổi 24 h`, **24 h là trần cứng** | SLA neo chưa cam kết được bằng số |
| 10 | mainnet | ⛔ **đóng** — `K-1`, chỉ Preprod | — |

**(5) là một đính chính về phạm vi.** §13.2 và `SEAM-REPORT` §14.6 đều ghi nợ spec là
*"3 route"*. Đo lại toàn tệp: `_canonical` **cũng** chưa có chương — hit duy nhất của
chuỗi đó trong `Strata-API.md` là chữ `entry_bytes_canonical` ở §8.3, không liên quan.
Nợ thật là **4 route + 1 trường**, và `_canonical` là cái **đắt nhất** trong bốn: nó là
đúng đường mà một đội tích hợp mới cần và cũng là đường duy nhất họ không có cách nào
biết là có.

### 14.10 Cái hợp đồng này KHÔNG nói

Ghi ra để không ai đọc nhầm phạm vi:

- **Không** chốt `canonicalize(DID)` — của anh Đức và anh Tuân (§14.4).
- **Không** chốt nhịp neo (`N-1`) — chờ `Mosaic-Math`.
- **Không** mở mainnet — `K-1` còn hiệu lực, mọi lượt chạy thật là **Preprod**.
- **Không** đụng PhoenixKey trong đợt này — chỉ ghi ra rằng chỗ cắm là trait
  `KeyRegistry` và điều kiện thu hồi của bảng khoá trao tay là resolver đó.

---

## 15. Daemon BỀN VỮNG — `store` in-memory → nhật ký trên đĩa (`#68`)

Việc chặn duy nhất của bảng §14.9. Hôm nay `strata-node` restart là **mất hồ sơ cây**, và
§14.5 đã đo là **không dựng lại được từ chuỗi**: on-chain chỉ có `StrataAnchor` 104 byte.

### 15.1 🔑 Ghi REQUEST, không ghi TRẠNG THÁI — và đó là cả giá trị của đợt này

Cách hiển nhiên là tuần tự hoá `ChainEntry` (versions + MMR + policy) rồi nạp lại. Cách đó
**bỏ qua lõi**: mọi bất biến `INV-E1/E2/E4` và `verify_strict` chỉ chạy ở đường ghi, nên
một tệp bị sửa — hỏng đĩa, tay người, một bản vá sai — nạp vào thành một `StrataChain`
**chưa bao giờ đi qua cửa nào**. Sau đó nó phục vụ proof, và những proof ấy **verify đúng**.

Nhật ký này vì thế ghi **đúng cái client đã gửi và cửa đã nhận**, rồi replay bằng cách
**gọi lại chính hàm của đường ghi** (`create_inner` · `append_inner` · `audit_inner` ·
`publish_anchor`). Hệ quả là một **tính chất**, không phải một lời hứa:

> Nhật ký chỉ chứa được những lịch sử mà cửa sẽ nhận **lần nữa**.

Đo bằng cách cố ý phá: đổi `content_cid` của một version từ `"beef"` thành `"bee0"` trong
tệp ⇒ replay đỏ `403 BadSignature` ⇒ **daemon không khởi động**. Không có đường nào để nó
phục vụ lịch sử giả.

Giá phải trả nói thẳng: replay là `O(n)` lượt `verify_strict`. Nó **đo được**, không ước —
xem §15.7.

Cùng lý do ấy dẫn tới một chi tiết dễ tưởng là thừa: `create_inner` / `audit_inner` được
**tách ra** khỏi handler thay vì viết bản thứ hai cho replay. Hai đường dựng genesis là
**hai định nghĩa cho cùng một vị ngữ**, và chúng lệch nhau vào ngày không ai nhìn.

### 15.2 `Did → pubkey` KHÔNG nằm trong nhật ký — và cái giá đã biết của việc đó

`Create` chỉ ghi **danh sách `Did`**, không ghi pubkey; replay phân giải khoá qua
**key-registry** như đường ghi thật (CHỐT-5). Ghi pubkey vào tệp là dựng **nguồn sự thật
thứ hai** cho đúng thứ registry sinh ra để là nguồn duy nhất — và hai nguồn thì lệch nhau.

**Cái giá, ghi ra để nó là hành vi có chủ ý chứ không phải một bất ngờ:** gỡ một khoá khỏi
`STRATA_NODE_KEYS` rồi khởi động lại ⇒ **daemon từ chối lên**, kèm tên dòng và
`424 UnknownAuthor`. Có một bài kiểm riêng cho đúng ca đó.

Fail-closed đúng chiều: phục vụ một lineage mà ta **không còn xác minh được chủ của nó**
thì tệ hơn là không phục vụ. Nhưng nó nối thẳng vào §14.4 — chừng nào đăng ký khoá còn là
một biến env, thì *"xoay khoá"* và *"khởi động lại"* còn là cùng một thao tác nguy hiểm.

### 15.3 Ba bất biến của **thứ tự ghi** — mỗi cái chặn một cách mất im lặng

| Thao tác | Nhật ký ghi ở đâu | Cái nó chặn |
|---|---|---|
| `create` | **dưới cùng khoá ghi của kho**, giữa `contains_key` và `insert` | bản ghi `Create` mồ côi (replay `RefExists`, daemon không lên) **và** ref sống trong RAM mà tệp không biết |
| `append` / `audit` | **sau** khi lõi nhận, **vẫn trong khoá của ref** | hai version đảo chỗ trong tệp ⇒ replay dựng ra một chuỗi khác |
| `anchor` | **sau** khi backend trả biên nhận | ref bị **cháy**: replay dựng lại một ref "đã neo" mà chuỗi không biết ⇒ `AnchorRollback` vĩnh viễn |

Bất biến của dòng đầu đáng viết ra thành câu: **bản ghi `Create` có trong nhật ký ⟺ ref đã
được chèn.** Tách hai bước ra ngoài khoá thì mất nó theo **cả hai chiều**. `create` là thao
tác một-lần-mỗi-lineage, nên đây là chỗ đúng để đổi thông lượng lấy một bất biến không có
cách nào phục hồi nếu vỡ.

Neo **lô** ghi `N` bản ghi rồi **fsync một lần** — lô là *một tx*, nên nó cũng là *một* sự
kiện bền vững. Mọi khoá ref của lô vẫn đang được giữ lúc đó, nên không ai chen vào giữa.

### 15.4 Ghi hỏng ⇒ nhật ký **tự đầu độc**, cửa trả `503`

Ghi **trước** khi lõi kiểm thì nhật ký chứa cả request bị từ chối ⇒ replay đỏ. Nên phải ghi
**sau**. Nhưng khi ấy một lượt ghi đĩa hỏng để lại trạng thái RAM **đã tiến** quá trạng thái
bền vững, và mọi request sau đó xây tiếp lên một nền sẽ biến mất.

⇒ Ghi hỏng ⇒ `Journal` bật cờ đầu độc **trước khi trả lỗi** (người gọi có thể nuốt lỗi, cờ
thì không), mọi lượt ghi sau trả `503 JournalBroken`. **Đọc vẫn phục vụ** — dữ liệu trong
RAM vẫn đúng, chỉ là daemon không nhận thêm việc nó không nhớ nổi.

`503` chứ không `500`: `500` nói *"lỗi bất ngờ"*, còn đây là một trạng thái **đã biết và đã
chọn**.

Bài kiểm dùng **`/dev/full`** (mọi lượt ghi trả `ENOSPC`) — một lỗi I/O **thật**, không phải
một cờ do bài kiểm tự bật.

### 15.5 🪤 Đuôi rách — bỏ **đúng** dòng cuối, và điều kiện đặt theo BYTE

Tiến trình chết giữa một lượt `write_all` để lại một dòng không có `\n` kết thúc. Bỏ nó là
**khôi phục đúng sự thật**: cửa chỉ trả `200` sau khi `append` nhật ký trả `Ok`, nên dòng
đó là một thao tác client **chưa từng được báo thành công**.

Điều kiện đặt theo **byte cuối tệp**, không theo *"dòng cuối có parse được không"*: một
dòng rách vẫn có thể **tình cờ parse được** (JSON cụt ở đúng chỗ), và khi ấy phép thử theo
nội dung sẽ nhận vào một bản ghi cụt — im lặng.

Và `fsync` không phải chi tiết thừa: thiếu nó thì *"đã ghi"* chỉ có nghĩa *"đã nằm trong
cache của hệ điều hành"* — đúng thứ biến mất trong chính ca mà nhật ký sinh ra để sống sót.

### 15.6 `STRATA_NODE_JOURNAL` **bắt buộc** — phù du phải được KHAI

Chạy phù du là một lựa chọn hợp lệ (dev, test, một lượt thử). Mất hồ sơ vì **không ai nghĩ
tới nó** thì không. Hai ca ấy phân biệt bằng đúng một thứ: người vận hành đã **nói ra** hay
chưa.

```
(thiếu env)                ⇒ TỪ CHỐI khởi động, mã thoát 1
STRATA_NODE_JOURNAL=none   ⇒ chạy, in "⚠️ kho PHÙ DU, restart là mất sạch hồ sơ cây"
STRATA_NODE_JOURNAL=<path> ⇒ replay rồi mới mở cổng
```

Cùng khuôn với `--commit` của bootstrap checkpoint (`SEAM §14.8`) và với **beacon mặc định
TẮT**: chỗ nào mất mát không lấy lại được thì mặc định phải là chỗ **đòi người vận hành
khai**.

Và replay chạy **trước khi mở cổng**: một daemon nhận request trong lúc còn đang dựng lại
chính mình sẽ trả `404` cho những ref nó sắp có, rồi ghi vào một chuỗi chưa đủ dài.

### 15.7 ✅ Nghiệm thu — restart THẬT bằng chính bin, kèm đối chứng

Lượt 1 — daemon lên với nhật ký mới, ví dụ OriLife ghi 3 cây × 3 version:

```
nhật ký: /tmp/strata-restart-demo.jsonl — replay 1 bản ghi trong 246,99µs: 0 ref · 0 version
_dirty  → count=3 total_pending=9   (head_seq=2 cả ba)
```

Tắt tiến trình. Lượt 2 — **tiến trình mới**, cùng tệp:

```
nhật ký: /tmp/strata-restart-demo.jsonl — replay 10 bản ghi trong 88,40ms: 3 ref · 9 version
_dirty  → count=3 total_pending=9   (head_seq=2 cả ba)   ← KHỚP
prove_field/tree_state → state_root be9338d8… , version_seq 2   ← `fields` sống
head    → mmr_root e285a2cf… , head_version_hash caf9cf65…
```

**Đối chứng là phần bắt buộc:** cùng bin, nhật ký **mới rỗng** ⇒ `_dirty count = 0`. Không
có nó thì ba dòng "KHỚP" ở trên cũng đúng với một daemon đọc lại chính bộ nhớ cũ.

`prove_field` là phép đo đáng giá nhất trong bảng: `state_root` là hàm **một chiều**, nên
mất `fields` là mất `prove_field` **vĩnh viễn** dù `chain` còn nguyên — và một bản replay
quên đúng nhánh đó vẫn qua sạch phép so `head`.

**Số đo quy mô** (bản release, 40 lineage × 25 version):

| | |
|---|---|
| nhật ký | 1 001 dòng · **560 267 byte** ⇒ ~**560 B/version** |
| replay | **54,18 ms** cho 1 000 version ⇒ ~**54 µs/version** |

Ngoại suy để biết ngưỡng chứ không để yên tâm: **1 triệu version ⇒ ~54 s replay, ~560 MB
tệp**. Đó là chỗ nợ nén/ảnh chụp bắt đầu có thật — và nay nó có **một con số**, không phải
một linh cảm.

### 15.8 Đảo mã — bốn mũi, mỗi mũi đỏ đúng bài của nó

Bộ kiểm xanh mà đảo mã vẫn xanh là bộ kiểm trang trí.

| Mũi đảo | Bài đỏ |
|---|---|
| bỏ xử lý **đuôi rách** | `duoi_rach_bo_dung_dong_cuoi_va_phan_con_lai_van_song` |
| bỏ **đối chứng `Anchor.seq`** | `anchor_seq_lech_thi_tu_choi_chu_khong_nap_vao_guong` |
| đường ghi **không ghi nhật ký** | 5 bài, gồm cả `audit_log_song_qua_restart` |
| bỏ **đầu độc** khi ghi hỏng | `ghi_hong_that_thi_dau_doc_va_cua_tra_503` |

`Anchor.seq` đáng một mũi riêng vì nó là chỗ dễ hiểu nhầm nhất: nó **KHÔNG** phải giá trị
nạp lại — replay tính lại `seq` bằng `publish_anchor()` rồi **so** với con số trong tệp.
Lệch nghĩa là chuỗi dựng lại **khác** chuỗi lúc ghi, tức mọi proof daemon sắp phục vụ nói
về một lịch sử khác lịch sử đã neo lên chuỗi. Nạp thẳng con số đó vào gương thì không có gì
bật ra.

**Bộ kiểm:** lib **135** · node **42** (33 + **9** mới) · toàn workspace **252 pass / 0
fail** · fmt + clippy `-D warnings` sạch.

### 15.9 Còn hở — nói ra, không giấu

| # | Chỗ hở | Đo được |
|---|---|---|
| 1 | **Cửa sổ giữa "on-chain nhận" và "fsync xong"** | tiến trình chết đúng khe đó ⇒ mất bản ghi `Anchor`. Replay để `last_anchor_seq = None` ⇒ ref lại nằm trong `_dirty` ⇒ coordinator neo lại **cùng `seq`**. Gác on-chain là `c.seq > a.seq` (`routes.rs:1000`) nên **bằng nhau vẫn qua** ⇒ hậu quả là **một tx trùng nội dung**, tốn phí, **không** phải ref kẹt. Ghi ra vì nó là đánh đổi, không phải sơ suất: đường duy nhất đóng hẳn là two-phase commit với chuỗi, mà chuỗi không có `prepare` |
| 2 | **Không nén, không ảnh chụp** | tệp lớn tuyến tính theo version; ngưỡng đã đo ở §15.7 |
| 3 | `fields` vẫn nằm **trong nhật ký**, chưa đi Mirage | §8.4 nói bản thật giữ CID. Hôm nay byte nằm cùng request đã ký ⇒ nhật ký mang cả dữ liệu riêng tư |
| 4 | **Một tệp, một tiến trình** | không có khoá tệp: hai daemon cùng trỏ một nhật ký sẽ ghi xen kẽ và cả hai đều sai. Chưa gác |
| 5 | Xoay khoá vẫn **phá** | §15.2 — nối vào nợ PhoenixKey của §14.4 |

---

## 16. Đường neo sản xuất đã **401 suốt 4 ngày** — và vì sao không ai thấy (`#70`)

Phát hiện trong lúc dựng lượt chạy đầu-cuối *có lá* (`SEAM §16`). Không phải một lỗi
mới viết ra: nó **đã sống trên `main` của cả hai kho** từ 2026-08-21.

### 16.1 Mốc thời gian — đo bằng `git`, không bằng trí nhớ

| Ngày | Việc | Cửa có gác chữ ký operator? |
|---|---|---|
| 08-15 | cửa `strata-anchor-batch` dựng (`Core#58`) | **không** |
| 08-17 | `dfd242a` thêm gác — trên **nhánh** | không (chưa vào `main`) |
| **08-20 17:02** | ✅ lượt chạy thật **lô LIÊN HỘ**, 3 tx Preprod (§12.8) | **không** |
| **08-21 08:48** | `Core#96` merge ⇒ gác **vào `main`** | **có** |
| 08-24 | hai đợt chạy thật — nhưng là luồng **checkpoint** | *(không đi qua cửa neo)* |
| **08-25** | lượt chạy này | 🔴 **401** |

⇒ Giữa mốc gác land và hôm nay, **không lượt chạy nào đi qua đường
`Strata → cửa Mosaic`.** Hai đợt 08-24 đều đi đường Plutus của coordinator, một seam
khác hẳn. Đường neo — đường **chính** của cả MB-6 — nằm hỏng bốn ngày.

### 16.2 Hai lỗi, và lỗi THỨ HAI mới là lý do lỗi thứ nhất vô hình

**Lỗi 1 — bên gửi không ký.** `MosaicDoorSubmitter::submit` gửi
`{label, payload_cbor, ref_ids, beacon, network}`. Cửa đòi thêm `operator_vkey` +
`operator_sig`, và `check_operator_signature` **từ chối `401`** khi thiếu — không có
nhánh "coi như hợp lệ". Đo thẳng vào cửa, có đối chứng:

```
gửi lô KHÔNG chữ ký  → HTTP 401 {"error_kind":"Unauthorized",
                                 "error":"thiếu hoặc sai chữ ký operator …"}
gửi KHÔNG token      → HTTP 401 {"error_kind":"Unauthorized",
                                 "error":"thiếu hoặc sai `Authorization: Bearer …`"}
```

*(Đối chứng thứ hai là phần bắt buộc: không có nó thì dòng đầu cũng đúng với một cửa
từ chối mọi thứ vì lý do khác.)*

**🪤 Lỗi 2 — `Unauthorized` bị gộp vào `NotConfigured`.** Bảng ánh xạ của bên gửi có:

```rust
Some("NotConfigured") | Some("Unauthorized") => AnchorError::NotConfigured,
```

Nên một cửa **401** hiện ra ở đầu Strata là:

```
HTTP 501  {"error":"AnchorNotConfigured","detail":{"detail":"daemon chưa cắm AnchorSink"}}
```

> Người vận hành đọc câu đó sẽ đi kiểm cấu hình sink — **đúng chỗ không có lỗi**. Còn
> chỗ có lỗi thì không được nhắc tới một chữ.

Hai trạng thái này khác hẳn nhau: `NotConfigured` là *ta chưa cắm gì*; `Unauthorized`
là *ta đã cắm, đã gọi tới nơi, và bị **từ chối***. Gộp chúng làm mất đúng thông tin
phân biệt được hai việc phải làm khác nhau.

Và nó cộng dồn với một tính chất **cố ý** của cửa: cửa trả **một thông điệp chung** cho
mọi đường hỏng của chữ ký (phân biệt "khoá lạ" với "chữ ký sai" biến cửa thành **máy dò
allow-list**). Bên cửa im lặng có chủ ý là đúng — nhưng khi bên gửi cũng bóp méo nốt
`error_kind` thì tín hiệu **mất cả hai chặng**.

### 16.3 Bản vá — và ba chỗ suýt dựng sai

| Vá | Nội dung |
|---|---|
| ký lô | `MOSAIC_DOOR_OPERATOR_SK` (hex 32 B seed ed25519); ký **sau** `encode_records`, trên **đúng byte gửi đi** |
| fail-closed | có `MOSAIC_DOOR_URL` mà thiếu khoá ký ⇒ **không khởi động** (cùng khuôn gác token đã có) |
| ánh xạ lỗi | `Unauthorized` ⇒ `Rejected` **giữ nguyên văn lời cửa nói**, kèm tên hai env phải kiểm |
| chẩn đoán | in **`operator_vkey=…`** lúc khởi động — nó là khoá **công khai**, chính là thứ phải nằm trong allow-list của cửa |

**Chỗ suýt sai thứ nhất — hai nguồn sự thật cho `network`.** Bản nháp đầu đọc
`MOSAIC_DOOR_NETWORK` làm env **bắt buộc**. Nhưng `sink_config` **đã** phân giải mạng từ
`STRATA_ANCHOR_NETWORK` để chọn endpoint Blockfrost. Mạng nằm **trong thông điệp được
ký**, nên hai nguồn cho cùng giá trị = hai thông điệp cho cùng một lô, và triệu chứng là
`401` không nói lý do. ⇒ Mạng thành **tham số** truyền xuống, env thứ hai bị bỏ đọc.

**Chỗ suýt sai thứ hai — ký ở sai tầng.** Thông điệp phủ `payload`, mà `payload` chỉ tồn
tại **sau** `encode_records`. Ký ở tầng trên là ký một byte khác byte gửi đi.

**Chỗ suýt sai thứ ba — in khoá.** In `operator_vkey` nghe như rò rỉ. Nó là khoá **công
khai**; thứ đắt ở đây là *không in* — người vận hành mất đường đối chiếu một chuỗi hex
trước lượt neo đầu, và đường duy nhất còn lại là đọc một `401` cố ý câm.

### 16.4 🔺 `operator_sig_message` là **bản sao của một định nghĩa sống ở kho khác**

Đây là chỗ nguy hiểm nhất của bản vá, nên nó được ghi ra thay vì để ngầm: hàm này phải
khớp **từng byte** với `Core: mosaic/l1/src/door.rs::operator_sig_message`.

```text
blake2b-256( "MOSAIC-STRATA-BATCH-v1" ‖ u8(len(net)) ‖ net
             ‖ u64be(label) ‖ u8(beacon) ‖ u64be(len(payload)) ‖ payload )
```

Một bài kiểm chỉ so bản này với **chính nó** sẽ xanh vĩnh viễn kể cả khi hai bên đã
lệch — **xanh giả**, và triệu chứng ngoài đời đúng là cái vừa xảy ra. Nên bộ kiểm ghim
**5 vector sinh từ chính cửa**, cộng hai bài mà thiếu chúng thì vector cũng vô dụng:

| Bài | Nó chặn |
|---|---|
| `thong_diep_ky_khop_tung_byte_voi_cua_mosaic` (5 vector) | hai bản lệch nhau |
| `moi_dai_luong_deu_doi_thong_diep` | một bản **bỏ quên** `beacon`/`label`/`network` vẫn khớp 5/5 vector đã ghim |
| `bien_giua_network_va_payload_khong_nhap_nhang` | mất length-prefix ⇒ `net="ab"‖payload="c"` = `net="a"‖payload="bc"` |

**Bộ kiểm:** anchor-io 8 → **12** · node **19** (sink_config + ca âm thiếu khoá ký) ·
workspace **260 pass / 0 fail** · fmt + clippy `-D warnings` sạch.

### 16.5 Luật rút ra

> **Một gác mới ở phía nhận là một thay đổi phá vỡ hợp đồng của phía gửi — kể cả khi
> hai phía nằm ở hai kho và CI của cả hai đều xanh.**

Cùng lớp với *"soát PR bằng cách GHÉP"*: chỗ hỏng nặng nhất nằm ở **mối nối**, và mối
nối không thuộc bộ kiểm của bên nào. Ở đây nó còn thêm một tầng: bên gửi **bóp méo mã
lỗi** của bên nhận, nên ngay cả một lượt chạy thật cũng chỉ ra một câu sai.

Cái đã bắt được nó không phải một bài kiểm mới — mà là **chạy thật đường đó một lần**.

---

## 17. Phiên 2026-08-27 — miền băm `fvh`, và hợp đồng DID sau phản hồi của anh Đức

> Nửa VeData của chương này (vận hành · runbook · ngưỡng) nằm ở
> `VeDataIO/Core: docs/VEDATA-MOSAIC-STRATA-SEAM-REPORT.md` **§17**. Chương này giữ phần
> thuộc kho Strata: miền băm, schema dây, vector chung, và kịch bản bàn giao.

### 17.1 🔴 `#71` — hai chế độ `fvh` đổ chung một miền

`fval_hash` và `fval_hash_salted` dùng **chung** `TAG_STATE_FVAL`. Length-prefix của
`#63` phân tách `salt` với `value` **bên trong** nhánh có salt; nó không phân tách
**nhánh có salt với nhánh không salt**.

```
V = u32_be(|S|) ‖ S ‖ M        (không làm mù)
fval_hash(V)  ==  fval_hash_salted(S, M)      ← trùng cả 32 byte
```

Người ghi cam kết `V`, rồi xuất `FieldProof` khai `salt = S, value = M`. `verify_field_proof`
băm lại khớp, `state_root` khớp, đường anh em không đụng — **xanh**. Tức **đổi được lời
khai về giá trị sau khi `state_root` đã nằm trong `version_hash` đã ký**, không cần va
chạm băm, không cần khoá nào.

Cùng lớp lỗi mà `nhap_nhang_bien_salt_value_bi_chan` đã chặn, lùi lên một bậc: chặn giữa
hai **trường** thì được, giữa hai **chế độ** thì chưa.

**Vá:** tag riêng `LN/STRATA/state/fval/salted/v1`. Nhánh không salt giữ tag cũ ⇒ trùng
từng bit, không `state_root` đã ký nào phải tính lại.

**Soát trước khi merge** — dựng lại lỗ độc lập (tự nối `V`, không gọi `fval_hash_salted`
để sinh nó): trùng `2f4ddaa3b1b3a497…`. Đảo mã một dòng: cả hai PoC **đỏ**. Ba gác CI
chạy tại máy — `262 pass` (workspace) · `fmt` sạch · `clippy` 0.

**Bán kính vụ nổ = 0 trên ba kho** (câu anh Đức để ngỏ trong `#71`):

| Kho | Kết quả |
|---|---|
| `Strata` đường **ghi** | `node/` chưa bao giờ dựng `SaltedField`; không route nào nhận `salt` |
| `VeDataIO/Core` | 0 chỗ tính `fvh` |
| `OriLifeTrace/OriLife-Core` `strata_client.py` | chỉ cài đường **không salt** |

🪤 Lần grep đầu trỏ vào `src/node/` — **thư mục không tồn tại**, nên "0 hit" là **rỗng
giả**. Vùng đúng là `node/` + `anchor-io/`. Kết luận không đổi; đường tới nó thì suýt sai.

### 17.2 🔴 `§3` của `#64` lệch mã ba chỗ — và một trong ba là an toàn

| | `§3` viết | Mã | Hỏng kiểu gì |
|---|---|---|---|
| tag | một | **hai**, chọn theo `salt` rỗng hay không | băm **nhầm miền** |
| prefix `salt` | `len(salt)` | `u32_be(len(salt))` 4 byte BE | lệch byte nếu cài 1-byte/varint |
| prefix `value` | `len(value)` | **không có** | thêm vào là **đổi byte** |

Anh Đức xếp chỗ thứ ba là *"không phải lỗi an toàn — `value` là phần còn lại nên biên
vẫn xác định duy nhất"*. Đúng, nhưng nó vẫn làm **mọi proof có salt đỏ** ở phía client
trong khi nhìn từ server không có gì sai — nên hậu quả vận hành giống hệt hai chỗ kia.

`§3` nay là bảng hai dòng + ba chi tiết dễ chép sai. Đoạn *"`salt` LUÔN có mặt"* cũng
sửa: `salt` không phải một **đầu vào** của một phép băm — nó **chọn chế độ**.

### 17.3 🟠 `#72` — chế độ phải NHÌN THẤY ĐƯỢC từ ngoài

`#71` vá lõi; `OriLife-Core` không đọc lõi. Hai chỗ họ **thật sự** đọc vẫn dạy công thức cũ:

| Chỗ | Vấn đề |
|---|---|
| `node/src/dto.rs:324` — chú thích `salt` của `FieldProofResp` | viết *"băm `salt ‖ value`"* — sai **hai lần** sau `#71` |
| `apis/canonical-core-vectors.json` | không có `tag_fval_salted`, không có quy tắc chọn chế độ |

🔺 **Vì sao vector thuận không đủ.** Mọi vector thuận đi **một chiều**: tính `fvh` từ
`(salt, value)`. Một bản cài dùng **chung** một tag vẫn khớp **toàn bộ** vector thuận —
không ca nào phát hiện hai miền đã chồng lên nhau. Phải có **đối chứng âm**:

```
NC1  fvh_salted(S, M) ≠ fvh_khong_salt(u32_be(|S|) ‖ S ‖ M)   ← khai thác của #71
NC2  fvh_salted("ab","c") ≠ fvh_salted("a","bc")              ← biên salt/value
```

Hai chi tiết cố ý:

- **`V` dựng lại trong test**, không đọc từ file — đọc từ file thì ai sửa `V` trong JSON
  là biến đối chứng thành trang trí mà vẫn xanh;
- `NC2` kèm khẳng định phụ rằng hai cặp **nối trần ra cùng một chuỗi** — thiếu nó thì
  `NC2` xanh vì hai cặp vốn khác nhau, chứ không phải vì length-prefix có tác dụng.

Bốn ca thuận: `M1` salt rỗng (phải trùng **bit** đường cũ) · `M2` salt 30 B · `M3` salt
**1 byte** (bắt bản cài dùng 1-byte length thay `u32_be`) · `M4` value **rỗng** (bắt bản
cài tự thêm `len(value)` cho "đối xứng").

Đảo mã hai mũi, cả hai làm cả hai test mới **đỏ**. Diff tệp vector **+16/−0** — bằng
chứng đường cũ không đổi một byte. `264 pass` · `fmt` sạch · `clippy` 0.

### 17.4 🟠 Hợp đồng `Did` — ba điều đã chốt

**(1)** `Did = blake2b_256(UTF-8(did))` — **không** salt, **không** domain-tag
(`PhoenixKey-Anchorme-Tech.md:68` → `phoenix_address.rs:52`). Khác quy ước `H_dom` của
Strata **có lý do**: khớp quy ước gốc bên PhoenixKey.

⚠️ **Strata bọc thêm `H_dom` là ra một `Did` KHÁC** cho cùng một người — và không lỗi nào
bật ra, chỉ `424 UnknownAuthor` mãi mãi.

**(2)** Xoay khoá **không** đổi `Did` (`:147` — `Rotate` đổi `new_controller_pkh` /
`new_hw_pubkey`, không đụng chuỗi DID). Lineage cũ **không đóng băng**.

⇒ **Ràng buộc lên trait `KeyRegistry`**, ghi ngay dù resolver chưa cắm:

> `resolve(did, at_ts)` **PHẢI** trả khoá công khai **có hiệu lực tại `at_ts`**, không
> phải khoá hiện hành. Bản cài chỉ giữ khoá mới nhất là **không hợp lệ** — nó chạy đúng
> tới đúng lần xoay đầu tiên, rồi **mọi version ký bằng khoá cũ verify hỏng**, và hỏng
> lúc có người đi kiểm một proof cũ chứ không lúc xoay.

`InMemoryRegistry` hôm nay giữ **một** khoá mỗi `Did` và **không** nhận `at_ts`. Stub đó
hợp lệ trong giai đoạn này vì chưa có lượt xoay nào; điều kiện thu hồi gắn với **lượt
xoay đầu tiên**, không gắn với lịch.

**(3)** Proof/field phải cho verifier biết **chế độ** — đã dựng ở §17.3.

### 17.5 ❓ Câu 3 của `canonicalize(DID)` — thư PhoenixKey ĐÃ VỀ

Thư về từ **2026-07-30**:
`OriLifeTrace/OriLife-Core: _Agents/inbox/_done/Phoenix-reply-did-canonical-158-2026-07-30.md`,
ghim trong `MassTreeIdentify/core/test_strata_client.py` (`#159`).

- PhoenixKey grep `author_did`/`authorDid` **toàn kho** = **0 hit** ⇒ bên đó **không có
  khái niệm `author_did`** để cấp vector;
- phần 64-hex của `did:phoenix` sinh từ `random256` ⇒ **không tất định**, không có bảng
  *"input → DID"*;
- thứ thư **có** cấp: 2 DID thật + **khuôn chặt** + xác nhận **không tầng nào
  normalize/hạ-case** DID.

```
^did:phoenix:[a-z2-7]{13}:[0-9a-f]{64}$
```

🔺 **Trong khuôn đó, cả bốn câu treo KHÔNG quan sát được.** Đo: **20 000** DID sinh ngẫu
nhiên đúng khuôn qua `NFC`/`NFKC`/`NFD`/`NFKD` + hạ-thường ⇒ **0 chuỗi lệch byte**; `%`
và `#` **không lọt** khuôn. Đối chứng để phép đo không rỗng: DID **ngoài** khuôn
(`did:phoenix:nông-dân:sầu-riêng`) thì `NFC` vs `NFD` **lệch byte thật**.

⇒ Đề xuất (**không** phải chốt của kho này): thay quyết định chuẩn-hoá bằng một **cổng
khuôn**. Trong khuôn thì chốt thế nào cũng ra cùng `Did`; ngoài khuôn thì lệch quan sát
được ngay, và `ref_id = H_dom(author_did ‖ genesis_nonce)` một chiều nên **không có đường
lui**.

⚠️ **Chỗ không tự hoà giải:** anh Đức dẫn `did_hash` từ `phoenix_address.rs:52`; thư
PhoenixKey nói bên đó **không có** khái niệm `author_did`. Có thể `did_hash` là hàm nội
bộ dẫn **địa chỉ**, không phải đại lượng Strata gọi là `Did`. Nếu **cùng tên khác nghĩa**
thì chỗ nó lộ ra là `424`, không phải lỗi build. Câu cần đóng: **`did_hash` ở
`phoenix_address.rs:52` có cùng đại lượng với `Did` của Strata không?**

### 17.6 🟡 Gói bàn giao — `scripts/orilife_handshake.py`

Python 3.9+ **stdlib thuần**, ba bước, dừng ở bước đầu tiên hỏng.

```
python3 scripts/orilife_handshake.py --did did:phoenix:… --pubkey <hex64> \
    [--url http://127.0.0.1:6690] [--canonical-core <hex bản mình dựng>]
```

| Bước | Làm gì |
|---|---|
| 1 | **cổng khuôn** DID + báo đúng dạng chuẩn-hoá nào đổi byte |
| 2 | dẫn `author_did`, in sẵn dòng `STRATA_NODE_KEYS` (`did_hex32:pubkey_hex32`) |
| 3 | `POST /_canonical`, so **BYTE** với bản bên gọi tự dựng; lệch thì chỉ **offset đầu tiên** + cửa sổ ±16 B |

Đã chạy thật cả bốn nhánh trong phiên: đúng khuôn (sạch) · ngoài khuôn (kêu đúng
`NFD`/`NFKD`) · `canonical_core` khớp (**148 B**, đúng `148 + len(content_cid)`) · lệch
**1 nibble** (chỉ ra **offset 50**).

🪤 Lượt thử đầu của nhánh *"khớp"* lại báo **LỆCH**, dài `155 B` thay vì `148 B` — không
phải kịch bản sai mà **mẫu trích quá rộng** (`grep canonical_core` bắt luôn dòng tiêu đề).
Neo mẫu thì đúng ngay. Cùng họ với chỗ grep vào thư mục không tồn tại ở §17.1; luật ở
`Core: docs/VEDATA-ANCHOR-RUNBOOK.md §11.3`.

🔺 **Dữ kiện xếp thứ tự, đo được:** `_canonical` **không** tra key-registry — daemon với
**0 khoá** vẫn trả `200`. Nên hai mảnh bàn giao **độc lập**: OriLife khớp byte layout
**trước khi** bảng khoá trao xong. Xếp nối đuôi là tự thêm một tuần chờ.

🔺 **Xin chuỗi DID, KHÔNG xin băm.** `STRATA_NODE_KEYS` nhận `did_hex32`, nhưng nếu
OriLife gửi thẳng băm thì phép dẫn xuất **không kiểm được** — hàm một chiều, sai thì nằm
im tới `424`. Xin cả hai ⇒ băm thành **tổng kiểm** cho chuỗi.

| Cột | Ai điền | Vì sao |
|---|---|---|
| `did` chuỗi đầy đủ | OriLife | thứ **duy nhất** kiểm được; qua cổng khuôn §17.5 |
| `pubkey_hex` Ed25519 32 B | OriLife | vào registry |
| `author_did_hex` | OriLife | **tổng kiểm** — VeData dẫn lại và so, lệch ⇒ dừng |
| ghi chú | OriLife | người/thiết bị giữ khoá, để lượt xoay sau truy được |

**Một đối chiếu ngoài dự tính:** `author_did` kịch bản dẫn ra cho
`did:phoenix:nông-dân:sầu-riêng` (NFC) là `cd70bf3c01bc4f7ba3ceb09513938ae4067d2942…` —
**trùng khít** vector `V3` đông lạnh trong `test_strata_client.py` của `OriLife-Core`.
Hai bản cài độc lập, hai ngôn ngữ, cùng một số. Đường dẫn xuất `Did` đã khớp trước khi
ai nối dây.

### 17.7 ✅ Lượt bắt tay kỹ thuật ĐÃ MỞ — `OriLife-Core#450`

`#72` **MERGED** vào `main` (`3dc0d68`) trước khi mở thư, để `scripts/orilife_handshake.py`
và `apis/canonical-core-vectors.json` lấy được từ `main` chứ không từ một nhánh có thể
biến mất. Gác trên `main` sau merge: **264 pass / 0 fail** · `fmt` sạch · `clippy` **0**.

**Luận điểm của thư:** lượt này **không đốt một quyết định một chiều nào** — không tạo
`ref_id`, không ký, không ghi lineage. Sai thì chạy lại. Nên nó **không phải chờ** hai
chỗ còn treo.

**Thứ tự chạy hai lượt, và vì sao thứ tự đó có lý do:**

| Lượt | `state_fields` | Đo cái gì | Cần gì |
|---|---|---|---|
| 1 | **RỖNG** | thuần **byte-layout TLV** — `state_root` = 32 byte `00` (`S1-empty`), **không phép băm nào** | không `blake3`, không `PinnedHasher` |
| 2 | có trường | tới `state_root` | `blake3` |

Tách được *"layout của mình sai"* khỏi *"`state_root` của mình sai"* — **hai lỗi cho cùng
một triệu chứng `403 BadSignature`**, và `403` không nhắc một chữ nào tới `state_root`.

**Ba phát hiện gửi kèm, đều đo được:**

1. 🔺 **Thứ họ đang xin thì đã có.** Header `strata_client.py` còn ghi *"đang xin Strata
   một vector `state_root`"* để khoá cách mã hoá `content_cid`. Vector `S6-cid-value-32B`
   vào fixture từ **2026-08-13** (`0be9452`), ghi thẳng *"câu trả lời cho `OriLife-Core#161"*;
   `#161` cũng đã đóng **08-15**. Chú thích của họ **cũ hơn** thứ họ cần.
2. 🔺 **Bảng khoá chỉ cần MỘT dòng.** Anh Đức chốt ở `OriLife-Core#151` (**08-07**): server
   **không giữ khoá nông dân** (PhoenixKey sinh trắc, on-device) ⇒ **khoá ký = nền tảng
   (notary)**, `author_did` mức platform; quyền sở hữu đi qua **`owner_did` = state-field
   CÓ KÝ**. Đây là chỗ em suýt báo nhầm thành "đang chặn" — nó đã chốt từ 7 tuần trước.
3. 🔺 **`#71` không trừu tượng — nó ngồi đúng dưới lời khai sở hữu.** `owner_did` là
   state-field ⇒ **field-proof trên `owner_did` chính là thứ chứng minh quyền sở hữu từng
   nông dân với bên thứ ba**. Với tag dùng chung, người ghi cam kết `V` rồi khai một
   `owner_did` **khác** mà proof vẫn xanh. Vá xong **trước** khi tồn tại lineage thật nào.

**Nghiệm thu đề nghị — ba dòng:** `canonical_core` rỗng trùng byte · `canonical_core` có
trường trùng byte · `author_did` hai bên trùng. Dòng thứ ba **đã có sẵn một điểm đối
chiếu**: `cd70bf3c…` khớp vector `V3` đông lạnh của họ.

**Hai chỗ chặn `lineage` thật, nói thẳng trong thư:** `did_hash` ↔ `Did` chưa đóng
(§17.5) · `owner` chưa truyền xuống `try_shadow_write` ⇒ `owner_did` **chưa tồn tại trong
bản ghi**, nối bây giờ thì các version đầu không mang chủ sở hữu.

**Nêu kèm, không thuộc phạm vi thư:** `OriLife-Core#276` (ghi `confirmed` ngay lúc node
nhận tx — đo lệch **108 giây**) và `#423` (`strata_doi_chieu` kết luận `khop` khi **không
có mảnh bằng chứng Cardano nào**). Cả hai làm **lời khai đầu ra** mạnh hơn bằng chứng —
chúng quyết *"nối xong thì mình nói được câu gì"*.

**Một câu hỏi mở gửi họ:** `genesis_nonce` bên họ dùng tag `LN/STRATA/extkey/v1`. Grep
toàn kho Strata (`spec/` `src/` `node/` `anchor-io/`) hôm nay: **tag đó không tồn tại**.
Hôm nay vô hại (`genesis_nonce` do họ tính, Strata không tính lại), nhưng nó là một tag
đặt trong **namespace của người khác** — ngày Strata định nghĩa đúng chuỗi ấy theo nghĩa
khác thì thành **một tên, hai định nghĩa**, và chỗ lộ ra là `ref_id` lệch, không phải lỗi
biên dịch. Không đề nghị đổi (đổi tag = đổi `ref_id`); hỏi có nên ghi nó vào bảng §2.1
như một mục dành cho bên tiêu thụ.

### 17.8 "Neo dữ liệu on-chain" — *dữ liệu* ở đây là gì

Bản đầy đủ: `VeDataIO/Core: docs/VEDATA-MOSAIC-STRATA-SEAM-REPORT.md §17.12`. Mục này
giữ phần một đội tích hợp cần, để đọc được mà không phải sang kho khác. Đã gửi
`OriLifeTrace/OriLife-Core#450`.

**Dữ liệu KHÔNG lên chuỗi.** Cái lên chuỗi là chuỗi **cam kết** — toàn hash. Một cây gói
trong **104 byte**: `StrataAnchor = ref_id(32) ‖ head_version_hash(32) ‖ mmr_root(32) ‖ seq(8)`
(`src/chain.rs:100`). Lô 08-25 mang 6 anchor ⇒ **624 byte** trên chuỗi. Không mã cây,
không tên người, không ảnh — thứ công khai vĩnh viễn là **hash**, không phải nội dung.

**Cái thang:**

```
một trường → fvh → state_root → version_hash (thứ được KÝ) → mmr_root
           → StrataAnchor (104 B) → Cardano nhãn 1234 → checkpoint root
```

Mỗi bậc một chiều. Dữ liệu thật ở `strata-node` (nhật ký) và Mirage (`content_cid`).

**Chứng minh được:** bản ghi **đã tồn tại trước** một slot · **chưa từng bị sửa** · một
trường có **giá trị X** (field-proof, không phải lộ trường khác).
**KHÔNG chứng minh:** giá trị đó **đúng sự thật**.

🔺 **Ranh giới, và nó là ranh giới giữa các module.** Neo on-chain biến một lời khai
thành lời khai **không chối được**, không biến nó thành lời khai **đúng** — và đó không
phải thiếu sót:

| | Là gì | Giữ việc gì |
|---|---|---|
| **Strata** | **hạ tầng LampNet**, KHÔNG phải module VeData (`MODULES.md §2.1`) | *"đã khai gì, thứ tự nào, ai ký"*; byte do Strata sinh, **Mosaic chỉ chở** |
| **Mosaic** | module VeData | lên L1: bất biến + mốc thời gian. `§1.4`: **KHÔNG** biết record content, **KHÔNG** tính `V(r)` |
| **Score** | module VeData | **Trách nhiệm DUY NHẤT: `V(r) ∈ [0,1]`** — đánh giá độ tin cậy |

`Rada`/`Stamp`/`Mosaic`/`Query` **đều** ghi *"tính `V(r)`"* vào ô **KHÔNG làm**; chỉ
`Score` nhận ⇒ ranh giới **có chủ đích trong spec**.

**Dính thẳng tới `#71`:** field-proof là **cách duy nhất** dữ liệu thật được đối chiếu
với thứ đã neo; lỗ chung-tag cho phép cam kết một đằng khai một nẻo mà proof vẫn xanh —
và theo `OriLife-Core#151`, `owner_did` đi **bằng chính đường đó**.
