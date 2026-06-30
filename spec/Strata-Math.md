# Strata — Mô hình & Toán (bản phổ thông)

**Module**: Strata — Evolving Content Record ("Hồ sơ Tiến hóa") — neo lịch sử nội dung bằng cấu trúc dữ liệu xác thực, sao cho lịch sử bất biến, chứng minh gọn, và giữ riêng tư từng trường.

> Bản phổ thông cho mọi người đọc (tech + nontech). Chứng minh đầy đủ, khai triển toán. Mọi ký hiệu, tên trường, invariant trong file này tuân theo `Specs/Strata/_CONTRACT.md` — mâu thuẫn là lỗi.

---

## Tóm tắt

Strata là một primitive duy nhất phục vụ mọi loại nội dung biến đổi theo thời gian trên LampNet. Một bản ghi Strata là một **chuỗi phiên bản nối bằng hash** (hash-linked version chain): mỗi version trỏ về version trước qua `prev_hash`, nên sửa bất kỳ version quá khứ nào sẽ làm hỏng toàn bộ version sau nó (hiệu ứng tuyết lở). Trên dãy phiên bản đó, Strata dựng một **Merkle Mountain Range (MMR)** — một dạng cây Merkle thiết kế cho dữ liệu chỉ-thêm — để vừa chứng minh "version k từng tồn tại" bằng một proof kích thước `O(log n)`, vừa bảo đảm thêm phiên bản mới không làm sai bất kỳ proof cũ nào (append-only).

Ngoài lịch sử, mỗi phiên bản còn có một **state_root** là cây Merkle trên từng trường dữ liệu, cho phép chứng minh giá trị của *một* trường mà không lộ các trường khác (field-privacy). Trạng thái mới nhất được "neo" (anchor) lên chuỗi bằng đúng 4 trường nhỏ `(ref_id, head_version_hash, mmr_root, seq)`; anchor đơn điệu theo `seq` nên không thể tua lùi về version cũ (chống rollback). Toàn bộ độ an toàn quy về độ khó tìm va chạm của BLAKE3 cộng với chữ ký Ed25519. So với CIP-68 (datum nhỏ, đắt, lộ hết, không có proof lịch sử gọn) và git (DAG bất biến nhưng proof = cả path, ref nhánh sửa được, không neo on-chain), Strata cho lịch sử tamper-evident có finality kinh tế với cam kết lịch sử on-chain chỉ 32 byte (`mmr_root`); cả anchor 4 trường là 104 byte.

---

## §0.5 Phạm vi module

Strata là **tầng TRÊN** lớp lưu trữ. Nó không tự lưu nội dung và không tự đo đóng góp. Nội dung mỗi phiên bản (`content_cid`) vẫn đi qua Mirage theo lớp bảo mật (Vault/Bulk); Strata chỉ giữ **các cam kết hash** (commitment) và cấu trúc chứng minh quanh chúng. Loại dữ liệu nằm trong *state đã commit*, **không** trong định danh (`ref_id`/`content_cid`) — đây là điểm sửa lỗi rò rỉ loại của hệ cũ.

| Lằn ranh | Nội dung |
|---|---|
| **Làm** | Định nghĩa `version_hash`, chuỗi hash-linked, MMR (`mmr_root`), `state_root` field-level; sinh inclusion-proof + temporal-proof + field-proof; định nghĩa `anchor` 4 trường; bất biến INV-E1..E9 |
| **Dùng** | Mirage (lưu `content_cid` theo lớp bảo mật); PhoenixKey (DID `author_did`, chữ ký Ed25519); on-chain (neo `anchor`); BLAKE3 làm hàm băm nền |
| **Không làm** | Không lưu nội dung thuần; không quyết phần thưởng; không quyết quyền (chỉ *kiểm* quyền qua `policy_hash` đã commit) |

---

## §1. Ký hiệu (giải thích bằng lời)

- `H32` — một giá trị băm 32 byte (256 bit). Hàm băm nền là **BLAKE3**. Viết tắt một giá trị thuộc tập `{0,1}^256`.
- `‖` — phép **nối byte** (concatenation). `a ‖ b` là chuỗi byte của `a` rồi đến `b`.
- `0^32` — chuỗi 32 byte toàn số 0 (dùng cho `prev_hash` của phiên bản gốc).
- `H_dom(tag, x)` — **hàm băm phân tách miền** (domain-separated): `H_dom(tag, x) = BLAKE3(tag ‖ 0x00 ‖ x)`, với `tag` là chuỗi ASCII dạng `"LN/STRATA/..."`. Byte `0x00` chen giữa `tag` và `x` để không một `tag` nào là tiền tố của ghép `tag ‖ x` của tag khác.
- **RFC 6962 prefix** — quy ước gắn một byte nhãn vào trước dữ liệu trước khi băm trong cây Merkle, để phân biệt lá với nút trong:
  - lá: `H_leaf(d) = H(0x00 ‖ d)`
  - nút trong: `H_node(l, r) = H(0x01 ‖ l ‖ r)`
- `version` — **nút phiên bản**, gồm các trường theo thứ tự canonical cố định:
  `{ seq:u64, prev_hash:H32, content_cid:Cid, state_root:H32, author_did:Did, policy_hash:H32, ts:u64, sig:Sig }`.
  - `seq` — số thứ tự phiên bản, `u64`, bắt đầu 0, tăng đúng +1.
  - `prev_hash` — `version_hash` của phiên bản liền trước (phiên bản gốc dùng `0^32`).
  - `content_cid` — CID nội dung (hash thuần BLAKE3 của nội dung phiên bản, đi vào Mirage; mã hóa nếu nhạy cảm).
  - `state_root` — gốc cây Merkle field-level của trạng thái sau phiên bản này (xem §6).
  - `author_did` — DID PhoenixKey của người tạo phiên bản.
  - `policy_hash` — cam kết hash của tập quyền (ai được sửa phần nào).
  - `ts` — dấu thời gian, `u64` (giây hoặc mili giây tùy cấu hình; đơn điệu không-giảm).
  - `sig` — chữ ký Ed25519 **canonical (low-S)** của `author_did` trên `version_hash` (KHÔNG trộn vào `version_hash`; xem §3.1, CHỐT-1).
- `version_hash` — băm định danh một phiên bản, ràng buộc cả nội dung phiên bản lẫn chữ ký (định nghĩa chính xác ở §3).
- `mmr_root` — gốc của Merkle Mountain Range trên dãy lá = `version_hash` của các phiên bản (§4).
- `ref_id` — định danh ổn định `lnref1…` (bech32), opaque, sinh từ `(DID người tạo ‖ nonce genesis)`. **Không** đổi qua các phiên bản; **không** mã hóa loại/độ nhạy.
- `anchor` — neo on-chain, gồm đúng 4 trường: `(ref_id, head_version_hash, mmr_root, seq)`, kích thước ~vài chục byte.
- `n` — số phiên bản hiện có trong một Strata (số lá MMR). `lg n` = logarit cơ số 2.

---

## §2. Hàm băm an toàn

Strata dùng một hàm băm nền duy nhất là BLAKE3 (đầu ra 32 byte), nhưng **không bao giờ** băm dữ liệu thô trực tiếp. Mọi lần băm đi qua hai lớp bảo vệ: phân tách miền (domain separation) và prefix RFC 6962. Đây là yêu cầu INV-E8.

### §2.1 Phân tách miền — `H_dom`

```
H_dom(tag, x) = BLAKE3( tag ‖ 0x00 ‖ x )
```

