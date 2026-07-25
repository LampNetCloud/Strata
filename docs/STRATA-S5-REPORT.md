# Strata S5 — Daemon + HTTP API §3 (`lampnet-strata-node`)

> **Repo:** `LampNetCloud/Strata` · **Branch:** `thinh/strata-s5-node-http` · **Ngày:** 2026-07-22
> **Spec nguồn:** `spec/Strata-API.md` §0 (mô hình 3 lớp) · §1 (kiểu nền) · §2 (API Rust) · §3 (HTTP + bảng lỗi §3.1) · §4 (adapter neo) · §8.4 (ranh giới liên-module)
> **Trạng thái:** ✅ **CODE-COMPLETE** — 163 test workspace xanh (17 mới), clippy 0 warning, fmt clean, **daemon đã chạy thật + smoke-test bằng `curl` 8/8**

---

## 1. Vì sao S5

Hiện mảnh còn thiếu **lớn nhất** là lớp giữa: `Strata-API.md §0` mô tả ba lớp

```
Platform  ──HTTP JSON (§3)──▶  daemon  ──Rust API (§2)──▶  lampnet-strata core (THUẦN)
```

nhưng repo mới chỉ có **lớp dưới** (crate lõi) + `lampnet-anchor-io` (I/O neo). **Lớp daemon
chưa có dòng nào** — nghĩa là platform (ProofChat/OriLife/AladinWork) chưa có cửa nào để gọi
Strata. Đây cũng chính là mục "còn: ráp daemon thật + HTTP §3" ghi ở milestone S2.

§3 đã đặc tả đủ 8 route + bảng lỗi §3.1, §2 đã cố định signature Rust thật ⇒ việc này là
**dev thuần**, không phải việc spec.

---

## 2. Ranh giới giữ nguyên (§0 + §8.4)

- Core `lampnet-strata` **KHÔNG** chạm file/mạng — S5 không thêm một dòng I/O nào vào crate lõi.
- Daemon **KHÔNG** tự băm / tự dựng MMR — `state_root`, `version_hash`, `mmr_root`, mọi proof
  đều gọi xuống core. Nếu thấy mình sắp băm cái gì trong `node/` thì đó là dấu hiệu code đang
  lấn xuống core.
- Daemon **KHÔNG ký hộ ai**: chữ ký do client gửi kèm; daemon gắn vào version rồi để
  `genesis`/`append_version` của core `verify_strict`. Không có đường ghi nào bỏ qua sig.
- Neo on-chain đi qua trait `AnchorSink` (§4.1, đã có sẵn ở `src/anchor_sink.rs`) — daemon
  không dựng tx Cardano, và S5 **không viết backend mới**.

---

## 3. Đã làm

Crate mới `node/` = **`lampnet-strata-node`** (workspace member thứ ba, sau lõi và `anchor-io`).

| File | Vai trò |
|---|---|
| `node/src/hexs.rs` | Hex CHỈ ở rìa DTO (§3 "hash/CID truyền hex"): `decode_fixed<N>`/`decode_var` + serde `h32`/`h64`/`hvar`. Sai độ dài ⇒ 400, không pad không truncate. |
| `node/src/error.rs` | **Bảng §3.1**: đủ **11** biến thể `StrataError` → HTTP code + tên lỗi nguyên văn, cộng 6 lỗi **mức-cửa** của daemon. Body `{ "error": …, "detail": {…} }`. |
| `node/src/registry.rs` | `trait KeyRegistry` (`Did → VerifyingKey`, **CHỐT-5**) + `InMemoryRegistry`. Không phân giải được ⇒ `UnknownAuthor` → **424**, fail-closed. |
| `node/src/store.rs` | `ChainStore` (ref → entry, **khoá riêng theo từng ref**) + `ChainEntry {chain, policy, fields theo seq, audit, anchored}`. |
| `node/src/dto.rs` | Request/response khớp từng trường §3 + `ProofDto`/`VersionDto`/`FieldProofResp` + `PriorityDto` (4-enum = enum Stamp). |
| `node/src/anchor.rs` | Cắm `AnchorSink`: `DisabledSink` (mặc định → 501), `MemorySink`, `FailingSink` (cho test). |
| `node/src/routes.rs` | Router + 8 handler §3. |
| `node/src/bin/strata_node.rs` | Binary: `STRATA_NODE_ADDR` + `STRATA_NODE_KEYS` (nạp **chỉ pubkey**). |
| `node/examples/dev_client.rs` | Sinh khoá dev + body `create` **đã ký** để thử bằng `curl`. |
| `node/tests/http.rs` | 16 test đầu-cuối qua router thật. |

