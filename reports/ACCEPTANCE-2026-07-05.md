# Nghiệm thu end-to-end Strata — T1–T6 (OriLife Strata-Neo-BuildRequest §3)

- Ngày chạy: đêm 2026-07-05 → rạng 2026-07-06 (giờ VN). Mạng: **Cardano Preview** (anchor THẬT).
- Daemon: binary `lampnet-node` release (lampnet-hivemind, HTTP 127.0.0.1:6499, `LAMPNET_DEV_MODE=true`), WAL sạch từ đầu.
- Client nghiệm thu: Rust độc lập (`acc-client`), ký Ed25519 thật; verifier KHÔNG tin daemon — tự dựng lại
  `version_hash`/`state_root` bằng crate `lampnet-strata` và tự leo cây proof.
- Evidence thô: `reports/evidence-2026-07-05/` (log + JSON nguyên văn từng bước).
- Nguyên tắc: không kết luận nào thiếu output thật. Mọi trích dẫn dưới đây copy từ evidence.

## 0. Phần I — nối AnchorSink vào route anchor (diff tối thiểu)

Repo `lampnet-hivemind`, 3 file (KHÔNG commit, KHÔNG push):

| File | Thay đổi |
|---|---|
| `lampnet-mirage/Cargo.toml` | +2 dòng: dep `lampnet-anchor-sink = { path = "<worktree Strata>/anchor-sink" }` |
| `lampnet-mirage/src/strata_routes.rs` | Bỏ trait placeholder → dùng crate thật; `StrataState` thêm `anchor_sink` (tiêm được, test) + `AnchoredLog` persist `<dir>/anchored.log` cạnh WAL; `h_anchor` cài thật; `env_settlement_sink()` + `map_anchor_err()` |
| `lampnet-mirage/tests/strata_http.rs` | +2 test mock sink: idempotent/no_anchor/tx-mới-sau-version + bảng lỗi anchor |

Luồng `POST /v1/strata/:ref/anchor` (giữ đúng 2 lớp INV-E7 §8.1b):
1. Snapshot `chain.anchor()` dưới lock (KHÔNG giữ lock qua network).
2. `SettlementSink::publish` trong `spawn_blocking` — sink tự kiểm idempotency on-chain:
   `on_chain_seq == seq` → `Ok(None)` (200, `anchor_txid:null`, không tx mới); `>` → 409 `AnchorRollback`.
3. Có tx mới → core `publish_anchor()` cập nhật `last_anchor_seq` + ghi `AnchoredLog` (persist).

Ánh xạ lỗi: `RollbackAttempt`→409, `NotConfigured`→501 (thiếu env → giữ hành vi 501 như trước),
`Network`→502, `Rejected`→422, `DatumTooLarge`→413, `InsufficientAda`→402.

Env cấu hình: `BLOCKFROST_TOKEN_GREENSUN`, `LAMPNET_ANCHOR_PUBLISHER`,
`LAMPNET_ANCHOR_SUBMITTER_DIR`, `LAMPNET_ANCHOR_SCAN_LIMIT` (tùy chọn, mặc định 200; phiên này 60),
`VEDATA_WALLET_MNEMONIC` (chỉ trong env process → child submitter; không log).

## 1. Bảng kết quả T1–T6

| Test | Nội dung | Trạng thái | Evidence chính |
|---|---|---|---|
| T1 | create(`tree-DT-061`) → ×3 version → head seq=3 → proof v2 verify offline | **PASS** | `t1.log` |
| T2 | version 5 trường → proof/field/species verify độc lập; proof KHÔNG lộ owner_did/gps | **PASS** | `t2.log`, `t2-proof-blob.json`, grep = 0 match |
| T3 | anchor THẬT Preview + Koios xác nhận độc lập + idempotent/rollback | **PASS** | tx `8a858746…`, `t3-*.json`, `t3-rollback.log` |
| T4 | replay nguyên body (cùng sig) bị chặn, head không đổi; anchor lại không tx mới | **PASS** | `t4.log`, `t4-anchor.json`, đếm on-chain = 1 tx |
| T5 | 20 event → 1 anchor tx cho cả lô; p50 POST < 200ms | **PASS** | tx `502676c3…`, `t5.log`: p50 = **0,19 ms** |
| T6 | blob 10KB thật → Mirage CID → version → head trả CID → tải ngược, sha256 khớp | **PASS** | `t6.log`, `t6-sha256.txt` |