`tag` là một nhãn ASCII cố định cho từng mục đích băm. Bảng tag CHUẨN (sao đúng `_CONTRACT.md` CHỐT-2, là tập DUY NHẤT — không tự đặt tên khác):

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

*Vì sao cần*: nếu mọi mục đích dùng chung `BLAKE3(x)`, một giá trị băm hợp lệ cho mục đích A có thể bị dùng lại làm giá trị hợp lệ cho mục đích B (cross-protocol / type-confusion). Byte phân tách `0x00` ngay sau `tag` bảo đảm: với hai tag khác nhau `t1 ≠ t2`, không tồn tại `x1, x2` sao cho `t1 ‖ 0x00 ‖ x1 = t2 ‖ 0x00 ‖ x2` trừ khi `t1 = t2` (vì `0x00` không xuất hiện trong các `tag` ASCII in được, nên vị trí dấu phân tách là duy nhất). Hệ quả: tìm va chạm liên-miền khó như tìm va chạm BLAKE3 thường.

### §2.2 Prefix RFC 6962 — phân biệt lá và nút trong

Trong mọi cây Merkle của Strata:

```
H_leaf(d)    = H_dom(tag_leaf, d)            (khái niệm: H(0x00 ‖ d))
H_node(l, r) = H_dom(tag_node, l ‖ r)        (khái niệm: H(0x01 ‖ l ‖ r))
```

*Vì sao cần — chống second-preimage kiểu leaf-vs-node*: nếu lá và nút trong băm bằng cùng công thức, kẻ tấn công có thể trình một **lá** có giá trị bằng đúng `l ‖ r` của một **nút trong**, khiến verifier nhầm một nút trong thành một lá (và ngược lại). Khi đó hai cây khác cấu trúc có thể cho cùng root → second-preimage. Gắn nhãn miền khác nhau cho lá (`tag_leaf`, tương ứng prefix `0x00`) và nút (`tag_node`, tương ứng prefix `0x01`) làm hai không gian băm tách rời: một preimage hợp lệ ở miền lá không bao giờ là preimage hợp lệ ở miền nút. Đây chính là khắc phục lỗi "second-preimage / leaf-vs-node" nêu trong `_CONTRACT.md`.

### §2.3 Dup-leaf guard — CVE-2012-2459

**Tấn công**: cây Merkle nhị phân cổ điển (kiểu Bitcoin) khi gặp số lá lẻ ở một tầng sẽ **nhân đôi lá cuối** (`H_node(x, x)`) cho đủ cặp. Kẻ tấn công lợi dụng: với một danh sách `[a, b, c]` (lẻ, nhân đôi `c`), tồn tại danh sách *khác* `[a, b, c, c]` (chẵn) cho **cùng một root**. Hai cây khác nhau, một root → mâu thuẫn bằng chứng (CVE-2012-2459, từng cho phép giả mạo khối Bitcoin).

**Cách Strata chặn**:

1. **Cấm nhân đôi lá lẻ.** Strata không bao giờ tạo nút `H_node(x, x)` từ một lá đơn lẻ.
2. **MMR xử lý số lẻ bằng carry, không copy.** Khi một tầng có lá lẻ dư ra, MMR **không** ghép nó với chính nó mà **giữ nguyên** nó như một "đỉnh núi" (peak) riêng, đem ghép ở bước bagging cuối (§4). Một cấu trúc lá xác định cho đúng một tập đỉnh xác định, nên `[a,b,c]` và `[a,b,c,c]` cho **hai mmr_root khác nhau** — không còn lối va chạm cấu trúc.

Vì vậy Strata thỏa: số lá khác nhau ⇒ tập đỉnh khác nhau ⇒ root khác nhau (với xác suất áp đảo, trừ va chạm BLAKE3). Lỗi dup-leaf bị đóng.

---

## §3. Hash-linked version chain

### §3.1 Định nghĩa `version_hash`

Gọi `core(v)` là mã hóa tất định (deterministic, ví dụ TLV độ-dài-có-tiền-tố hoặc CBOR canonical) của **tất cả** trường phiên bản **trừ** `sig`, theo đúng thứ tự canonical ở §1:

```
core(v) = canonical( seq, prev_hash, content_cid, state_root, author_did, policy_hash, ts )
```

`version_hash` băm **đúng phần lõi**, **KHÔNG** trộn `sig` (theo `_CONTRACT.md` CHỐT-1):

```
version_hash(v) = H_dom("LN/STRATA/ver/v1", core(v))
sig(v)          = Ed25519_sign( sk(author_did), version_hash(v) )   // bắt buộc canonical (low-S)
```

Tức là: phần lõi được băm thẳng ra `version_hash`; tác giả ký **chính `version_hash`** đó. Chữ ký nằm *ngoài* phép băm. Hai tính chất cần thiết:

- `version_hash` định danh duy nhất phần **nội dung** phiên bản (không phụ thuộc chữ ký). Vì `prev_hash` của phiên bản kế trỏ vào `version_hash` này, liên kết chuỗi (§3.2) khóa chặt nội dung.
- `sig` **bắt buộc là chữ ký Ed25519 canonical (low-S)**: với một cặp `(pk, version_hash)` chỉ tồn tại **một** chữ ký canonical hợp lệ. Điều này chặn malleability ở tầng chữ ký (không thể tạo một `sig'` khác cùng hợp lệ cho cùng nội dung) — chứng minh ở §10 Mệnh đề 6. Verifier kiểm `Ed25519_verify(pk(author_did), version_hash(v), sig(v))` **và** kiểm dạng canonical (low-S) rồi mới chấp nhận; cột chặt nội dung ↔ tác giả (phục vụ INV-E4).

> Lưu ý mã hóa: `canonical` phải là **song ánh** trên miền trường (mỗi bộ trường ↔ đúng một chuỗi byte). Dùng độ-dài-có-tiền-tố cho mọi trường biến độ dài (`content_cid`, `author_did`) để tránh nhập nhằng ranh giới — nếu không, hai bộ trường khác nhau có thể cho cùng `core` và cùng `version_hash` (va chạm cấu trúc, không phải va chạm BLAKE3). Đây là điều kiện để mọi mệnh đề an toàn ở §10 quy được về độ khó BLAKE3.

### §3.2 Liên kết chuỗi và chứng minh INV-E1, INV-E2

**INV-E1 (hash-linked)**: với mọi `k ≥ 1`, phiên bản `v_k` (có `seq = k`) thỏa `prev_hash(v_k) = version_hash(v_{k-1})`; phiên bản gốc `v_0` có `prev_hash(v_0) = 0^32`.

**INV-E2 (đơn điệu seq)**: `seq(v_k) = k`, tăng đúng +1, không nhảy, không lùi.

**Mệnh đề (avalanche — sửa quá khứ làm hỏng tương lai).** Giả sử kẻ tấn công thay phiên bản `v_k` bằng `v_k'` với `v_k' ≠ v_k` (sửa bất kỳ trường lõi nào). Khi đó hoặc `version_hash(v_k') = version_hash(v_k)` (va chạm BLAKE3 — bỏ qua, xác suất ~`2^-256`), hoặc `version_hash(v_k') ≠ version_hash(v_k)`. Trong trường hợp sau:

- Phiên bản kế `v_{k+1}` có `prev_hash(v_{k+1}) = version_hash(v_k) ≠ version_hash(v_k')`. Để chuỗi vẫn hợp lệ (INV-E1), kẻ tấn công phải sửa `prev_hash(v_{k+1})` thành `version_hash(v_k')`, nhưng `prev_hash` là một trường lõi của `v_{k+1}` → `core(v_{k+1})` đổi → `version_hash(v_{k+1})` đổi.
- Lập luận lặp lại: thay đổi lan tới `v_{k+2}, …, v_{n-1}`. **Mọi** `version_hash` của các phiên bản `> k` đều phải đổi.

**Hệ quả**: không thể sửa lén một phiên bản quá khứ mà giữ nguyên phần đuôi. Muốn giữ chuỗi hợp lệ, kẻ tấn công phải băm lại và **ký lại** mọi phiên bản sau `k` (cần khóa của từng `author_did`), rồi còn phải cập nhật `mmr_root` và `anchor` on-chain (bị INV-E7 chặn, §7). Đây là tính bất biến lịch sử của Strata. ∎

> So với git: git cũng hash-linked DAG nên có avalanche tương tự cho *nội dung*. Khác biệt của Strata: (a) `seq` liên tục (INV-E2) cho phép chứng minh "không có version chen giữa" (§5); (b) head được **ký** và **neo on-chain** (INV-E4, INV-E7), trong khi nhánh git là ref khả biến (mutable ref) có thể bị tua/đổi không để lại dấu.

---

## §4. Merkle Mountain Range (MMR)

### §4.1 Vì sao MMR thay vì cây Merkle cân bằng

Lịch sử Strata là **chỉ-thêm** (append-only): phiên bản mới luôn nối vào cuối, phiên bản cũ bất biến. Một cây Merkle cân bằng cố định kích thước phải **dựng lại gần như toàn bộ** mỗi lần thêm lá (đổi số lá → đổi hình cây → đổi hầu hết nút trong → mọi proof cũ có thể sai). MMR sinh ra đúng cho ca này: nó là một **rừng các cây nhị phân hoàn hảo** (perfect binary trees), thêm lá chỉ tạo nút mới và đôi khi gộp hai cây cùng cỡ, **không đụng** các nút đã có. Nhờ đó proof cũ vẫn đúng dưới root mới (INV-E3).

### §4.2 Cấu trúc MMR

Với `n` lá (`leaf_i = H_leaf(version_hash(v_i))`, `i = 0..n-1`), MMR là tập các cây nhị phân hoàn hảo có kích thước là các lũy thừa 2 trong **khai triển nhị phân của n**. Ví dụ `n = 11 = 8 + 2 + 1` → ba cây cỡ 8, 2, 1. Gốc mỗi cây gọi là một **đỉnh** (peak). Số đỉnh = số bit 1 của `n`, tức `popcount(n) ≤ ⌈lg(n+1)⌉`.

Nút trong dựng bằng prefix RFC 6962:

```
parent = H_node(child_left, child_right) = H_dom("LN/STRATA/mmr/node/v1", child_left ‖ child_right)
```

**Bagging the peaks** — gộp các đỉnh thành một root duy nhất. Gọi các đỉnh từ cây lớn nhất tới nhỏ nhất là `p_1, p_2, …, p_m`. Gộp từ phải sang trái (fold-right), có ràng buộc số lá để chống nhập nhằng:

```
bag = p_m
for j = m-1 downto 1:
    bag = H_node(p_j, bag)
mmr_root = H_dom("LN/STRATA/mmr/root/v1", u64_be(n) ‖ bag)
```

Băm kèm `n` (số lá, big-endian 8 byte) vào root: hai dãy lá khác độ dài luôn cho `mmr_root` khác (củng cố guard dup-leaf §2.3 — kích thước được commit tường minh).

### §4.3 Append `O(log n)` amortized

```
append(version_hash):
    leaf = H_leaf(version_hash)
    push leaf vào danh sách đỉnh
    while hai đỉnh cuối cùng có cùng kích thước (cùng chiều cao):
        r = pop;  l = pop
        push H_node(l, r)
    n = n + 1
    cập nhật mmr_root bằng bagging (§4.2)
```

Vòng `while` gộp các cây cùng cỡ giống hệt phép **cộng nhị phân có nhớ** (carry): thêm 1 vào `n` tạo tối đa một chuỗi carry. Số lần gộp trên toàn bộ `n` lần append là `n - popcount(n) < n`, nên **amortized `O(1)` phép băm gộp/append**; chi phí cập nhật `mmr_root` (bagging) là `O(popcount n) = O(lg n)` mỗi append. Tổng: `O(log n)` mỗi append (chi phối bởi bagging), `O(1)` amortized cho phần xây cây.

### §4.4 Inclusion proof và kích thước `O(log n)`

Proof "lá `i` (tức `version_hash(v_i)`) nằm trong cây có `mmr_root`" gồm:
1. **Đường anh em trong cây chứa lá `i`**: dãy hash anh em từ lá lên đỉnh của cây con đó — độ dài = chiều cao cây con `≤ ⌊lg n⌋`.
2. **Các đỉnh còn lại** để bagging — `m - 1 < lg(n+1)` hash.
3. **Số lá `n`** (để tái tạo root đúng công thức §4.2).

Verifier: băm lên từ lá theo đường anh em ra đỉnh của cây con, rồi bag với các đỉnh khác, rồi băm với `n` → so với `mmr_root`. Kích thước proof:

```
|proof| = (chiều cao cây con) + (số đỉnh − 1) ≤ ⌊lg n⌋ + ⌈lg(n+1)⌉ = O(log n)
```

mỗi phần tử 32 byte. Đây là ưu điểm so với git (proof = cả path trong DAG, không có root gọn neo on-chain) và CIP-68 (không có proof lịch sử gọn).

### §4.5 Chứng minh INV-E3 (append-only)

**INV-E3**: thêm một phiên bản (một lá mới) chỉ MỞ RỘNG MMR; mọi inclusion-proof cũ vẫn đúng dưới `mmr_root` mới.

> Làm rõ phạm vi bất biến: bất biến là **đường lá → đỉnh (peak) của cây con chứa lá**, không phải *byte proof* nguyên khối. Khi root đổi, verifier xác thực lại bằng đường anh em cũ (bất biến) **cộng tập đỉnh cập nhật + `n` hiện hành** — phần "các đỉnh còn lại + n" lấy theo trạng thái mới. Tức proof cũ vẫn *đúng* nhưng phải verify dưới tập peak hiện hành, không nên hiểu là chuỗi byte proof bất biến.

**Chứng minh.** Xét trạng thái có `n` lá, đỉnh `P_n = {p_1,…,p_m}`. Khi append lá thứ `n` (thành `n+1` lá), thuật toán §4.3 chỉ **đẩy lá mới** rồi **gộp các cây cùng cỡ ở đuôi**. Các nút trong của những cây *không* tham gia gộp ở bước này **không bị chạm** — giá trị băm của chúng giữ nguyên, vì băm cha chỉ phụ thuộc hai con (đã cố định).

Với một lá cũ `i`: đường anh em của nó nằm hoàn toàn trong cây con hoàn hảo chứa nó tại thời điểm proof được sinh. Hai khả năng:
- Cây con đó **không** bị gộp ở các append sau: mọi nút trên đường anh em giữ nguyên → đỉnh cũ giữ nguyên. Khi bagging với tập đỉnh mới, verifier cần tập đỉnh *hiện hành*; proof inclusion theo chuẩn Strata mang **đường anh em tới đỉnh cây con** (bất biến) và cho phép thay phần "các đỉnh còn lại + n" bằng giá trị hiện hành. Đường anh em — phần ràng buộc lá vào đỉnh — không đổi.
- Cây con đó **bị gộp** vào cây lớn hơn: phép gộp chỉ **thêm** một nút cha mới *bên trên* đỉnh cũ; toàn bộ con đường cũ từ lá tới đỉnh cũ vẫn là tiền tố của đường mới tới đỉnh mới. Đường anh em cũ vẫn đúng, chỉ cần **nối thêm** các hash anh em mới ở phần trên (verifier dùng tập đỉnh hiện hành sẽ tái tạo đúng).

