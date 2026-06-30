# Strata — Interface Contract (nội bộ, dùng chung cho các file Feat/Math/Tech/API)

> File này KHÔNG phải spec công bố. Nó là "khế ước giao diện" để các file Strata-Feat/Math/Tech/API
> dùng **cùng tên, cùng ký hiệu, cùng invariant**. Mọi mâu thuẫn với file này là lỗi.
> (Strata-API.md = API public cho platform: signature crate thật + HTTP + adapter neo; nó cũng theo khế ước này.)
> Sau khi 3 file xong + audit sạch, có thể xoá file này.

## Tên gọi
- **Strata** = Evolving Content Record = "Hồ sơ Tiến hóa".
- **ref_id** — định danh ổn định `lnref1…` (bech32), opaque, sinh từ (DID người tạo ‖ nonce genesis).
  KHÔNG đổi qua các phiên bản. KHÔNG mã hóa loại/độ nhạy nội dung.
- **content_cid** — CID nội dung của một phiên bản (hash thuần BLAKE3, đi vào Mirage). Mã hóa nếu nhạy cảm.
- **version** (nút phiên bản), các trường canonical theo thứ tự:
  `{ seq:u64, prev_hash:H32, content_cid:Cid, state_root:H32, author_did:Did, policy_hash:H32, ts:u64, sig:Sig }`
- **version_hash** = `H_dom("LN/STRATA/ver/v1", canonical(core))` — **KHÔNG gồm sig** (xem CHỐT-1).
  `core` = các trường version TRỪ `sig`, theo đúng thứ tự canonical bên dưới.
- **sig** = `Ed25519_sign(sk_author, version_hash)`, **bắt buộc canonical (low-S)** để chống malleability.
- **mmr_root** = `H_dom("LN/STRATA/mmr/root/v1", u64_be(n) ‖ bag_of_peaks)` — **commit số lá `n`** (xem CHỐT-3).
- **anchor** (neo on-chain) = `(ref_id, head_version_hash, mmr_root, seq)` — 4 trường = 32+32+32+8 = **104 byte**.
  Lưu ý dùng từ: "cam kết lịch sử" = riêng `mmr_root` 32 byte; "anchor" = cả 4 trường 104 byte. KHÔNG viết "anchor 32 byte".

## CHỐT kỹ thuật (sau audit vòng 1 — mọi file phải theo)
- **CHỐT-1**: `version_hash` KHÔNG trộn `sig`. Tamper-evidence của sig đến từ yêu cầu chữ ký canonical Ed25519 over `version_hash`, không từ việc băm sig. Math KHÔNG được claim "đổi sig đổi version_hash".
- **CHỐT-2**: Bảng domain-tag dưới đây là DUY NHẤT. Cả Math lẫn Tech dùng đúng chuỗi tag này, không tự đặt tên khác.
- **CHỐT-3**: `mmr_root` commit `u64_be(n)` trước khi bag peaks (củng cố dup-leaf guard: hai dãy khác độ dài → root khác).
- **CHỐT-4**: `value_cid` của một trường state phải là **content_cid thuần** (`gen_content_cid`, không class byte, không doc_type) — nếu không sẽ leak loại qua field-proof (INV-E5/E6).
- **CHỐT-5**: `Did` lưu dạng `[u8;32]` (băm DID PhoenixKey). Verify chữ ký cần ánh xạ `Did → pubkey` qua key-registry của lampnet-join/PhoenixKey — nêu rõ phụ thuộc này, không giả định Did == pubkey.

## Hàm băm (thống nhất toàn bộ)
- Hash nền: **BLAKE3** 32 byte, viết `H32`.
- **Domain-separated**: `H_dom(tag, x) = BLAKE3(tag ‖ 0x00 ‖ x)`, `tag` là chuỗi ASCII trong bảng dưới.
- **RFC 6962 prefix**: leaf = `H(0x00 ‖ data)`, internal = `H(0x01 ‖ left ‖ right)`.
- **Dup-leaf guard (CVE-2012-2459)**: cấm nhân đôi leaf lẻ; số leaf lẻ xử lý theo MMR (carry), KHÔNG copy leaf cuối.

### Bảng domain-tag CHUẨN (CHỐT-2 — copy nguyên văn)
| Mục đích | tag |
|---|---|
| Sinh ref_id | `LN/STRATA/ref/v1` |
| Băm version (core) | `LN/STRATA/ver/v1` |
| Policy commitment (tập author) | `LN/STRATA/policy/v1` |
| MMR leaf | `LN/STRATA/mmr/leaf/v1` |
| MMR internal node | `LN/STRATA/mmr/node/v1` |
| MMR root (bag + n) | `LN/STRATA/mmr/root/v1` |
| State: băm giá trị trường | `LN/STRATA/state/fval/v1` |
| State: leaf (key+fval) | `LN/STRATA/state/leaf/v1` |
| State: internal node | `LN/STRATA/state/node/v1` |
| State: padding (giấu số trường) | `LN/STRATA/state/pad/v1` |
| Batch entry (sub-MMR gộp lô) | `LN/STRATA/entry/v1` |
| Merkle Sum Tree leaf (tabular aggregate) | `LN/STRATA/sum/leaf/v1` |
| Merkle Sum Tree internal node | `LN/STRATA/sum/node/v1` |