### 3.1 Bảng route (§3 ↔ §2 ↔ code)

| HTTP §3 | Rust §2 | Ghi chú cài đặt |
|---|---|---|
| `POST /v1/strata/create` | `StrataChain::genesis` | ref_id = `gen_ref_id_raw(author_did, genesis_nonce)`; trả bech32m `lnref1…` |
| `POST /v1/strata/:ref/version` | `chain.append_version` | `prev_seq` lệch head ⇒ `SeqNotMonotonic` (422) TRƯỚC khi dựng version |
| `POST /v1/strata/:ref/event` | `append_version` (kind=version) **hoặc** `AuditLog::append_access` (kind=audit) | phân nhánh bằng `kind` (§2.6 hai cách) |
| `GET /v1/strata/:ref/head` | `chain.head()` + `mmr_root()` | |
| `GET /v1/strata/:ref/version?at=` | `chain.version_at(t)` | trả kèm inclusion-proof; `t < ts(genesis)` ⇒ 404 |
| `GET /v1/strata/:ref/proof/version/:seq` | `chain.prove_version(seq)` | |
| `GET /v1/strata/:ref/proof/field/:key` | `state::prove_field` | thêm `?seq=` tuỳ chọn để tra **lịch sử**; vắng ⇒ head |
| `POST /v1/strata/:ref/anchor` | `chain.publish_anchor()` + `AnchorSink::publish` | thứ tự đặc biệt — xem §4 |

`:ref` nhận cả bech32m `lnref1…` (dạng §2.1 trả ra) lẫn hex 64 ký tự (tiện debug).

---

## 4. Một quyết định thiết kế đáng ghi: thứ tự neo

`StrataChain::publish_anchor()` **cập nhật `last_anchor_seq` ngay khi trả `Ok`**. Nếu daemon
gọi nó *trước* rồi mới đẩy on-chain, và backend hỏng, thì:

- on-chain **không có gì**,
- nhưng lõi đã coi seq đó là "đã neo" ⇒ mọi lần thử lại đều `AnchorRollback` ⇒ **ref chết vĩnh viễn**.

Nên trình tự cài là **kiểm → đẩy → chốt**, cả ba nằm trong khoá của riêng ref:

1. kiểm rollback bằng **gương** `anchored.seq` phía daemon (chưa đụng lõi);
2. `AnchorSink::publish` đẩy on-chain;
3. thành công mới `chain.publish_anchor()` để chốt INV-E7 ở lõi.

Có test riêng cho đúng cái bẫy này (`failing_backend_does_not_burn_the_ref`): với sink luôn
hỏng mạng, gọi neo **hai lần** phải ra **503 cả hai lần** — không được lần hai biến thành 409.

`priority = no_anchor` cũng KHÔNG chốt seq (§4.2: sống ở tầng a/b), nên neo thật ngay sau đó
vẫn đi được — cũng có test.

---

## 5. Kiểm chứng

### 5.1 Test

```
cargo test --workspace   → 163 pass / 0 fail (1 ignored, pre-existing)
   trong đó MỚI: node lib 1 + node HTTP 16 = 17
cargo clippy --workspace --all-targets → 0 warning
cargo fmt -p lampnet-strata-node --check → clean
```

16 test HTTP chia ba nhóm:

- **Đường sống (2):** create → version → head → `proof/version` → `proof/field` → `version?at`.
  Proof lấy về được **verify THẬT** bằng chính `StrataChain::verify_version` /
  `state::verify_field_proof` của core (không chỉ "có trả JSON"); `proof/field?seq=0` trả đúng
  giá trị **cũ**; event audit không đẻ version còn event kind=version thì có.