Trong cả hai khả năng, không một nút cũ nào bị *thay giá trị*; chỉ có nút *mới được thêm phía trên*. Do đó mọi quan hệ "lá `i` băm lên ra đỉnh cây con của nó" được bảo toàn, và inclusion-proof cũ (đường anh em tới đỉnh) vẫn xác thực dưới `mmr_root` mới. ∎

> Đây chính là tính chất một cây Merkle cân bằng-cố-định **không** có: ở cây cân bằng, đổi `n` thường đổi hình cây và băm lại nhiều nút trong, làm proof cũ sai. MMR chỉ thêm nút, không sửa nút — nên hợp append-only.

---

## §5. Inclusion & temporal proof

### §5.1 Proof "version k tồn tại"

Là inclusion-proof MMR của lá `k` (§4.4) cộng với chính `version_hash(v_k)` (và tùy chọn `core(v_k)` + `sig` nếu cần kiểm chữ ký). Verifier:
1. Tính `leaf_k = H_leaf(version_hash(v_k))`.
2. Băm lên theo đường anh em + bag với đỉnh hiện hành + commit `n` → so `mmr_root` (lấy từ `anchor` on-chain, §7).
3. (Tùy chọn) kiểm `Ed25519_verify` để chắc phiên bản do `author_did` ký.

Nếu khớp: phiên bản `k` **chắc chắn** từng được neo (trừ va chạm BLAKE3).

### §5.2 Proof "giá trị tại thời điểm t"

Mục tiêu: chứng minh "trạng thái tại thời điểm `t` là trạng thái sau phiên bản `k*`", với `k*` là phiên bản có `ts` lớn nhất mà `ts ≤ t`. Proof gồm hai phần:

**(A) `v_{k*}` tồn tại và `ts(v_{k*}) ≤ t`**: inclusion-proof của lá `k*` (§5.1) + tiết lộ `ts(v_{k*})` (nằm trong `core`, đã bị `version_hash` ràng buộc nên không sửa được).

**(B) Không có phiên bản nào chen giữa `(k*, t]`**: cần chứng minh "phiên bản kế tiếp `k*+1` hoặc không tồn tại, hoặc có `ts(v_{k*+1}) > t`".

- *Nếu `k*` là head* (`k* = n-1`): `anchor.seq = k*`, neo on-chain xác nhận không có phiên bản nào sau nó. Xong.
- *Nếu `k* < n-1`*: tiết lộ `v_{k*+1}` kèm inclusion-proof của lá `k*+1`, và `ts(v_{k*+1}) > t`. Vì `seq` **liên tục** (INV-E2: tăng đúng +1, không nhảy), giữa `k*` và `k*+1` **không thể** có phiên bản nào khác — không có "khe" `seq` để chèn. Cộng với `ts` đơn điệu không-giảm theo `seq`, mọi phiên bản `> k*` đều có `ts ≥ ts(v_{k*+1}) > t`. Vậy `k*` đúng là phiên bản cuối cùng có `ts ≤ t`.

**Vì sao INV-E2 là chìa khóa**: nếu `seq` được phép nhảy (ví dụ 5 rồi 9), kẻ tấn công có thể giấu một phiên bản `seq = 7` có `ts` nằm trong `(k*, t]` và vẫn trình một chuỗi "hợp lệ bề ngoài". Ràng buộc `seq` tăng đúng +1 loại bỏ mọi khe chèn: chứng minh `k*` và `k*+1` liền kề là đủ để khẳng định không có gì ở giữa. ∎

> Đây là khả năng "đọc giá trị tại thời điểm t" mà `_CONTRACT.md` yêu cầu, dùng để audit lịch sử (giá tài sản, số dư, nhiệt độ tại một mốc thời gian).

---

## §6. Field-level Merkle (state_root)

### §6.1 Định nghĩa

Một trạng thái có cấu trúc là tập cặp `(field_key, field_value)`. Định nghĩa:

Theo `_CONTRACT.md` khối "Mã hóa state leaf" (CHỐT-4), mã hóa chính xác:

```
fk_i              = field_key_i                          // khóa trường, chuỗi byte
field_value_bytes = giá trị inline HOẶC content_cid thuần (32B)   // CHỐT-4: content_cid thuần, KHÔNG class byte / doc_type
fvh_i  = H_dom("LN/STRATA/state/fval/v1", field_value_bytes)         // hash giá trị trường — KHÔNG để lộ giá trị thuần
leaf_i = H_dom("LN/STRATA/state/leaf/v1", u32_be(len(fk_i)) ‖ fk_i ‖ fvh_i)
node   = H_dom("LN/STRATA/state/node/v1", left ‖ right)
state_root = node-root trên các leaf_i đã sắp theo field_key tăng dần
```

`field_value_bytes` là giá trị inline (trường nhỏ) hoặc **`content_cid` thuần 32B** (trường lớn lưu off-chain). CHỐT-4 bắt buộc dùng `content_cid` thuần (`gen_content_cid`), **không** đính class byte hay doc_type — nếu không sẽ rò rỉ loại qua field-proof (vi phạm INV-E5/E6).

Sắp các lá **tất định** theo thứ tự từ điển của `fk_i` (lexicographic, byte-wise, `field_key` tăng dần). Dựng cây Merkle nhị phân trên dãy lá đã sắp, dùng prefix RFC 6962 (qua các tag `state/leaf` cho lá, `state/node` cho nút trong) + dup-leaf guard giống §2–§4 (số lá lẻ xử lý kiểu MMR/carry, không copy lá cuối). Gốc là `state_root`, được nhúng vào `core(version)` (§3) nên bị `version_hash` ràng buộc.

*Sắp xếp tất định* bảo đảm mọi node dựng cùng `state_root` từ cùng tập trường (yêu cầu đồng thuận, song song INV-R16 của Reward về tính tất định).

### §6.2 Field-proof và chứng minh INV-E6

Proof "trường `fk_j` có giá trị `field_value_j`" gồm: `field_value_j` (để verifier tính `fvh_j` rồi `leaf_j`), vị trí lá, và **đường anh em** từ `leaf_j` lên `state_root`. Verifier băm lên → so `state_root` (đã được `version_hash` neo).

**INV-E6 (field-privacy)**: proof một trường từ `state_root` KHÔNG tiết lộ giá trị các trường khác.

**Chứng minh.** Đường anh em chỉ gồm các **hash anh em** trên đường từ `leaf_j` tới gốc. Mỗi hash anh em là `H_dom(...)` của một nút con — hoặc là một `leaf_i` (đã là `H_dom` của `len ‖ fk_i ‖ fvh_i`) hoặc một nút trong (hash của hai con). Trong cả hai, phần tử proof là **ảnh băm**, không phải tiền ảnh. Để khôi phục `field_value_i` của trường khác từ một hash anh em, kẻ tấn công phải đảo `H_dom` (preimage BLAKE3) — khó `2^256`. Hơn nữa `fvh_i = H_dom("LN/STRATA/state/fval/v1", field_value_bytes)` đã băm giá trị một lần trước khi vào lá, nên ngay cả lá anh em (nếu lộ) cũng chỉ là hash của hash. Vậy giá trị các trường khác được bảo mật về tính toán. ∎