### Mã hóa state leaf (CHỐT-4 — Math & Tech giống hệt)
```
fvh_i  = H_dom("LN/STRATA/state/fval/v1", field_value_bytes)   // field_value_bytes = giá trị inline HOẶC content_cid thuần (32B)
leaf_i = H_dom("LN/STRATA/state/leaf/v1", u32_be(len(field_key)) ‖ field_key ‖ fvh_i)
node   = H_dom("LN/STRATA/state/node/v1", left ‖ right)
state_root = node-root trên các leaf_i đã sắp theo field_key tăng dần
```

## 4 loại dữ liệu (MECE — theo quan hệ định danh↔nội dung qua thời gian)
1. **Tĩnh** (Static/write-once): 1 ID ↔ 1 nội dung cố định. VD: video, ảnh, PDF, release.
2. **Chuỗi-thêm** (Append-only): chuỗi mục, chỉ thêm cuối, mục cũ bất biến. VD: commit Gitlamp, log IoT, comment, sự kiện like/share.
3. **Thanh-ghi** (Mutable register): chỉ giá trị mới nhất, ghi đè; lịch sử để audit. VD: giá tài sản, số dư ví, nhiệt độ hiện tại.
4. **Hồ sơ cấu trúc** (Structured evolving): nhiều trường, cập nhật từng phần theo quyền. VD: học bạ, sổ bệnh, hồ sơ DID.

Ghi chú bắt buộc nêu trong cả 3 file:
- Đếm view/like = **register (#3) materialize từ append-log (#2)**, không phải loại riêng.
- Một primitive Strata phục vụ cả 4: #1 = Strata 1 version; #2 = MMR chính là log; #3 = đọc head; #4 = state_root field-level + policy.

## Invariant (đánh số INV-E*, cả 3 file phải dùng đúng số này)
- **INV-E1** (hash-linked): version seq=k có `prev_hash == version_hash(seq=k-1)`. seq=0: `prev_hash = 0^32`.
- **INV-E2** (đơn điệu seq): seq tăng đúng +1, không nhảy, không lùi.
- **INV-E3** (append-only history): thêm version chỉ MỞ RỘNG mmr; mọi inclusion-proof cũ vẫn đúng dưới root mới.
- **INV-E4** (quyền + chữ ký): `sig` hợp lệ bởi khóa của `author_did`, và `author_did` được `policy_hash` cho phép sửa phần tương ứng.
- **INV-E5** (CID không lộ loại): `ref_id`/`content_cid` là hash thuần; KHÔNG nhúng nhãn loại/độ nhạy (sửa lỗi leak Vault/Bulk hiện tại).
- **INV-E6** (field-privacy): proof một trường từ `state_root` KHÔNG tiết lộ trường khác.
- **INV-E7** (chống rollback): anchor on-chain đơn điệu theo `seq`; không thể neo lại version cũ.
- **INV-E8** (hashing an toàn): mọi cây dùng domain-sep + RFC6962 prefix + dup-leaf guard.
- **INV-E9** (bảo mật nhạy cảm): nội dung + state nhạy cảm được mã hóa (AES-256-GCM, khóa qua Argon2id/threshold); chỉ commitment hash công khai. Tách bạch "mã hóa" và "tái phân tán" — Strata yêu cầu CẢ HAI cho dữ liệu nhạy cảm.

## So sánh bắt buộc nêu (mỗi file ở mức phù hợp)
- **CIP-68**: anchor/cập nhật on-chain OK, nhưng (a) lịch sử không có proof gọn, (b) datum nhỏ/đắt/lộ hết, (c) không append-only nội tại. Strata: nội dung off-chain, cam kết on-chain 32-byte (mmr_root; anchor 104-byte), MMR cho proof O(log n), field-privacy.
- **git**: hash-linked DAG bất biến, nhưng inclusion proof = cả path; không neo on-chain/không finality kinh tế; branch (mutable ref) không tamper-evident. Strata: MMR compact proof + anchor on-chain ký.

## Lỗi hệ thống cũ Strata khắc phục (nêu nhất quán)
- Merkle dup-leaf (CVE-2012-2459) — dùng guard.
- Second-preimage / leaf-vs-node — RFC6962 prefix.
- Rollback/tua lùi version — seq + anchor đơn điệu (INV-E7).
- Mutable-ref tamper (git branch) — head ký + neo on-chain.
- CIP-68 size/cost/leak — off-chain content + commitment 32-byte.
- CID leak loại — INV-E5.

## Chi tiết cần xử lý (đặc biệt ở Math/Tech)
- **Gộp lô tần suất cao**: register/IoT cập nhật mỗi giây → 1 version = N entry qua sub-MMR theo epoch (checkpoint), tránh đẻ version vô hạn. Nêu CRDT là lựa chọn cho register hội tụ.
- **Đọc "giá trị tại thời điểm t"**: proof MMR tới version có ts ≤ t.
- **Quan hệ với DataClass hiện tại {Vault, Bulk}**: Strata là tầng TRÊN; mỗi version's content vẫn lưu qua Mirage theo lớp bảo mật, nhưng loại nằm trong state đã commit, không trong định danh.