- **Bảng lỗi §3.1 (8):** 424 `UnknownAuthor` · 403 `PolicyHashMismatch` / `BadSignature` /
  `PolicyDenied` · 422 `SeqNotMonotonic` / `TimestampRegress` · 409 `RefExists` · 404 ref/seq/key
  · 400 `MalformedRequest` (JSON hỏng, thiếu `at`, ref sai dạng).
- **Neo (4):** happy → rollback 409 → có version mới thì neo lại được · `no_anchor` txid null và
  không nuốt seq · backend hỏng không cháy ref · chưa cắm backend thì 501 nhưng `no_anchor` vẫn 200.

### 5.2 Chạy thật — smoke test `curl` (daemon nghe cổng 6690)

```
$ cargo run -p lampnet-strata-node --example dev_client     # sinh khoá dev + body đã ký
$ STRATA_NODE_KEYS=1111…11:0d7550754e0800a5d237eef5826035766b9b3e5a15868a940ab289958788e3b0 \
  cargo run -p lampnet-strata-node --bin strata-node
strata-node nghe tại http://127.0.0.1:6690 — route §3 dưới /v1/strata, 1 khoá trong registry
```

| # | Lệnh | Kết quả |
|---|---|---|
| 1 | `POST /v1/strata/create` | **200** — `ref_id=lnref1k28n0y3n9hc05d3alnxq7ev58md0klm80e20zczfh0pcveuznncqlt0zcj`, `head_version_hash=8b370ff3…f811f0`, `mmr_root=a20e7818…ac2a8a` |
| 2 | `GET …/head` | **200** — `head_seq=0`, `content_cid=cafe`, mmr_root khớp #1 |
| 3 | `GET …/proof/version/0` | **200** — `mmr_size=1`, `siblings=[]`, `peaks=[9689099a…]` |
| 4 | `GET …/proof/field/diagnosis` | **200** — `fvh=180b83fc…`, `state_root=51800634…672745`, `version_seq=0` |
| 5 | `GET …/version?at=1700000001` | **200** — trả version 0 đầy đủ + proof |
| 6 | `POST …/anchor {"priority":"no_anchor"}` | **200** — `anchor_txid=null`, `backend=null` |
| 7 | `POST …/anchor {"priority":"immediate"}` | **501** `AnchorNotConfigured` (bin mặc định `DisabledSink`) |
| 8 | `POST /create` lần 2 cùng nonce | **409** `RefExists` |

---

## 6. Điểm §3 chưa phủ — cách xử lý + **câu hỏi treo anh Đức**

Ba chỗ §3 không nói, đã chọn phương án fail-closed và ghi lại đây để anh chốt (chưa đụng
byte-layout nào đã neo, nên đổi sau vẫn rẻ):

1. **Tập author của policy lúc `create`.** §3 bắt gửi `policy_hash` nhưng không nói *thành viên*
   policy lấy từ đâu. Đã làm: thêm trường **tuỳ chọn** `policy_authors: ["<did hex>", …]`; vắng ⇒
   policy **một-thành-viên** = người tạo. Mọi DID phải phân giải được qua key-registry (CHỐT-5),
   và `policy_hash` client gửi phải khớp policy dựng ra, lệch ⇒ 403. → *Anh muốn giữ dạng này,
   hay policy là cấu hình phía daemon (cửa admin riêng)?*
2. **Chữ ký của `event kind=audit`.** `AuditEntry` không có trường `sig` (chữ ký **không** vào
   leaf ⇒ không cam kết byte-layout nào), nhưng §3 vẫn bắt gửi `sig`. Đã làm: kiểm ở **cửa** —
   `verify_strict` Ed25519 trên `entry.canonical()`, khoá từ key-registry. Không đặt domain-tag
   mới (bảng CHỐT-2 là DUY NHẤT). → *Xác nhận thông điệp ký là `canonical()` chứ không phải
   `leaf_bytes()`?*