### §6.3 Giới hạn và giải pháp (zero-knowledge-lite)

Field-proof là **ZK-lite**, không phải zero-knowledge đầy đủ. Nó *có* rò rỉ hai thứ ngoài lề:

1. **Số trường** (gần đúng): độ sâu cây và số phần tử trong đường anh em để lộ chặn trên/dưới của số lá → lộ *khoảng* số trường.
2. **Hash anh em**: lộ các giá trị `H_dom(...)` của trường anh em (không phải giá trị, nhưng là một commitment cố định — cho phép so khớp "có thay đổi/không thay đổi" giữa hai proof của cùng vị trí).

*Giải pháp khi cần giấu cả số trường và chống so-khớp*:
- **Đệm (padding)** số lá lên lũy thừa 2 cố định bằng các lá giả `H_dom("LN/STRATA/state/pad/v1", nonce)` — che số trường thật.
- **Làm mù (blinding)**: `fvh_i = H_dom("LN/STRATA/state/fval/v1", salt_i ‖ field_value_bytes)` với `salt_i` ngẫu nhiên mỗi phiên bản — khiến hash anh em đổi mỗi lần, chặn so-khớp liên-proof và tấn công từ điển trên giá trị ít entropy (ví dụ trường boolean).
- Khi cần ẩn hoàn toàn (chứng minh thuộc tính mà không lộ cả hash), nâng lên cam kết đại số + bằng chứng ZK (ngoài phạm vi V hiện tại; ghi chú để backlog).

Đây là sự đánh đổi có chủ đích: ZK-lite rẻ (chỉ băm), đủ cho phần lớn ca dùng; khi cần kín hơn thì bật padding + blinding.

---

## §7. Chống rollback (INV-E7) và quyền (INV-E4)

### §7.1 Anchor đơn điệu — INV-E7

**INV-E7 (chống rollback)**: anchor on-chain đơn điệu theo `seq`; không thể neo lại version cũ.

`anchor = (ref_id, head_version_hash, mmr_root, seq)`. Hợp đồng on-chain cập nhật anchor cho một `ref_id` chỉ chấp nhận anchor mới nếu:

```
seq_mới > seq_cũ
∧ head_version_hash_mới khớp version_hash của phiên bản seq = seq_mới
∧ inclusion-proof: lá tương ứng head_cũ vẫn nằm dưới mmr_root_mới   (append-only, §4.5)
```

Điều kiện `seq_mới > seq_cũ` được thực thi **bởi chuỗi** (so sánh số nguyên trong validator/script). Vì chuỗi giữ trạng thái `seq_cũ` cuối cùng và từ chối mọi anchor có `seq ≤ seq_cũ`, không ai có thể "tua lùi" về một head cũ — kể cả tác giả hợp lệ. Đây là **finality kinh tế**: muốn ghi đè cần một phiên bản `seq` lớn hơn, không bao giờ nhỏ hơn. Điều kiện inclusion của head cũ dưới root mới bảo đảm anchor mới là phần **mở rộng** của lịch sử cũ, không phải một nhánh khác (chống "rewrite then re-anchor").

*Khắc phục lỗi hệ cũ*: git branch là mutable ref, tua lùi không để dấu; Strata neo head đã ký + `seq` đơn điệu on-chain nên rollback bị chặn và tamper-evident.

### §7.2 Quyền và chữ ký — INV-E4

**INV-E4 (quyền + chữ ký)**: `sig` hợp lệ bởi khóa của `author_did`, và `author_did` được `policy_hash` cho phép sửa phần tương ứng.

- **Chữ ký**: như §3.1, `sig(v) = Ed25519_sign(sk(author_did), version_hash(v))`, **canonical (low-S)**. Verifier kiểm `Ed25519_verify(pk(author_did), version_hash(v), sig(v))` và kiểm dạng canonical. Một phiên bản không có chữ ký hợp lệ của `author_did` của chính nó bị từ chối — không ai mạo danh tạo phiên bản.
- **Ánh xạ Did → pubkey** (`_CONTRACT.md` CHỐT-5): `author_did` lưu dạng `Did = [u8;32]` — đây là **băm** DID PhoenixKey, **không phải** khóa công khai. Để `Ed25519_verify`, verifier phải tra `pk` tương ứng `author_did` qua **key-registry** của lampnet-join/PhoenixKey. Strata **không** giả định `Did == pubkey`; phụ thuộc này là điều kiện ngoài (external dependency) của mọi bước kiểm chữ ký — nếu registry không phân giải được `Did → pk` thì phiên bản coi như chưa xác thực được tác giả.
- **Quyền**: `policy_hash` là cam kết hash của tập quyền `Policy` (ai được sửa trường/phần nào). Khi cập nhật một trường, người sửa phải kèm **bằng chứng quyền**: một phần tử của `Policy` (ví dụ entry `(author_did, field_key, perm)`) cộng Merkle-proof rằng entry đó nằm dưới `policy_hash` (nếu `Policy` là cây Merkle) — tương tự field-proof §6. Verifier kiểm: (a) `author_did` xuất hiện trong `Policy` với quyền trên `field_key` đang sửa; (b) `policy_hash` của phiên bản khớp commit. Nhờ commit hash, tập quyền không thể bị sửa lén mà không đổi `policy_hash` → đổi `core` → đổi `version_hash`.

Hai thứ cùng nhau: chữ ký chứng *ai làm*, policy chứng *người đó được phép làm gì*. Phiên bản chỉ hợp lệ khi cả hai đạt.

> **SPEC-TODO (INV-E4 phạm vi V1)**: V1 thực thi **mức chain** — `policy_hash` cam kết tập `author_did` được phép, mọi author hợp lệ sửa được **mọi trường**. Phần "bằng chứng quyền field-level" (entry `(author_did, field_key, perm)` + Merkle-proof dưới `policy_hash`) **deferred** sang phiên bản sau; INV-E4 vẫn giữ nguyên trong danh sách, chỉ thu hẹp phạm vi cài đặt V1 để spec ↔ code trung thực.

---

## §8. Gộp lô tần suất cao

Register/IoT có thể cập nhật **mỗi giây**. Nếu mỗi cập nhật đẻ một phiên bản, `n` tăng vô hạn → chuỗi và MMR phình. Giải pháp: **một phiên bản = một sub-MMR của N entry trong một epoch-checkpoint**.

### §8.1 Mô hình sub-MMR theo epoch

Chia thời gian thành các **epoch-checkpoint** (mặc định mỗi 1 giờ, đồng bộ `EPOCH_DURATION_SECS=3600` ở Strata-Tech §7). Trong một epoch, các entry tần số cao `e_1, …, e_N` (mỗi entry là một cập nhật giá trị + `ts`) được gom vào một **sub-MMR** riêng:

```
batch_root = mmr_root( [ H_leaf(H_dom("LN/STRATA/entry/v1", canonical(e_j))) : j = 1..N ] )
```

Cuối epoch, Strata tạo **một** phiên bản `v_k` với `content_cid`/`state_root` phản ánh `batch_root` (ví dụ nhúng `batch_root` vào state, hoặc dùng nó làm `content_cid` của batch). Chuỗi phiên bản chỉ tăng 1 mỗi epoch, nhưng từng entry trong epoch vẫn có inclusion-proof `O(log N)` qua sub-MMR, rồi `O(log n)` qua MMR phiên bản → tổng `O(log N + log n)`.