## 2. Chi tiết + trích evidence

### T1 — vòng đời + proof version, verify offline

```
T1 create → 200 ref_id=lnref1tm7wa73556uany5r6yavrtgjdg4y73jyxh84g9vudgp30pdfsu8qy6m99g
T1 resolve(tree-DT-061) → {"ref_id":"lnref1tm7wa73…"}          (YC-1: cùng ref_id tất định)
T1 version seq=1/2/3 → 200
T1 head → head_seq=3, mmr_root=267f5129554fb6201cbb600ba0e56d718c442fe60986f5ebc414d80c0b235e92
T1 proof/version/2 → mmr_size=4, siblings ×2, peaks ×1
T1 verify offline (lampnet-strata verify_version, root=head.mmr_root): PASS
```
Verifier gọi thẳng `StrataChain::verify_version` của crate core (MMR BLAKE3) — không tin kết quả daemon.

### T2 — field-proof + riêng tư (INV-E6)

Version seq=4 với 5 trường: `owner_did`, `species`, `gps`, `harvest_date`, `note`.
`GET proof/field/species` → blob chỉ chứa: key `species`, value `durian-monthong` (hex),
`fvh`, 1 sibling hash, `state_root`.

- `verify_field_proof` (client tự leo cây từ leaf): **PASS**.
- `state_root` trong proof **==** `build_state_root(5 trường)` client tự tính khi ký: **PASS**.
- Grep bytes hex của giá trị trường khác trong TOÀN BỘ proof-blob:

```
owner_did hex: 6469643a70686f656e69783a616c6164696e2d6f776e65722d30303432 → 0 match
gps hex      : 31302e3736323632322c3130362e363630313732                   → 0 match
chuỗi thô 'aladin-owner'                                                  → 0 match
```

### T3 — anchor THẬT trên Preview + xác nhận độc lập

```
POST /v1/strata/lnref1tm7wa73…/anchor  (20,8s)
→ 200 { anchor_txid: "8a85874611053e08e4f74ed372d848b8972e84d824c8dbb1da512a9a6b493bfe",
        backend:"settlement", seq:4,
        mmr_root:"e867da2aab781ba0f869f9f63957cc59fafcca01b83b27961ee2da7770cca9fe" }
Blockfrost: confirmed sau ~10s — block_height 4442887, size 918B, fee 195 817 lovelace
```

Link: <https://preview.cardanoscan.io/transaction/8a85874611053e08e4f74ed372d848b8972e84d824c8dbb1da512a9a6b493bfe>

Xác nhận ĐỘC LẬP qua Koios (không đi qua Blockfrost):

```
curl -s -X POST "https://preview.koios.rest/api/v1/tx_metadata" \
  -H 'Content-Type: application/json' \
  -d '{"_tx_hashes":["8a85874611053e08e4f74ed372d848b8972e84d824c8dbb1da512a9a6b493bfe"]}'
→ metadata["1234"] = [{ "t":1, "a":[ 0x5efceefa…870e,  # ref_id  == anchored.log ✅
                                     0x80df8a02…45e1,  # head_vh == response ✅
                                     0xe867da2a…a9fe,  # mmr_root == response + anchored.log ✅
                                     4 ] }]            # seq ✅
```
(Koios trả hex kèm prefix `0x` — so sánh sau khi strip.)