3. **6 lỗi mức-cửa ngoài §3.1**: `MalformedRequest` 400 · `NotFound` 404 · `RefExists` 409 ·
   `AnchorNotConfigured` 501 · `AnchorRejected` 502 · `AnchorNetwork` 503. Tên tách hẳn khỏi tên
   biến thể lõi để client không lẫn. Riêng `AnchorError::RollbackAttempt` của backend được **gộp
   về** `AnchorRollback` (409) vì đúng nghĩa là INV-E7, không phải lỗi hạ tầng.

---

## 7. Chưa làm (ranh giới của S5)

- **Bền vững**: store hiện in-memory. Đường persist (Mirage cho blob theo §8.4 + đĩa cho log)
  là milestone sau, cắm qua chính `ChainStore` — mất tiến trình là mất state, ghi rõ để không ai
  tưởng nhầm đây là bản production.
- **Cắm backend neo thật** vào binary (`MosaicAnchorSink` / `SettlementSink`): cần khoá + endpoint
  chuỗi, thuộc việc vận hành; interface thì đã sẵn, đổi một dòng `Arc::new(...)` ở `main`.
- **Ghép S3/S4 vào daemon**: vòng gộp epoch (`EpochAccumulator` → checkpoint → 1 version) và
  composite/tabular chưa có route — §3 cũng chưa đặc tả route cho chúng.
- **`DerivedIndex` (S2)** chưa gắn vào đường đọc: các route hiện quét thẳng chain (đúng kết quả,
  chưa tối ưu). §7.5 cấm index ghi ngược nên việc gắn là thuần tăng tốc đọc.

---

## 8. Land lên main (2026-07-25)

Nhánh viết xong 07-22, main đi tiếp 4 lần trong lúc chờ (PR #17 test regression flood, #20 gom
docs, #22 dọn audit #16). Trước khi merge đã rebase lên `1b18c6d` và chạy lại toàn bộ.

**Conflict GitHub báo là hình thức.** `mergeable_state: dirty` chỉ vì PR #20 dời
`STRATA-ROADMAP.md` → `docs/` sau khi nhánh này tách ra. Rebase local nhận rename, không có một
hunk conflict nào.

**Kiểm chứng lại trên base mới:**

| Hạng mục | Kết quả |
|---|---|
| `cargo test --workspace` | **164 pass / 0 fail** (16 test HTTP của node giữ nguyên) |
| `cargo clippy --workspace --all-targets` | **0 warning** |
| `cargo fmt -p lampnet-strata-node --check` | clean |

Repo không có workflow CI nào, nên "no checks reported" trên PR là bình thường — kiểm chứng
hoàn toàn dựa vào chạy local, ghi rõ để lần sau không ai đọc nhầm thành CI xanh.

**Ba điểm dọn trước khi land:**

1. `STRATA-S5-REPORT.md` → `docs/` cho khớp convention PR #20 (báo cáo viết trước lần dời đó).
2. Gỡ `log = "0.4"` khỏi `node/Cargo.toml` — khai mà không có một dòng `log::` nào.
3. `proof_field` nhận `Query<SeqQuery>` **trần**, nên `?seq=<không phải số>` rơi về format lỗi mặc
   định của axum — lệch đúng mục tiêu "client chỉ phải hiểu MỘT format" mà chính báo cáo này đặt
   ra. Đã bọc `Result` như `version_at`, thêm assertion vào
   `malformed_inputs_are_400_in_our_error_format`.

**Câu hỏi §6.2 (thông điệp ký của `event kind=audit`) tách thành issue #23.** Lý do tách riêng
khỏi hai câu còn lại: §6.1 và §6.3 là lựa chọn code-side, sai thì sửa, không ai ở ngoài phụ
thuộc. §6.2 thì khác — chữ ký audit **không đi vào leaf**, nên không root nào cam kết byte-layout
đã ký; client ngoài phải tự đoán, và nếu sau này chốt khác thì chữ ký cũ chết **im lặng**, không
test nào bắt được. Đây đúng hạng lỗi `stamp_id` lệch encoder (VeDataIO/Core#39), chỉ khác là lộ
ra trước khi có client thứ hai.