### §8.2 Công thức chi phí

Với `R` cập nhật/giây, epoch dài `T` giây, `N = R·T` entry/epoch:

| Đại lượng | Không gộp (1 entry = 1 version) | Gộp sub-MMR |
|---|---|---|
| Số phiên bản sau thời gian `D` | `R·D` | `D/T` |
| On-chain anchor/đơn vị thời gian | `R` anchor/s | `1` anchor mỗi `T` giây |
| Proof một entry | `O(log(R·D))` | `O(log N + log(D/T))` |
| Băm/append amortized | `O(1)`/entry | `O(1)`/entry (sub-MMR) + `O(log n)`/epoch (bagging version) |

Gộp giảm số anchor on-chain (chi phí đắt nhất) theo hệ số `T·R`, giữ proof vẫn logarit.

### §8.3 CRDT cho register hội tụ

Khi nhiều nguồn ghi đồng thời vào một register trong cùng epoch (thứ tự đến không chắc), dùng **CRDT** để hội tụ tất định, không phụ thuộc thứ tự mạng:

- **LWW-Register (Last-Writer-Wins có timestamp)**: mỗi cập nhật mang `(value, ts, writer_did)`. Giá trị thắng = `ts` lớn nhất; hòa `ts` thì phá hòa tất định bằng `writer_did` (so sánh byte). Tính chất hợp nhất giao hoán + kết hợp + lũy đẳng (commutative/associative/idempotent) ⇒ mọi node hội tụ cùng giá trị bất kể thứ tự nhận.
- **Counter CRDT (G-Counter / PN-Counter)** cho đếm view/like: mỗi nguồn giữ một bộ đếm riêng; giá trị = tổng (hoặc hiệu các bộ đếm dương/âm). Hội tụ không cần khóa.

**Ràng buộc thứ tự**: trong sub-MMR, entry vẫn được **sắp tất định** trước khi dựng cây (theo `ts` rồi `writer_did`), để `batch_root` của một epoch là duy nhất dù các node nhận entry theo thứ tự khác nhau. CRDT quyết *giá trị* hội tụ; sắp tất định quyết *cây* hội tụ. Hai thứ cùng nhau cho `batch_root` và `state_root` xác định trên mọi node.

> Khớp `_CONTRACT.md` ghi chú #2/#3: "Đếm view/like = register materialize từ append-log" — append-log là sub-MMR các entry, register là giá trị CRDT đọc ra ở cuối epoch.

---

## §9. Bốn loại MECE — biểu diễn toán

Strata là **một** primitive phục vụ cả 4 loại dữ liệu (MECE theo quan hệ định danh↔nội dung qua thời gian). Mỗi loại là một cấu hình của cùng bộ máy.

| Loại | Bản chất | Ánh xạ vào Strata | Chi phí proof |
|---|---|---|---|
| **1. Tĩnh** (write-once) | 1 ID ↔ 1 nội dung cố định | Strata **suy biến 1 lá**: chỉ phiên bản `v_0`, `n = 1`, `mmr_root = H_leaf(version_hash(v_0))`, không cập nhật. `content_cid` = nội dung. | `O(1)` — chỉ kiểm `version_hash` |
| **2. Chuỗi-thêm** (append-only) | Chuỗi mục, chỉ thêm cuối, mục cũ bất biến | **MMR chính là log**: mỗi mục = một lá; inclusion-proof "mục i tồn tại" = §4.4; append-only = INV-E3. State có thể trống. | `O(log n)` mỗi mục |
| **3. Thanh-ghi** (mutable register) | Chỉ giá trị mới nhất, ghi đè; lịch sử để audit | **Đọc head**: giá trị hiện hành = state của phiên bản `seq = anchor.seq` (head). Lịch sử = các phiên bản cũ trong MMR (audit qua §5). Cập nhật tần số cao gộp theo §8 (CRDT + sub-MMR). | đọc head `O(1)`; audit lịch sử `O(log n)` |
| **4. Hồ sơ cấu trúc** (structured evolving) | Nhiều trường, cập nhật từng phần theo quyền | **state_root field-level** (§6) + **policy_hash** (§7.2): mỗi cập nhật sửa một/vài trường, kiểm quyền qua policy, sinh phiên bản mới với `state_root` mới; field-proof tiết lộ chọn lọc một trường. | field-proof `O(log f)` (`f` = số trường) |

Khẳng định MECE: bốn loại phủ kín không gian (mỗi quan hệ định danh↔nội dung↔thời gian rơi đúng một loại) và không chồng lấn (tiêu chí phân loại — bất biến hoàn toàn / chỉ-thêm / ghi-đè / cập-nhật-từng-phần — loại trừ nhau). Một bộ máy Strata cấu hình được cả bốn nên không cần primitive riêng cho từng loại.

---

## §10. An toàn (mệnh đề)

Mọi mệnh đề dưới đây quy về một trong hai giả thiết khó: (i) BLAKE3 **kháng va chạm và kháng preimage** (tìm va chạm ~`2^128`, tìm preimage ~`2^256`); (ii) Ed25519 **không giả mạo được dưới tấn công chọn thông điệp** (EUF-CMA), và **tất định theo RFC 8032** (cùng `sk`+thông điệp → cùng chữ ký) — kết hợp ràng buộc low-S thì mỗi `(pk, version_hash)` có đúng một chữ ký hợp lệ (Mệnh đề 6b). Cộng thêm: mã hóa `canonical` là song ánh (§3.1) nên không có va chạm "cấu trúc" ngoài va chạm băm.

**Mệnh đề 1 — Bất biến lịch sử (INV-E1, INV-E2).** Không thể sửa một phiên bản quá khứ mà giữ chuỗi hợp lệ và head không đổi, trừ khi tìm được va chạm BLAKE3 hoặc giả mạo chữ ký Ed25519 của mọi tác giả phiên bản sau. *Lập luận*: §3.2 (avalanche) cho thấy sửa `v_k` buộc đổi `version_hash` mọi phiên bản `> k`; mỗi phiên bản phải được ký lại (EUF-CMA chặn) và head/`mmr_root` mới phải neo lại (Mệnh đề 2 chặn).

**Mệnh đề 2 — Chống rollback (INV-E7).** Không thể neo lại một head cũ trên chuỗi. *Lập luận*: §7.1 — chuỗi từ chối mọi anchor có `seq ≤ seq_cũ`; vượt qua đòi hỏi đảo so-sánh số nguyên on-chain (không thể) hoặc dựng một lịch sử dài hơn hợp lệ (đòi va chạm/giả-mạo theo Mệnh đề 1).

**Mệnh đề 3 — Append-only (INV-E3).** Thêm phiên bản không làm sai inclusion-proof cũ. *Lập luận*: §4.5 — MMR chỉ thêm nút, không sửa nút; đường anh em cũ được bảo toàn (chứng minh quy nạp trên hai khả năng gộp/không-gộp).

**Mệnh đề 4 — Field-privacy (INV-E6).** Field-proof một trường không lộ giá trị trường khác (về tính toán). *Lập luận*: §6.2 — phần tử proof chỉ là ảnh băm `H_dom`; khôi phục giá trị anh em đòi preimage BLAKE3. **Cảnh báo phạm vi**: đây chỉ là **ZK-lite**, KHÔNG zero-knowledge đầy đủ — proof *vẫn lộ* (a) số trường (gần đúng, qua độ sâu cây) và (b) các hash anh em (commitment cố định, cho phép so-khớp đổi/không-đổi). Muốn giấu cả số trường và chống so-khớp phải bật padding + blinding (§6.3).