Idempotent + rollback (INV-E7 lớp adapter, §8.1b):
- Anchor lại khi head không đổi → 200 `anchor_txid:null` (14,9s — resolve on-chain thấy seq 4 đã neo, KHÔNG build tx).
- Dựng anchor seq=1 (< on-chain 4), gọi sink trực tiếp với submitter-panic-nếu-bị-gọi:
  `RollbackAttempt { on_chain_seq: 4, attempted: 1 }` — chặn TRƯỚC khi build tx.

### T4 — chống replay + anchor idempotent

```
T4 replay body seq=4 (cùng sig) → 422 {"error":"SeqNotMonotonic","detail":{"expected":5,"got":4}}
T4 head không đổi (seq=4, hash=80df8a02…45e1) — PASS
T4b anchor lại → 200 anchor_txid:null (không tx mới)
T4c đếm on-chain: tx label-1234 chứa ref_id này = ['8a858746…'] → SỐ TX = 1
```

### T5 — 20 event / 1 anchor + p50

Chain mới `tree-DT-061-lot-2026` (`lnref1cerf82c…`), 20 event `kind=version` (§2.6 cách 1, state rỗng),
client tự dựng `prev_hash` cục bộ (không GET head giữa chừng) và đối chiếu `version_hash` daemon trả — 20/20 khớp.

```
latency 20 request (ms): 0.27 0.21 0.22 0.22 0.19 0.20 0.22 0.21 0.21 0.20
                         0.19 0.19 0.18 0.18 0.18 0.20 0.19 0.18 0.18 0.19
p50 = 0,19 ms | p95 = 0,22 ms | max = 0,27 ms   (yêu cầu p50 < 200 ms — PASS, dư ~1000×)
```
Lưu ý: đo trên loopback local, release build — không đại diện RTT mạng thật.

MỘT anchor duy nhất sau event 20:

```
→ 200 { anchor_txid:"502676c3f3c6152c06ea5079b8aeacfbc222a58ca3ade56b61cad2f967284588",
        seq:20, mmr_root:"a7ee4d9f…40af" }
Koios: mmr_root on-chain khớp: True | seq on-chain: 20
Đếm on-chain cho ref lô: SỐ TX = 1  (20 event → 1 tx)
```
Link: <https://preview.cardanoscan.io/transaction/502676c3f3c6152c06ea5079b8aeacfbc222a58ca3ade56b61cad2f967284588>

Đối chiếu quy tắc BatchPolicy (profile `proofchat`, panel 2026-07-04): `mmr_root` seq-20 cam kết CẢ 20 event
(MMR append-only) — một commitment/lô đúng tinh thần gom-lô. Daemon hiện neo theo lệnh chủ động
(`POST /:ref/anchor` sau event cuối), CHƯA tự flush theo `epoch_secs/max_entries/flush_max_age` — xem rủi ro §4.

### T6 — byte thật qua Mirage

```
sha256 gốc     : b3635e939d610243996ddea4d0fa4803888d6d7d8c1dacb47d6dc210a905722d  (10 240B urandom)
POST /mirage/put → CID = ln1q_25564a97509a32f4_orilife-test
T6 version seq=5 content_cid=hex(CID) → 200
GET head → content_cid decode == CID upload — PASS
GET /internal/fetch/<CID> → sha256 tải về: b3635e93…722d — KHỚP
```
Strata chỉ giữ CID thuần (INV-E5); blob sống bên Mirage của chính daemon.

## 3. Regression (Phần III)

| Crate | Kết quả | Ghi chú |
|---|---|---|
| `lampnet-strata` (worktree) | **83 pass** (64 unit + 7 + 12 integration), 0 fail | gồm module `batch.rs` mới |
| `lampnet-anchor-sink` | **28 pass** (15 unit + 13 integration), 0 fail, 1 ignored | ignored = test on-chain live (`onchain_preview.rs`, cần env + tADA; luồng live đã được nghiệm thu qua daemon ở T3/T5) |
| `lampnet-mirage` `strata_http` | **5 pass** (3 cũ + 2 anchor mới), 0 fail | server axum thật, sig thật |
| Build | `cargo build --release -p lampnet-mirage --bin lampnet-node` sạch | exit 0 |
| Clippy | 0 warning trong `strata_routes.rs` + `strata_http.rs` (phần mới) | warning còn lại thuộc code cũ (carpet…), không đụng |