**Mệnh đề 5 — Không rò rỉ loại (INV-E5).** `ref_id` và `content_cid` không tiết lộ loại/độ nhạy nội dung. *Lập luận*: `ref_id = bech32(H_dom(..., DID ‖ nonce_genesis))` và `content_cid` là hash thuần BLAKE3 của nội dung — không trường nào nhúng nhãn loại. Phân biệt được loại từ hai giá trị này đòi đảo băm (preimage). Loại nằm trong **state đã commit** (đọc cần proof + quyền), không trong định danh. Đây là khắc phục lỗi leak Vault/Bulk hệ cũ.

**Mệnh đề 6 — Quyền & xác thực tác giả (INV-E4).** Một phiên bản chỉ hợp lệ khi tác giả ký đúng và được `policy_hash` cho phép. *Lập luận*: §7.2 — chữ ký Ed25519 (EUF-CMA) chứng tác giả (sau khi phân giải `Did → pk` qua key-registry, CHỐT-5); Merkle-proof dưới `policy_hash` chứng quyền; sửa policy lén đổi `policy_hash` → đổi `core` → đổi `version_hash` (kháng va chạm).

**Mệnh đề 6b — Không malleable chữ ký (tamper-evidence của `sig`).** `version_hash` **không** trộn `sig` (CHỐT-1), nên không thể dựa vào "đổi sig đổi version_hash". Thay vào đó: yêu cầu `sig` **canonical Ed25519 (low-S)** cho ta tính duy nhất — với một cặp `(pk, version_hash)`, **chỉ tồn tại đúng một** chữ ký vượt cả `Ed25519_verify` lẫn kiểm low-S. *Lập luận*: Ed25519 chuẩn cho phép hai biểu diễn `S` và `S + ℓ` (ℓ = bậc nhóm) cùng verify — đó là khe malleability. Ràng buộc `0 ≤ S < ℓ` (low-S) loại bỏ biểu diễn thứ hai, nên ánh xạ `(sk, version_hash) → sig_canonical` là **hàm** (một đầu ra). Kẻ tấn công không thể sinh `sig' ≠ sig` cùng hợp lệ cho cùng `version_hash` mà không có `sk` (EUF-CMA), và không thể "biến thể" `sig` sẵn có (low-S chặn). Vậy `sig` là tamper-evident dù nằm ngoài `version_hash`.

**Mệnh đề 7 — Hashing an toàn (INV-E8).** Mọi cây dùng domain-sep + RFC 6962 prefix + dup-leaf guard. *Lập luận*: §2 — phân tách miền chặn type-confusion liên-miền; prefix lá/nút chặn second-preimage leaf-vs-node; guard + commit `n` chặn CVE-2012-2459. Tất cả quy về kháng va chạm/preimage BLAKE3.

**Mệnh đề 8 — Bảo mật nhạy cảm (INV-E9).** Nội dung/state nhạy cảm chỉ công khai ở dạng commitment hash; bản rõ được mã hóa (AES-256-GCM, khóa qua Argon2id/threshold) và **tái phân tán** qua Mirage. Strata yêu cầu **cả hai** (mã hóa và tái phân tán) cho dữ liệu nhạy cảm — chúng tách biệt: mã hóa giấu *nội dung*, tái phân tán bảo đảm *tính sẵn sàng*. Strata chỉ cam kết hash; bản rõ không bao giờ lên chuỗi.

**Tóm tắt quy giản**: phá Strata ⇒ hoặc tìm va chạm/preimage BLAKE3 (~`2^128`/`2^256`), hoặc giả mạo Ed25519 (EUF-CMA), hoặc phá AES-256-GCM/Argon2id (cho dữ liệu nhạy cảm). Không có lối phá "cấu trúc" nhờ domain-sep + RFC6962 prefix + dup-leaf guard + `canonical` song ánh.

---

## §11. So sánh bắt buộc

| Tiêu chí | **CIP-68** | **git** | **Strata** |
|---|---|---|---|
| Lịch sử bất biến | một phần (on-chain) | có (DAG hash-linked) | có (hash-chain + MMR) |
| Proof gọn lịch sử | không | proof = cả path | **`O(log n)`** (MMR) |
| On-chain mỗi cập nhật | datum nhỏ/đắt/lộ hết | không neo on-chain | **104 byte** (anchor; cam kết `mmr_root` 32 byte) |
| Append-only nội tại | không | có (nhưng ref khả biến) | **có** (INV-E3) + head ký |
| Chống rollback | yếu | branch tua được, không dấu | **`seq` đơn điệu on-chain** (INV-E7) |
| Field-privacy | không (datum lộ) | không | **có** (state_root, INV-E6) |
| Rò rỉ loại | có (datum) | — | **không** (INV-E5) |
| Finality kinh tế | có | **không** | **có** (neo on-chain ký) |

Strata lấy điểm mạnh của cả hai (bất biến của git + neo on-chain của CIP-68) và bỏ điểm yếu (proof to của git, datum đắt/lộ của CIP-68).

---

## §12. Composite Strata — biểu diễn toán

Đối tượng thật là một **rừng/đồ thị các Strata nguyên thủy** (xem Strata-Feat §6). Mô hình toán: một **Strata ghép** `C` là một Strata loại #4 mà `state` chứa các tham chiếu con. Gọi tập con là `{(ref_id_j, role_j)}`, mỗi con là một Strata nguyên thủy độc lập (loại bất kỳ trong #1–#4).

**Cam kết con qua state_root.** Mỗi con xuất hiện như một trường trong `state` của `C`:

```
field_key   = role_j                                     // VD b"profile", b"posts"
field_value = ref_id_j  (32B, hash thuần — KHÔNG class byte, CHỐT-4)
```

Vậy `state_root(C)` (theo §6) cam kết toàn bộ danh sách con. Thêm/bớt con = một version mới của `C` với `state_root` mới (INV-E1/E2 áp dụng nguyên). Quan hệ cha–con là **đệ quy**: con của `C` lại có thể là một Strata ghép.

**Proof hai tầng (composite inclusion).** Chứng minh "phần tử `x` thuộc đối tượng ghép `C`":

1. **Tầng cha**: field-proof từ `state_root(C)` rằng `role_j → ref_id_j` (§6.2) — chứng minh con `j` thuộc `C`, độ dài `O(log |children|)`.
2. **Tầng con**: inclusion-proof (§4.4) hoặc field-proof (§6.2) bên trong Strata `ref_id_j` rằng `x` thuộc con đó — `O(log n_j)`.

Tổng: `O(log |children| + log n_j)`. Verifier nối hai tầng: gốc tầng con (`ref_id_j` hoặc `mmr_root_j`/`state_root_j`) phải khớp giá trị mà field-proof tầng cha trả về.

**Tính chất bảo toàn.** Vì mỗi con giữ nguyên loại MECE của nó, các invariant của con không bị composite phá: append-only của con #2 vẫn là INV-E3 trong phạm vi con; field-privacy của con #4 vẫn là INV-E6. Composite chỉ **thêm một tầng cam kết** ở trên, không trộn các cây con vào một cây — nên không có rò rỉ chéo giữa các con (proof một con không lộ con khác, cùng lập luận §6.2 ở tầng cha).

> Khớp Strata-Feat §6: nhóm chat = rừng channel-log; bảng = rừng row-Strata + index; profile = profile(#4)+posts(#1/#2)+counters(#3). Bộ máy không đổi — composite là Strata cha tham chiếu Strata con, đệ quy.

---

## §13. Truy vấn lịch sử — số liệu

Mục này đưa con số cụ thể cho một ca dùng nặng: **một nhóm chat 100.000 thành viên, hoạt động 3 năm.** Đây là composite (rừng channel-log, §12), nhưng để ước lượng coi tổng dòng sự kiện như một MMR append-only (loại #2).

**Quy mô lá.** Giả định mỗi thành viên sinh trung bình `r` sự kiện/năm (tin nhắn + like + đọc). Với `r ≈ 180..360` sự kiện/người/năm:

```
n = 100_000 người × (180..360 sự kiện/người/năm) × 3 năm
  ≈ 54..108 triệu lá  ≈ 55..110 triệu (làm tròn)
```

**Kích thước inclusion-proof — `O(log n)`.** Với `n ≈ 10^8`:

```
log2(n) ≈ log2(10^8) ≈ 26,6   →  ~26 hash trên đường proof
|proof| ≈ 26 × 32 byte = 832 byte
```

Một proof "sự kiện thứ i từng tồn tại" chỉ **832 byte** dù lịch sử có hơn trăm triệu sự kiện. Đây là điểm mạnh MMR so với git (proof = cả path không có root gọn) và CIP-68 (không có proof lịch sử).

**Khung xương thời gian — checkpoint mỗi giờ.** Để truy vấn "trạng thái tại thời điểm t" nhanh, Strata giữ một **checkpoint mỗi giờ** (mốc `mmr_root` + `seq` tại đầu mỗi giờ; xem gộp lô §8). Số checkpoint trong 3 năm:

```
3 năm × 365 ngày × 24 giờ ≈ 26_280 checkpoint  (~26k)
mỗi checkpoint ≈ 80 byte   (mmr_root 32B + seq 8B + ts 8B + biên độ overhead ~32B)
tổng khung xương ≈ 26_280 × 80 byte ≈ 2,1 MB
```

**~2,1 MB "khung xương thời gian"** đủ định vị bất kỳ thời điểm nào trong 3 năm xuống độ phân giải một giờ; trong một giờ thì dùng inclusion-proof tới version có `ts ≤ t` (§5.2). Khung này nhỏ, có thể giữ ngay trên client.

**Index nóng cho truy vấn mili-giây.** Để trả "mọi tin của người gửi S quanh thời điểm t" trong mili-giây, Strata giữ một **index nóng** khóa `(sender, ts)` → vị trí lá. Index này là **materialized view derived, untrusted** (nguyên tắc cứng ở Strata-Tech): nó tăng tốc tìm kiếm, nhưng mọi kết quả vẫn verify được bằng inclusion-proof về `mmr_root` đã neo. Mất index = dựng lại từ log; index sai = proof không khớp, bị phát hiện.

Tổng kết số liệu nhóm 100k/3 năm:

| Đại lượng | Giá trị |
|---|---|
| Số lá (sự kiện) | ~55–110 triệu |
| `log2(n)` | ~26 |
| Kích thước một inclusion-proof | ~832 byte (26 × 32B) |
| Số checkpoint (mỗi giờ, 3 năm) | ~26.280 |
| Kích thước khung xương thời gian | ~2,1 MB (26k × 80B) |
| Độ trễ truy vấn `(sender, ts)` | mili-giây (qua index nóng derived) |

---

## §14. Merkle Sum Tree cho tabular (tính tổng có proof)

Bảng là rừng row-Strata, granularity per-row (Strata-Feat §8). Để chứng minh **tổng/đếm một cột** mà không tiết lộ từng hàng, dùng **Merkle Sum Tree (MST)** trên cột cần tổng.

**Cấu trúc.** Khác cây Merkle thường (node chỉ giữ hash), mỗi node MST giữ thêm `(sum, count)`:

```
leaf_i  = ( H_dom("LN/STRATA/sum/leaf/v1", …), value_i, 1 )            // value_i = giá trị ô của hàng i
node    = ( H_dom("LN/STRATA/sum/node/v1", h_L ‖ s_L ‖ c_L ‖ h_R ‖ s_R ‖ c_R),
            sum   = s_L + s_R,
            count = c_L + c_R )
root    = ( H_root, total_sum, total_count )
```

MST dùng **bộ tag riêng** `LN/STRATA/sum/{leaf,node}/v1` (đã thêm vào bảng domain-tag chuẩn `_CONTRACT.md` CHỐT-2), tách miền hoàn toàn khỏi cây `state_root` — đúng nguyên lý INV-E8 (mỗi cây một miền, không dựa vào việc "đầu vào node khác nhau"). `sum`/`count` được **băm vào node** nên không sửa được mà giữ root. Cây MST là một **cây tabular riêng** (mỗi cột cần tổng một MST), KHÔNG trộn vào `state_root` hồ sơ cấu trúc (§6). Cài đặt: `lampnet-merkle-anchor::sumtree` (hash-agnostic).

**Proof một hàng góp vào tổng.** Để chứng minh "hàng `i` có giá trị `value_i` và góp đúng vào `total_sum`": đường anh em từ `leaf_i` lên root **kèm `(sum, count)` của mỗi anh em**. Verifier cộng dồn lên, kiểm: (a) hash khớp root, (b) `sum`/`count` cộng dồn ra `total_sum`/`total_count` ở gốc. Mỗi bước verifier biết phần đóng góp mà không cần biết giá trị từng hàng khác.

**Tính chất.** MST cho **tổng kiểm chứng được** mà giữ field-privacy: proof một hàng lộ giá trị hàng đó + các `(sum, count)` anh em (tổng cục bộ, không phải từng giá trị) — yếu hơn ZK đầy đủ nhưng đủ để audit tổng. Muốn giấu cả tổng cục bộ thì cần blinding/ZK (backlog, như §6.3).

> **Trùng VeData A22 MEASUREMENT_SERIES**: MST là **sub-primitive dùng chung** với chuỗi đo lường A22 của VeData. Cài một lần trong `lampnet-merkle-anchor` (hash-agnostic, Strata-Tech §0.6), hai bên gọi chung. Lọc/join chạy trên columnar engine derived (untrusted), ngưỡng theo đo thực — không thuộc cây băm.

---

## §15. Liên kết spec khác

- **Specs/Strata/_CONTRACT.md**: khế ước giao diện — tên trường, ký hiệu, INV-E1..E9 (nguồn chuẩn).
- **Specs/Strata/Strata-Feat.md**: mục tiêu tính năng + 4 loại MECE + hành trình người dùng.
- **Specs/Strata/Strata-Tech.md**: cài đặt cụ thể `version_hash`, MMR, state_root, anchor thành code.
- **Mirage**: lưu `content_cid` theo lớp bảo mật (Vault/Bulk); tái phân tán cho INV-E9.
- **PhoenixKey / lampnet-join**: DID `author_did` (`Did=[u8;32]` băm DID), **key-registry phân giải `Did → pubkey`** (CHỐT-5), chữ ký Ed25519 canonical over `version_hash`, khóa cho mã hóa nhạy cảm.
- **On-chain (Cardano)**: validator/script neo `anchor` 4 trường, thực thi `seq` đơn điệu (INV-E7).

---

Khớp `_CONTRACT.md`: dùng đúng `H32`, `H_dom`, RFC 6962 prefix (leaf `0x00`/internal `0x01`), trường `version` theo thứ tự canonical, `version_hash`, `mmr_root`, `ref_id`, `content_cid`, `anchor (ref_id, head_version_hash, mmr_root, seq)`, và INV-E1..E9 đúng số.