## 4. Phí ADA thật

| Tx | Kích thước | Phí | Đối chiếu mô hình phí (panel: 0,155381 + 0,000044057×byte) |
|---|---|---|---|
| `8a858746…` (T3, 1 anchor) | 918 B | **0,195817 tADA** | 0,155381 + 44×919B ≈ 0,195817 — khớp |
| `502676c3…` (T5, 1 anchor/20 event) | 918 B | **0,195817 tADA** | khớp; ≈ **0,0098 tADA/event** khi gom lô 20 |

Tổng chi phí nghiệm thu: **0,391634 tADA** (2 tx; mỗi tx tự-trả 2 tADA về ví — không mất ngoài phí).
Số dư ví publisher sau nghiệm thu: **7 510,79 tADA**.

## 5. Rủi ro còn lại (tổng hợp 3 nhóm + nghiệm thu)

1. **CHỐT-5 chưa đóng**: `DevKeyRegistry` DEV-ONLY (bearer + `LAMPNET_DEV_MODE=true`). Production phải chờ
   key-registry PhoenixKey; không được bật DEV_MODE trên SuperNode thật.
2. **Race 2 request anchor đồng thời cùng ref**: idempotency dựa resolve tx ĐÃ confirm — trong cửa sổ
   ~20–60s trước confirm, 2 request song song có thể tạo 2 tx cùng seq (không phá dữ liệu — nội dung
   trùng nhau; chỉ tốn phí). Khắc phục đề xuất: mutex per-ref quanh publish ở daemon.
3. **Trust pin = ví VeData dùng chung** + `LAMPNET_ANCHOR_SCAN_LIMIT` hữu hạn: ví bận việc khác có thể đẩy
   anchor cũ ra ngoài cửa sổ quét → idempotency miss (tx thừa, không sai dữ liệu). Đề xuất: ví publisher
   riêng cho Strata, hoặc indexer off-chain thay quét trang.
4. **`last_anchor_seq` (lớp INV-E7 in-process) không persist qua WAL** — sau restart chỉ còn lớp on-chain
   bảo vệ (đúng thiết kế 2 lớp §8.1b, nhưng cần biết khi audit).
5. **Daemon chưa tự flush theo BatchPolicy (S3)**: `batch.rs` mới ở core; anchor hiện là endpoint chủ động.
   Việc tiếp: scheduler flush theo `epoch_secs=600 / max_entries=4096 / flush_max_age=180s`.
6. **Submitter phụ thuộc `npx tsx` + node_modules** (Node 24 local): production cần pin version + đóng gói;
   timeout child 180s. Mỗi lần anchor tốn ~15–21s (quét Blockfrost + build/submit) — không nằm trên
   đường nóng POST version (đã tách `spawn_blocking`).
7. **WAL lưu giá trị trường dạng hex trên đĩa daemon** — chưa mã hoá at-rest (ghi nhận từ nhóm A).
8. **Koios trả hex kèm `0x`** — SDK/indexer phía đọc phải strip prefix khi đối chiếu (đã xử lý trong
   client nghiệm thu; cần ghi vào tài liệu tích hợp OriLife).
9. **p50 0,19ms là số loopback** — nghiệm thu OriLife thực địa cần đo lại qua mạng thật (ngưỡng 200ms
   vẫn dư địa lớn).

## 6. Kết luận

6/6 test T1–T6 **PASS** với evidence thật; 2 anchor tx THẬT trên Cardano Preview được xác nhận độc lập qua
Koios (label 1234, CBOR raw, t=1, mmr_root khớp từng byte với response daemon và `anchored.log`).
Regression 83 + 28 + 5 test pass, build + clippy phần mới sạch. Chưa commit/push — chờ anh duyệt.
