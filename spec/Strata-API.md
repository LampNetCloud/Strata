# Strata — API public (cửa platform gọi)

**Module**: Strata (`lampnet-strata`) — đặc tả API cho lập trình viên build platform (ProofChat / OriLife / AladinWork) gọi vào Strata.

> File này là **nguồn chuẩn API** (signature + request/response + lỗi). Tên/ký hiệu/invariant theo `_CONTRACT.md`. Toán ở `Strata-Math.md`; cài đặt nội bộ + endpoint HTTP gốc ở `Strata-Tech.md §6`. Mọi mâu thuẫn với `_CONTRACT.md` là lỗi.
>
> **Quan trọng — API trích từ CODE THẬT** (`lampnet-hivemind/lampnet-strata/src/*`), không bịa. Mọi signature dưới đây đã có trong crate (44 inline test + 11 integration test pass). Phần nào CHƯA có code đánh dấu rõ **[SPEC-TODO]**.

---

## §0. Mô hình gọi — hai lớp

Strata core là **crate THUẦN, no I/O** (`lampnet-strata`). Platform KHÔNG gọi thẳng crate qua mạng; giữa có một daemon (`lampnet-node`) làm I/O + lưu + key-registry + index dẫn xuất.

```
Platform (ProofChat/OriLife/AladinWork)
   │  HTTP JSON  (§3 — cửa public)
   ▼
lampnet-node daemon   ── lưu version/blob qua Mirage; phân giải Did→pk; index derived
   │  Rust API thuần   (§2 — signature crate thật)
   ▼
lampnet-strata core   ── StrataChain / StrataVersion / state / audit (no I/O)
   │
   ▼
lampnet-merkle-anchor  ── MMR + Merkle Sum Tree (hash-agnostic, BLAKE3)
```

- **Platform** chỉ thấy lớp HTTP (§3). Hash/CID truyền hex.
- **Daemon implementer** (cùng team LampNet) gọi lớp Rust (§2).
- Ranh giới giữ nguyên: core không chạm file/mạng; daemon không tự băm/dựng MMR.

Bảy thao tác public mà §6.1 Ecosystem yêu cầu, ánh xạ thẳng vào code:

| Thao tác (yêu cầu §6.1) | Lớp Rust thật (§2) | HTTP (§3) | Trạng thái |
|---|---|---|---|
| `create` | `StrataChain::genesis(ref_id, v0, &policy)` | POST `/v1/strata/create` | ✅ code |
| `append_version` | `chain.append_version(v, &policy)` | POST `/v1/strata/:ref/version` | ✅ code |
| `append_event` (loại #2) | `chain.append_version(v, &policy)` (state rỗng) **hoặc** `AuditLog::append_access` cho audit-log | POST `/v1/strata/:ref/event` | ✅ code (xem §2.6) |
| `read_head` | `chain.head()` / `chain.anchor()` | GET `/v1/strata/:ref/head` | ✅ code |
| `read_at(t)` | `chain.version_at(t)` | GET `/v1/strata/:ref/version?at=t` | ✅ code |
| `prove_version` | `chain.prove_version(seq)` | GET `/v1/strata/:ref/proof/version/:seq` | ✅ code |
| `prove_field` | `state::prove_field(fields, key)` | GET `/v1/strata/:ref/proof/field/:key` | ✅ code |
| `anchor` | `chain.publish_anchor()` → adapter §4 | POST `/v1/strata/:ref/anchor` | ✅ core; adapter [SPEC-TODO] |

---

## §1. Loại dữ liệu nền (khớp code)

Trích `lampnet-strata/src/{version,chain,state,refid,audit}.rs` + `lampnet-merkle-anchor/src/mmr.rs`. Đây là kiểu thật, KHÔNG phải đề xuất.

```rust
// version.rs — KHÔNG có struct "StrataRef"; trạng thái sống trong StrataChain.
pub type Hash32 = [u8; 32];                  // BLAKE3 32B
pub struct StrataVersion {
    pub seq: u64, pub prev_hash: Hash32, pub content_cid: Vec<u8>,
    pub state_root: Hash32, pub author_did: [u8;32], pub policy_hash: Hash32,
    pub ts: u64, pub sig: [u8;64],
}

// chain.rs
pub struct StrataChain { /* ref_id + Mmr<Blake3Hasher> + Vec<StrataVersion> + last_anchor_seq */ }
pub struct StrataAnchor { pub ref_id: Hash32, pub head_version_hash: Hash32, pub mmr_root: Hash32, pub seq: u64 }
pub struct Policy { /* allowed: BTreeMap<[u8;32], VerifyingKey> */ }
pub enum StrataError { HashLinkBroken, SeqNotMonotonic, BadSignature, PolicyDenied,
                       PolicyHashMismatch, UnknownAuthor, TimestampRegress, AnchorRollback, SeqOverflow }

// state.rs
pub struct FieldProof { pub key: Vec<u8>, pub value: Vec<u8>, pub fvh: Hash32,
                        pub siblings: Vec<(Hash32, bool)>, pub state_root: Hash32 }

// mmr.rs (re-export qua lampnet_merkle_anchor::mmr)
pub struct InclusionProof { pub siblings: Vec<(Hash32, bool)>, pub peak_index: usize, pub peaks: Vec<Hash32> }

// audit.rs
pub struct AuditEntry { pub created_ts: u64, pub actor_did: [u8;32], pub action: AuditAction,
                        pub signed_hash: Hash32, pub location: Hash32 }
pub enum AuditAction { Create, Read, Sign, ShareProof, Update }
pub struct AuditLog { /* Mmr<Blake3Hasher> + Vec<AuditEntry> + last_ts */ }
```

> **Đính chính spec ↔ code (xem §6 bảng đối chiếu):** `Strata-Tech.md §1.2` mô tả một struct `StrataRef` (ref_id + head + mmr_root) như SSoT trạng thái. Code KHÔNG có `StrataRef`; trạng thái head/mmr nằm trong `StrataChain` và được trả ra qua `StrataChain::anchor()`. Dùng `StrataChain` + `StrataAnchor` làm chuẩn.

---

## §2. API Rust (lớp daemon gọi core)

### §2.1 `create` — genesis (seq=0)

KHÔNG có free-fn `create_strata`. Trình tự thật: dựng `StrataVersion` chưa ký → ký → `StrataChain::genesis`.

```rust
// 1. ref_id (refid.rs) — THUẦN, không class byte (INV-E5). Trả String "lnref1…".
let ref_id_str: String = lampnet_strata::gen_ref_id(&author_did, &genesis_nonce);
let ref_id: Hash32      = lampnet_strata::refid::gen_ref_id_raw(&author_did, &genesis_nonce);

// 2. state_root field-level (state.rs). fields: &[(key, value_bytes)].
let state_root = lampnet_strata::build_state_root(&fields);

// 3. version chưa ký → ký Ed25519 (low-S verify_strict).
let mut v0 = StrataVersion::unsigned(0, [0u8;32], content_cid, state_root,
                                     author_did, policy.policy_hash(), ts);
v0.sign(&signing_key);                       // sig = Ed25519_sign(sk, version_hash(v0))

// 4. genesis — enforce seq==0, prev_hash==0^32, sig hợp lệ, author∈policy, policy_hash khớp.
let chain: StrataChain = StrataChain::genesis(ref_id, v0, &policy)?;   // Result<_, StrataError>
```

Loại #1 (Tĩnh) dừng tại đây (1 version). `content_cid` trỏ blob đã đẩy qua Mirage.

> **Đính chính:** `gen_ref_id` trong code trả **`String` bech32m** (`lnref1…`); bản raw 32B là `refid::gen_ref_id_raw`. `Strata-Tech.md §2.1` viết `gen_ref_id(...) -> H32` — đó là bản raw, đã đổi tên thành `gen_ref_id_raw`. `policy_hash` PHẢI bằng `policy.policy_hash()` đang thực thi, nếu không genesis trả `PolicyHashMismatch`.

### §2.2 `append_version` — thêm phiên bản (INV-E1/E2/E4 + ts đơn điệu)

```rust
let head_vh = chain.head_version_hash();
let mut v = StrataVersion::unsigned(chain.head().seq + 1, head_vh, content_cid,
                                    build_state_root(&fields), author_did, policy.policy_hash(), ts);
v.sign(&signing_key);
chain.append_version(v, &policy)?;            // &mut self; nối từ head, MMR mở rộng append-only
```

`append_version` enforce theo thứ tự: seq == head+1 (overflow→`SeqOverflow`); `prev_hash == head_version_hash` (chống fork → `HashLinkBroken`); `ts >= head.ts` (`TimestampRegress`); `policy_hash` khớp (`PolicyHashMismatch`); author∈policy (`PolicyDenied`); `verify_strict` sig (`BadSignature`); Did phân giải được (`UnknownAuthor`).

> **INV-E4 — hai chế độ (đã có code):**
> - **V1 mức-chain** (`append_version` / `genesis` + `chain::Policy`): mọi author trong `Policy` sửa mọi trường; `check_auth` kiểm tập author + khớp `policy_hash`.
> - **V2 field-level** (`append_version_fielded` / `genesis_fielded` + `field_policy::FieldPolicy` + `FieldAuthProof`): mỗi trường sửa cần một bằng chứng quyền `(author_did, field_key)` dưới `policy_hash`. `check_auth_fielded` kiểm `policy_hash` khớp (`PolicyHashMismatch`) + sig (`BadSignature`/`UnknownAuthor`) + MỖI trường có bằng chứng hợp lệ của chính author (thiếu → `FieldPolicyDenied { field_key }`; proof sai commit → `FieldProofPolicyMismatch { field_key }`).

### §2.3 `read_head` — giá trị mới nhất (loại #3)

```rust
let head: &StrataVersion = chain.head();      // seq lớn nhất
let anchor: StrataAnchor  = chain.anchor();    // (ref_id, head_version_hash, mmr_root, seq) — KHÔNG cập nhật last_anchor_seq
let root: Hash32          = chain.mmr_root();
```

Đếm view/like = register (#3) materialize từ append-log (#2): đọc head là giá trị cộng dồn hiện tại. Không phải loại riêng.

### §2.4 `read_at(t)` — giá trị tại thời điểm t

```rust
// Trả (version, inclusion-proof). None nếu t < ts(genesis). Binary search (ts đơn điệu).
let (v, proof): (&StrataVersion, InclusionProof) = chain.version_at(t)?;
```

### §2.5 `prove_version` / `prove_field` — bằng chứng

```rust
// MMR inclusion (INV-E3). Trả (proof, mmr_size, leaf_version_hash). None nếu seq ngoài phạm vi.
let (proof, size, vh) = chain.prove_version(seq)?;
let ok = StrataChain::verify_version(root, &vh, seq, size, &proof);   // root lấy từ anchor đã neo

// Field-proof (INV-E6) — KHÔNG lộ trường khác (sibling chỉ là hash).
let fp: FieldProof = lampnet_strata::prove_field(&fields, key)?;       // None nếu key vắng
let ok = lampnet_strata::verify_field_proof(&fp);                      // tự kiểm fvh == H(value)
```

`verify_version` SUY chiều trái/phải từ `leaf_index`, KHÔNG tin cờ trong proof (bind chặt index↔hash). `verify_field_proof` tính lại root từ `value` + siblings rồi so `state_root`.

> ⚠️ **Tiền đề key-duy-nhất (INV-E6/E6-key, xem §8.0):** một version PHẢI có `field_key` duy nhất. Tới khi core enforce `DuplicateFieldKey` (issue #39), một `FieldProof` verify hợp lệ dưới `state_root` đã ký **KHÔNG** bảo đảm giá trị duy nhất cho một key — nếu caller ký version có key trùng, hai giá trị mâu thuẫn cùng sinh proof hợp lệ dưới CÙNG root (equivocation). Verifier bên-3 phải coi đây là rủi ro tồn dư; caller PHẢI khử trùng key trước khi ký.

### §2.6 `append_event` — loại #2 (chuỗi-thêm) và audit-log

Hai cách, chọn theo nhu cầu neo:

1. **Event là một version** (mỗi event cần `author_did`/`policy`/anchor riêng): dùng `append_version` với `state` rỗng (`build_state_root(&[]) == [0u8;32]`), `content_cid` = CID nội dung event. MMR chính là log (INV-E3). Tốt cho event giá trị cao.

2. **Event tần suất cao / audit truy cập** (không đẻ version mỗi event): dùng `AuditLog` — mỗi event là một leaf MMR riêng:

```rust
let mut log = AuditLog::new();
let idx = log.append_access(AuditEntry {                 // enforce created_ts đơn điệu (TimestampRegress)
    created_ts, actor_did, action: AuditAction::Read, signed_hash, location })?;
let (proof, size, leaf_bytes) = log.prove(idx)?;          // inclusion "actor đã truy cập lúc T"
let ok = AuditLog::verify(log.root(), &leaf_bytes, idx, size, &proof);
```

Quan hệ với gộp lô tần suất cao (§5): nhiều event/giây gom thành sub-MMR theo epoch, một checkpoint = một version. `AuditLog` là một cài đặt sẵn của loại #2 cho ngữ cảnh "ai đọc gì khi nào".

### §2.7 `publish_anchor` — neo (enforce INV-E7)

```rust
let anchor: StrataAnchor = chain.publish_anchor()?;       // &mut self; seq <= last_anchor_seq → AnchorRollback
```

`anchor()` (read-only) KHÔNG cập nhật `last_anchor_seq`; chỉ `publish_anchor()` enforce đơn điệu. Đẩy on-chain qua adapter §4.

---

## §3. API HTTP (cửa public cho platform)

Style axum khớp `lampnet-node.rs` (`Router::new().route("/v1/...", post(handler))`). Hash/CID hex (64 char cho H32). Body JSON. Bảng route mở rộng §6 Strata-Tech; file này là spec chuẩn request/response + lỗi.

### POST `/v1/strata/create`
```jsonc
// req
{ "author_did":"<hex32>", "genesis_nonce":"<hex32>", "content_cid":"<hex>",
  "state_fields":[{"key":"diagnosis","value":"<hex32-content_cid-thuần>"}],
  "policy_hash":"<hex32>", "ts":1719600000, "sig":"<hex64>" }
// 200
{ "ref_id":"lnref1...", "head_seq":0, "head_version_hash":"<hex32>", "mmr_root":"<hex32>" }
```

### POST `/v1/strata/:ref/version`
```jsonc
// req
{ "prev_seq":4, "content_cid":"<hex>", "state_fields":[...],
  "author_did":"<hex32>", "policy_hash":"<hex32>", "ts":1719603600, "sig":"<hex64>" }
// 200
{ "seq":5, "version_hash":"<hex32>", "mmr_root":"<hex32>", "prev_hash":"<hex32>" }
```

### POST `/v1/strata/:ref/event`  (loại #2 / audit-log)
```jsonc
// req — kind="version": event = version state-rỗng; kind="audit": entry vào AuditLog.
{ "kind":"audit", "actor_did":"<hex32>", "action":"Read",
  "signed_hash":"<hex32>", "location":"<hex32>", "ts":1719603600, "sig":"<hex64>" }
// 200
{ "index":12, "log_root":"<hex32>" }            // kind=version → trả như /version
```

> **Byte-được-ký của `kind=audit` (chốt #23):** client ký trên **`AuditEntry::canonical()`** (KHÔNG phải `leaf_bytes()`). `AuditEntry` không có trường `sig` nên chữ ký KHÔNG vào MMR — đây là điểm DUY NHẤT trong Strata mà byte-được-ký không do root nào cam kết, nên phải nói tường minh. Daemon verify `pk.verify_strict(&ae.canonical(), &sig)` (INV-E4, loại malleable/small-order) bằng khoá phân giải từ `actor_did` (CHỐT-5). **KHÔNG** dùng domain-tag riêng (entry đã có `actor_did` + `created_ts` bên trong `canonical()`).

### GET `/v1/strata/:ref/head`
```jsonc
{ "ref_id":"lnref1...", "head_seq":5, "head_version_hash":"<hex32>",
  "mmr_root":"<hex32>", "content_cid":"<hex>" }
```

### GET `/v1/strata/:ref/version?at=<unix_ts>`
```jsonc
{ "seq":3, "version":{ /* StrataVersion hex */ },
  "proof":{ "leaf_seq":3, "leaf_hash":"<hex32>", "mmr_size":6,
            "siblings":[["<hex32>",true], ...], "peak_index":0, "peaks":["<hex32>", ...] } }
```

### GET `/v1/strata/:ref/proof/version/:seq`  → `InclusionProof` (so `mmr_root` đã neo, INV-E3)
### GET `/v1/strata/:ref/proof/field/:key`
```jsonc
{ "key":"diagnosis", "value":"<hex32-content_cid-thuần>", "fvh":"<hex32>",
  "siblings":[["<hex32>",false], ...], "state_root":"<hex32>", "version_seq":5 }
```

### POST `/v1/strata/:ref/anchor`  (đẩy anchor on-chain qua adapter §4)
```jsonc
// req  (priority lấy từ Stamp anchor_priority — Stamp-Strata-Mapping §4)
{ "priority":"immediate" }
// 200
{ "ref_id":"<hex32>", "head_version_hash":"<hex32>", "mmr_root":"<hex32>", "seq":5,
  "anchor_txid":"<hex>", "backend":"settlement" }     // anchor_txid null nếu no_anchor
```

### §3.1 Bảng lỗi HTTP (ánh xạ `StrataError`)

| HTTP | `StrataError` | Khi nào | INV |
|---|---|---|---|
| 409 Conflict | `HashLinkBroken` | `prev_hash` != head (fork / version cũ) | E1 |
| 422 Unprocessable | `SeqNotMonotonic` | seq nhảy/lùi (không = head+1) | E2 |
| 422 | `SeqOverflow` | seq đạt `u64::MAX` | E2 |
| 403 Forbidden | `PolicyDenied` | author không trong policy | E4 |
| 403 | `BadSignature` | `verify_strict` fail (sai khóa / malleable / tamper) | E4 |
| 403 | `PolicyHashMismatch` | `version.policy_hash` != cam kết policy đang thực thi | E4 |
| 424 Failed Dependency | `UnknownAuthor` | key-registry không phân giải `Did → pubkey` | E4/CHỐT-5 |
| 422 | `TimestampRegress` | `ts` < ts head (hoặc entry trước trong audit-log) | — |
| 409 | `AnchorRollback` | neo lại seq ≤ seq đã neo | E7 |
| 404 Not Found | (None từ `version`/`prove_version`/`prove_field`) | ref/seq/key không tồn tại | — |

Daemon trả `{ "error":"<StrataError variant>", "detail":{...} }`. Fail-closed: mọi vi phạm invariant từ chối version, KHÔNG ghi.

---

## §4. Adapter anchor → Mosaic / settlement (một đường neo)

§6.1 Ecosystem + Ecosystem-DataFlow §3.3/§4.4 nêu rõ: Strata sinh `anchor` 4 trường nhưng **KHÔNG tự neo** (không có dep Cardano). Cần **một adapter một-đường** thống nhất; hội tụ OriLife 1454/1455 + Strata + VeData Mosaic về một cơ chế. Phần này là spec interface; cài đặt là **[SPEC-TODO]** (chưa có code).

### §4.1 Interface (trait — daemon-side, ngoài core thuần)

```rust
/// Adapter một-đường: nhận StrataAnchor (104 byte) đã enforce INV-E7, đẩy on-chain.
/// Core KHÔNG biết Cardano; adapter sống ở daemon. Một trait, nhiều backend.
pub trait AnchorSink {
    /// Đẩy commitment. `priority` lấy từ Stamp anchor_priority (Stamp-Strata-Mapping §4).
    /// Trả receipt (txid + backend) hoặc None nếu priority == NoAnchor (sống ở tầng a/b).
    fn publish(&self, anchor: &StrataAnchor, priority: AnchorPriority)
        -> Result<Option<AnchorReceipt>, AnchorError>;
}

pub enum AnchorPriority { Immediate, Milestone, BatchDaily, NoAnchor }  // = Stamp 4-enum

pub struct AnchorReceipt { pub txid: String, pub backend: AnchorBackend, pub slot: Option<u64> }
pub enum AnchorBackend { Settlement, Mosaic }
pub enum AnchorError { NotConfigured, Rejected(String), Network(String) }
```

### §4.2 Hai backend, một interface

| Backend | Cơ chế | Khi nào | Nguồn pattern |
|---|---|---|---|
| **Settlement** (LampNet) | tx metadata **label 1234**, record `{t,a}` raw-bytes (`t=1` anchor, `a=[ref_id, head_version_hash, mmr_root, seq]`) — byte-layout ở §8.1(a), test-vector `apis/settlement-metadata.json`; message người-đọc label **674** CIP-20 | Strata cập nhật dày, gộp lô (priority `batch_daily`/`milestone`) — rẻ nhất | `lampnet-settlement/src/settle.ts:344-391` |
| **Mosaic** (VeData/GreenSun) | reference UTxO CIP-68 spend-recreate, validator Plutus V3 enforce `seq' == seq + 1` on-chain (INV-E7) — **đã build + chạy Preview**; mã ở `VeDataIO/Code: mosaic/aiken/validators/strata_anchor.ak` (KHÔNG ở repo này) | giá trị cao cần on-chain state + finality (priority `immediate`) | `Strata-Tech.md §5.2 Lựa chọn A` + `§5.4`, `Stamp-Strata-Mapping §4` |

Cadence đẩy theo `AnchorPriority` (Stamp-Strata-Mapping §4): `immediate` → đẩy mỗi version (Mosaic A); `milestone` → mốc/epoch; `batch_daily` → gom ngày (settlement metadata); `no_anchor` → KHÔNG đẩy, sống tầng (a)/(b).

### §4.3 Ràng buộc adapter

- Adapter nhận anchor **sau** `publish_anchor()` đã enforce INV-E7 ở core (seq đơn điệu). Với backend metadata (settlement B), INV-E7 còn cần indexer off-chain từ chối anchor seq ≤ seq đã thấy (metadata không có validator nội tại). Với Mosaic A, validator enforce on-chain.
- Chỉ đẩy `StrataAnchor` 104 byte; KHÔNG đẩy content/datum đầy đủ (riêng tư INV-E5, rẻ).
- `MIN_LOVELACE_PER_OUTPUT = 1_500_000n` (`settle.ts:374`) áp cho backend UTxO (Mosaic A); metadata (settlement B) không khóa min-ADA.
- **Quyết định liên-nền-tảng còn treo** (cần anh + GreenSun chốt): chọn Settlement hay Mosaic làm đường mặc định, và OriLife 1454/1455 có hội tụ về đây không. Spec này chuẩn bị interface để cả hai cắm vào; KHÔNG tự chốt backend.

---

## §5. Composite Strata + 4 tầng lưu trữ — chi tiết build (bổ sung)

Khái niệm ở `Strata-Feat §6/§7`, toán ở `Strata-Math §12`. Phần này thêm chi tiết đủ để code, **không phá** phần đã có.

### §5.1 Composite — cài đặt bằng state thường (KHÔNG primitive mới)

CompositeStrata = một Strata loại #4 mà state chứa các tham chiếu con. KHÔNG có struct riêng trong code; build bằng `build_state_root` với mỗi con là một trường:

```rust
// Mỗi con = một field (key = role, value = ref_id_con 32B thuần — CHỐT-4).
let fields = vec![
    (b"profile".to_vec(),  ref_id_profile.to_vec()),   // con #4
    (b"posts".to_vec(),    ref_id_posts.to_vec()),     // con #2
    (b"counters".to_vec(), ref_id_counters.to_vec()),  // con #3
];
let parent = StrataChain::genesis(gen_ref_id_raw(&did, &nonce),
                 signed_v0_with(build_state_root(&fields)), &policy)?;
```

- Thêm/bớt con = `append_version` mới của Strata cha với `state_root` mới (INV-E1/E2 nguyên).
- **Proof hai tầng** (`composite_two_tier_proof`, test #16): `prove_field(parent_fields, role)` trả `value == ref_id_con` (tầng cha) → rồi `prove_version`/`prove_field` BÊN TRONG Strata con (tầng con). Verifier nối: gốc tầng con khớp `value` field-proof cha trả về. Mỗi tầng `O(log)`.
- Đệ quy: con của cha có thể lại là composite.

Ba ví dụ build (khớp Feat §6):

| Đối tượng | Cha (#4) state fields | Con |
|---|---|---|
| **Nhóm chat** | `{channel_<id> → ref_id}` mỗi kênh | mỗi kênh = Strata #2 (append-only); metadata nhóm = #3 |
| **Bảng tabular** | `{row_<id> → ref_id}` + `index → ref_id` | mỗi hàng = Strata #4 (cột = field); index = #3 trỏ tập ref_id hàng |
| **Profile MXH** | `{profile, posts, counters}` | profile #4; posts #1/#2; counters #3 (materialize từ #2) |

### §5.2 Bốn tầng lưu trữ — khi nào chuyển tầng (chi tiết triển khai)

Trục 2 (tần-suất/giá-trị) quyết tầng, độc lập loại MECE (Feat §7). Quy tắc triển khai cụ thể:

| Tầng | Trạng thái lưu | Điều kiện VÀO tầng | Điều kiện LÊN tầng trên |
|---|---|---|---|
| **(a) Nóng cục bộ** | `StrataChain`/`AuditLog` trong RAM + WAL local của node tạo | mọi version/event mới, giá trị thấp/tạm | đủ `BatchPolicy.epoch_secs` HOẶC `max_entries` (§5.3) → (b) |
| **(b) Checkpoint gộp lô** | sub-MMR epoch → **một** version checkpoint (`content_cid`=batch CID, `state_root`=sub-MMR root) | cuối epoch / vượt `max_entries` / `flush_max_age` | cần bền (không mất) → (c); cần finality → (d) |
| **(c) Phân tán chọn lọc** | blob version + checkpoint qua Mirage (peer_assignment + repair) | priority ≥ `batch_daily` (Stamp anchor_priority) hoặc dữ liệu cần bền | cần chống rollback on-chain → (d) |
| **(d) Anchor on-chain** | `StrataAnchor` 104B qua AnchorSink §4 | priority `immediate`/`milestone`, hoặc giá trị cao | — |

Nguyên tắc cứng (Feat §7, Stamp-Strata-Mapping §4): **chỉ (c)/(d) theo tầng giá trị** — không đẩy hết lên chuỗi. `no_anchor` dừng (a)/(b).

### §5.3 Checkpoint — khi nào đóng epoch (khớp `BatchPolicy`, Tech §7.3)

```rust
// batch — spec CHỐT; code hợp nhất tại PR #7 (Thịnh).
pub struct BatchPolicy { pub epoch_secs: u64, pub max_entries: u32, pub flush_max_age: u64, pub max_epoch_bytes: u32 }
// default: epoch_secs=3600 (khớp EPOCH_DURATION_SECS Reward), max_entries=10_000 chống RAM
//          phình, flush_max_age=300 — không entry nào chờ quá hạn, max_epoch_bytes=64 MiB.
// profile:  BatchPolicy::proofchat() = { epoch_secs: 600, max_entries: 4096, flush_max_age: 180, max_epoch_bytes: 16 MiB }.
```

Đóng checkpoint khi BẤT KỲ: (1) `now - epoch_start >= epoch_secs`; (2) `entries >= max_entries`; (3) **tuổi entry CŨ NHẤT** trong epoch `>= flush_max_age`. Van (3) KHÔNG phải "im lặng" (đổi từ `flush_on_idle` cũ): chuỗi tin nhịp chậm rả rích vẫn gom được vào MỘT checkpoint, nhưng không entry nào chờ quá `flush_max_age`. Đóng = dựng `mmr_root(sub.leaves)` (CHỐT-3 commit n) → một `append_version`.

> **Addendum S3.1 (gia cố, chốt 08/07 — 4 điểm hai bản implement đầu cùng thiếu):**
> 1. **Blob lô có header**: `serialize_batch = u8(format_version=1) ‖ u32_be(count) ‖ entries…`; parse strict — version lạ / count sai / cụt / thừa byte → `MalformedBatch`. Version byte để nâng format sau không phá parse cũ.
> 2. **`entry_seq` LIÊN TỤC bắt buộc** (per-chain, `prev + 1`): watermark không chỉ "tăng nghiêm ngặt" mà từ chối cả gap — mất entry phải LỘ ra ở điểm ghi, không im lặng. (Gap chủ ý không tồn tại: `entry_seq` do daemon cấp.)
> 3. **Van (c) dùng arrival-time do daemon gán** (`ts` = lúc daemon NHẬN entry), KHÔNG dùng ts client khai — ts giả quá khứ sẽ ép đóng epoch liên tục (checkpoint-per-message DoS). ts nguồn nếu cần thì nằm trong payload, không tham gia quyết định van.
> 4. **Trần RAM theo byte**: thêm `max_epoch_bytes: u32` (van b′ — đóng sớm khi tổng payload vượt; default 64 MiB, profile proofchat 16 MiB) — `max_entries` một mình không chặn được 10k × 1 MiB = 10 GiB.
>
> Lưu ý bảng CHỐT-2: `LN/STRATA/entry/v1` = leaf batch-entry gộp lô (S3, đang dùng); `LN/STRATA/checkpoint/entry/v1` = tag DỰ TRỮ đã chốt ở issue #1 (04/07) cho loại checkpoint-entry tương lai, KHÔNG dùng cho sub-MMR S3 — tránh nhầm hai tag.

Hợp đồng API (code hợp nhất tại PR #7): `EpochAccumulator::new(policy)` → `push(entry_seq, ts, payload, now)` (entry_seq toàn-chain tăng nghiêm ngặt — replay bị từ chối; epoch đầy → `EpochFull`, entry thuộc epoch SAU) → `should_close(now)` → `close()` trả `ClosedEpoch { sub_mmr_root, sub_size, entries, entries_serialized }`. Core THUẦN: `now` do caller truyền (không SystemTime), blob lô + `content_cid` do caller đẩy Mirage. Vòng lặp gọi định kỳ vẫn là việc daemon.

### §5.4 Index derived — rebuild thế nào, truy vấn lịch sử

Nguyên tắc cứng (Tech §7.5): **log = SSoT duy nhất; index = view khả biến, untrusted, một chiều log→index**.

```rust
pub trait DerivedIndex {
    fn replay(log: &VersionLog) -> Self;      // tất định: cùng log → cùng index byte-chính-xác
    fn lookup(&self, q: &Query) -> Vec<Seq>;  // trả VỊ TRÍ; client tự verify bằng InclusionProof
    // KHÔNG có write_back đụng MMR — cấm đường ghi vòng.
}
```

- **Rebuild**: xóa index → `replay(log)` dựng lại từ chuỗi version + MMR. Test bắt buộc `index_replay_root_bit_exact` (Tech §9 test #13): sau replay, `mmr_root` + mọi `state_root` khớp BIT; lệch ⇒ có đường ghi vòng ⇒ lỗi nghiêm trọng.
- **Truy vấn lịch sử** (Math §13, nhóm 100k/3 năm): khung xương checkpoint mỗi giờ (~2,1 MB cho ~26k checkpoint) định vị thời điểm; trong giờ dùng `version_at(t)` (inclusion-proof tới version `ts ≤ t`). Index nóng `(sender, ts) → leaf_seq` cho truy vấn mili-giây, kết quả verify bằng `MmrProof` về `mmr_root` đã neo.
- **[SPEC-TODO]**: `DerivedIndex`/`VersionLog`/columnar engine chưa có code; là khung daemon. Tabular lọc/join chạy trên columnar derived (untrusted), ngưỡng theo đo thực.

---

## §6. Bảng đối chiếu spec ↔ code (đã sửa lệch)

Rà toàn bộ tên hàm/struct trong `Strata-Tech.md` với code thật. Các lệch đã chỉnh (ghi ở đây làm SSoT API; KHÔNG sửa lan man trong Tech để giữ phần đã audit):

| Spec-Tech viết | Code thật | Xử lý |
|---|---|---|
| free-fn `create_strata(author_did, ...)` (§3.1) | `StrataChain::genesis(ref_id, v0, &policy)` (dựng+ký version TRƯỚC) | §2.1 dùng tên + trình tự thật |
| `append_version(strata_ref, prev_version, ...)` free-fn (§3.2) | method `chain.append_version(v, &policy)` `&mut self` | §2.2 |
| struct `StrataRef { ref_id, head_version_hash, mmr_root, ... }` (§1.2) | KHÔNG tồn tại; trạng thái trong `StrataChain`, đọc qua `chain.anchor()` | §1 đính chính |
| `gen_ref_id(...) -> H32` (§2.1) | `gen_ref_id(...) -> String` (bech32m); raw 32B = `gen_ref_id_raw` | §2.1 đính chính |
| `extend_mmr` / `read_head` / `version_at` free-fn (§3.3/§3.5/§3.7) | MMR mở rộng TRONG `append_version`; `chain.head()`; `chain.version_at(t)` | §2.3/§2.4 |
| `FieldProof { value_cid, leaf_idx, merkle_path, version_seq }` (§1.6) | `FieldProof { key, value, fvh, siblings: Vec<(Hash32,bool)>, state_root }` (no `value_cid`/`leaf_idx`/`version_seq`) | §1 + §3 dùng schema code thật |
| `MmrProof { leaf_seq, leaf_hash, mmr_size, merkle_path, peaks }` (§1.6) | `InclusionProof { siblings: Vec<(Hash32,bool)>, peak_index, peaks }` (mmr_size truyền riêng từ `prove_version`) | §1 + §3 |
| `AuditEntry { target_ref_id, subject_hash, action, ts, location_cid, sig }` (§1.6b) | `AuditEntry { created_ts, actor_did, action, signed_hash, location }` (no `target_ref_id`/`sig` trong leaf; ts tên `created_ts`) | §1 + §2.6 |
| `Cid = Vec<u8>` "content_cid thuần" | khớp — `content_cid: Vec<u8>`, `value: Vec<u8>` (CHỐT-4) | ✅ |

Phần KHỚP đúng (không cần sửa): `StrataVersion` (8 trường, thứ tự canonical, CHỐT-1 sig ngoài hash), `StrataAnchor` (4 trường 104B), `Policy.policy_hash()` (cam kết tập author), `build_state_root` (CHỐT-4 hai tầng fvh→leaf, sort key, carry dup-leaf), MMR (CHỐT-2 tag, CHỐT-3 commit n, carry guard), `StrataError` (9 biến thể), domain tags (CHỐT-2 đủ 11 + sum/leaf, sum/node).

---

## §7. Đánh giá độ hoàn chỉnh (trung thực cho anh)

**Đủ-để-Lợi-build NGAY (✅ có code + test pass):**
- 7 thao tác lõi (create/append_version/append_event/read_head/read_at/prove_version/prove_field) + anchor commitment — signature ở §2, schema HTTP + lỗi ở §3.
- Composite (build bằng state thường, proof hai tầng) — §5.1, test `composite_two_tier_proof`.
- ref_id thuần (INV-E5), state_root field-level (INV-E6), audit-log (INV-E3), MMR (INV-E3/E8), anchor đơn điệu (INV-E7), padding/decoy.

**Cần daemon implement (📝 [SPEC-TODO] — spec đủ, code khung chưa có):**
- Lớp HTTP §3 (route `/v1/strata/*` trong `lampnet-node.rs`) — wire core vào axum + key-registry phân giải `Did → pk`.
- `AnchorSink` adapter §4 — interface đã spec; backend mặc định ĐÃ CHỐT = **Settlement** (label 1234, payload CBOR raw + discriminator `t`), Mosaic CIP-68 cho hồ sơ giá trị cao. Code hợp nhất tại PR #6; đường submit production giao VeData vận hành (Strata gửi lô anchor qua intake, xem ranh giới OriLife 06/07). Đã nghiệm thu on-chain Preview: xem `reports/ACCEPTANCE-2026-07-05.md`.
- Vòng gộp epoch §5.3 — lớp checkpoint (`BatchPolicy`/`EpochAccumulator`/verify hai tầng): spec chốt, code hợp nhất tại PR #7; daemon còn phần vòng lặp định kỳ (`now`) + đẩy blob lô qua Mirage.
- `DerivedIndex` + columnar engine §5.4 — khung daemon, untrusted.

**Vẫn cần quyết định (không phải việc code thuần):**
1. **Backend neo mặc định** (Settlement metadata vs Mosaic UTxO) + OriLife 1454/1455 có hội tụ không — liên-nền-tảng, anh + GreenSun.
2. **INV-E4 field-level perm** — ĐÃ CÀI ĐẶT trong crate (`field_policy::FieldPolicy` + `append_version_fielded`); còn lại là quyết định vận hành: mỗi loại Strata (học bạ, sổ bệnh…) chọn bộ entry `(author_did, field_key)` nào — thuộc cấu hình platform, không phải core.
3. **Mirage mode `EncryptedDistributed`** (mã hóa ∧ repair) — Tech §4.3; tới khi có, dữ liệu nhạy cảm CHƯA đạt INV-E9 (mã hóa có, repair chưa). Việc của Mirage, không phải Strata core.

---

## §8. Bổ sung để implement S1/S2/S3 tới HOÀN THÀNH (bàn giao Thịnh)

> Phần này lấp GAP còn lại giữa "spec đủ để hiểu" và "spec đủ để một dev mới code xong không hỏi lại". §1–§7 đủ cho lớp lõi thuần; ba việc bàn giao S1 (anchor→Mosaic CIP-68), S2 (DerivedIndex/columnar), S3 (BatchPolicy/checkpoint) cần chốt thêm: byte-layout on-chain, error-semantics adapter, tham số/domain-tag, tiêu chí test, và ranh giới liên-module. Mọi tên/tag/invariant vẫn theo `_CONTRACT.md`; đây là mở-rộng, KHÔNG sửa phần đã audit.

### §8.0 Đối chiếu `StrataError` — code có 11 biến thể (đính chính §1)

`§1` liệt kê 9 biến thể `StrataError`; **code thật có 11** (`chain.rs:19-44`), thêm hai biến thể field-level và biến thể có **payload struct**. Bảng chuẩn đầy đủ (dùng cho §3.1 và mọi map HTTP):

| Biến thể (payload) | INV | HTTP (bổ sung §3.1) |
|---|---|---|
| `HashLinkBroken { expected: H32, got: H32 }` | E1 | 409 |
| `SeqNotMonotonic { expected: u64, got: u64 }` | E2 | 422 |
| `SeqOverflow` (không payload) | E2 | 422 |
| `BadSignature` | E4 | 403 |
| `PolicyDenied` | E4 | 403 |
| `PolicyHashMismatch { expected: H32, got: H32 }` | E4 | 403 |
| `UnknownAuthor` | E4/CHỐT-5 | 424 |
| `TimestampRegress { prev: u64, got: u64 }` | — | 422 |
| `AnchorRollback { current: u64, attempted: u64 }` | E7 | 409 |
| `FieldPolicyDenied { field_key: Vec<u8> }` | E4 field-level | 403 |
| `FieldProofPolicyMismatch { field_key: Vec<u8> }` | E4 field-level | 403 |
| `ReplaySeq { last: u64, got: u64 }` (batch, §8.3) | E2 | 409 |
| `PayloadTooLarge { len: usize }` (batch, §8.3) | — | 413 |
| `MalformedBatch` (blob lô hỏng, §8.3) | — | 400 |

- Body lỗi HTTP: `{ "error":"<variant>", "detail":{ <các trường payload hex/số> } }`. VD `HashLinkBroken` → `detail:{ "expected":"<hex32>", "got":"<hex32>" }`; `FieldPolicyDenied` → `detail:{ "field_key":"<hex>" }`.
- **Case biên bắt buộc xử lý ở daemon (không phải `StrataError` core, nhưng phải trả lỗi rõ, KHÔNG panic):**
  - `ref`/`seq`/`key` không tồn tại (core trả `None` từ `version`/`prove_version`/`prove_field`/`version_at`) → 404 `{ "error":"NotFound", "detail":{ "what": "ref"|"seq"|"field key"|"version tại t" } }`. (Daemon phân biệt bằng `detail.what`, KHÔNG bằng tên biến thể riêng — code `node/src/error.rs` KHÔNG có `RefNotFound`.)
  - `version_at(t)` với `t < ts(genesis)` → core trả `None` → 404 (KHÔNG 500).
  - Body sai schema / hex sai độ dài (H32 ≠ 64 hex char, sig ≠ 128 hex char) → 400 `{ "error":"MalformedRequest", "detail":{...} }` TRƯỚC khi vào core. (Tên biến thể cửa theo `error.rs`: `MalformedRequest`, `NotFound`, `RefExists`, `AnchorNotConfigured`, `AnchorRejected`, `AnchorNetwork` — KHÔNG phải `BadRequest`.)
  - `state_fields` có `key` trùng → daemon từ chối 400 (core `prove_field` chỉ trả lần xuất hiện đầu sau sort; trùng key = ngữ nghĩa mơ hồ). **Chốt INV key-duy-nhất:** key trong một version PHẢI duy nhất. ⚠️ **Không được chỉ chặn ở daemon** — `build_state_root` (`state.rs`) hiện nhận dup key thản nhiên, nên caller gọi thẳng Rust API ký được version "field X = v1" VÀ "X = v2" cùng sinh proof hợp lệ (equivocation). Core PHẢI enforce: `build_state_root`/`prove_field` reject dup key bằng biến thể lỗi mới `DuplicateFieldKey { field_key }` (E6) — reject KỂ CẢ khi value giống hệt (fail-closed, đơn giản). Phạm vi: chỉ `state_fields`/`build_state_root`; **KHÔNG** áp cho `field_policy::grant()` (dedupe-idempotent ở đó là đúng — ngữ nghĩa QUYỀN khác GIÁ TRỊ). Issue riêng (#39) giao Thịnh — tách khỏi S1/S2/S3.

### §8.1 S1 — `AnchorSink → Mosaic` (CIP-68): byte-layout datum + resolve

Interface trait ở §4.1 đủ hình dạng, nhưng THIẾU ba thứ để code chạy được: (a) byte-layout datum CIP-68 map từ anchor 4 trường; (b) error-semantics đủ mọi case; (c) thuật toán resolve ngược. Chốt dưới đây.

**(a) Map anchor 4 trường → datum CIP-68 (Lựa chọn A, Tech §5.4).** CIP-68 datum là `Constr 0 [ <metadata: Map>, <version: Int>, <extra> ]`. Strata dùng `extra` (field thứ 3, plutus-data tự do) mang anchor; `metadata` map để tối thiểu, `version = 1`:

```
StrataAnchorDatum = Constr 0 [
  metadata : Map [ (b"name", b"LN-STRATA-ANCHOR") ],     // CIP-68 bắt buộc có metadata map
  version  : 1,                                          // CIP-68 datum version
  extra    : Constr 0 [
     ref_id            : Bytes(32),   // anchor.ref_id  — bất biến (INV-E5)
     head_version_hash : Bytes(32),   // anchor.head_version_hash
     mmr_root          : Bytes(32),   // anchor.mmr_root — cam kết lịch sử
     seq               : Int          // anchor.seq (u64 → Int, KHÔNG âm)
  ]
]
```

- **Thứ tự trường trong `extra` CHỐT theo canonical anchor** `(ref_id, head_version_hash, mmr_root, seq)` — đúng thứ tự `StrataAnchor` (`_CONTRACT.md`). Validator đang chạy (`Strata-Tech §5.4`) đọc field thứ 4 của `extra` và kiểm `datum_out.seq == datum_in.seq + 1` (KHÔNG cho gap). ⚠️ Tầng đẩy off-chain hiện cho gap → lệch pha, chưa giải; xem cảnh báo ở §5.4 trước khi dùng đường Mosaic-A.
- **`seq` là `Int`**: Plutus `Int` không giới hạn `u64`; adapter PHẢI reject `seq > u64::MAX` (không xảy ra vì core dùng `u64`) và reject `Int` âm khi resolve về `u64`.
- **Byte-size on-chain:** 3×32 bytes + Int(≤8B) trong `extra` + overhead map metadata ~40B ≈ **~180–200 byte datum** (so 104B commitment thuần) — vẫn nhỏ, đủ min-ADA `1_500_000` lovelace (§4.3). Metadata map cố ý tối thiểu để không đội phí; KHÔNG nhét thêm field nào (giữ INV-E5 — không lộ loại).
- **Backend Settlement (Lựa chọn B, metadata label 1234):** payload KHÔNG phải map field-tên JSON-hex. Mỗi record = **CBOR** `{ "t": <uint discriminator>, "a": [ … ] }` với giá trị **byte thô** (không hex-string). Hai loại record:
  - `t=1` (anchor): `a = [ ref_id: Bytes(32), head_version_hash: Bytes(32), mmr_root: Bytes(32), seq: uint ]` — đúng thứ tự canonical `(ref_id, head_version_hash, mmr_root, seq)`.
  - `t=2` (rotation): `a = [ payload ]` — một bytestring (hoặc mảng chunk khi >64B).
  - **Chunk rule** (giới hạn 64B/mục metadata Cardano): độ dài `≤64B` → MỘT bytestring (cấm chunk); `>64B` → mảng chunk, mọi chunk trừ cuối đúng 64B.
  - **Luật TỪ CHỐI (bắt buộc — nửa âm của hợp đồng).** Fixture hiện chỉ phủ ca **dương** (round-trip hợp lệ), nên một cài đặt pass 8/8 vector vẫn có thể *nhận* encoding không chuẩn tắc — đúng lỗ malleability mà decoder Rust đang chống. Decoder ĐÚNG PHẢI reject:
    1. bytestring đơn lẻ dài `>64B` → `BadChunking`;
    2. mảng chunk có `<2` phần tử → `BadChunking`;
    3. chunk cuối rỗng → `BadChunking`;
    4. map record KHÔNG đúng 2 entry `(t, a)` → `BadShape` (chặn cả khoá trùng);
    5. `t` lạ (không 1, không 2) → **BỎ QUA record, KHÔNG lỗi** (forward-compat có chủ ý).
  - **Nguồn sự thật = `apis/settlement-metadata.json` cho nửa dương** (test-vector CHUNG Rust↔TS, sinh bởi `cargo run --example dump_settlement_fixture`; Rust giữ decoder = ra đề, TS phải khớp) **+ 5 luật từ chối trên cho nửa âm**. Fixture chỉ trở thành nguồn sự thật DUY NHẤT khi bổ sung mục `must_reject` (cbor_hex + lý do) — việc thuộc tầng code/artifact. KHÔNG có metadata map CIP-68 (đó là Lựa chọn A/Mosaic). Consumer PHẢI decode theo `{t,a}` raw, KHÔNG theo map field-tên.

**(b) Error-semantics `AnchorSink` (mở rộng §4.1 `AnchorError`).** §4.1 mới có 3 biến thể; đủ cho case biên cần:

```rust
pub enum AnchorError {
    NotConfigured,                 // backend chưa cấu hình (thiếu key/URL)
    Rejected(String),              // backend/validator từ chối (VD seq' ≤ seq on-chain)
    Network(String),               // lỗi mạng/timeout — RETRYABLE
    RollbackAttempt { on_chain_seq: u64, attempted: u64 },  // INV-E7 backend phát hiện anchor cũ hơn
    DatumTooLarge { bytes: usize }, // datum vượt maxTxSize/protocol param
    InsufficientAda { need: u64, have: u64 }, // backend UTxO (Mosaic A), min-ADA không đủ
}
```

- **Idempotency (bắt buộc):** `publish` cùng một anchor `seq` hai lần (retry sau `Network`) KHÔNG được tạo hai tx spend-recreate. Adapter phải: query on-chain seq hiện tại TRƯỚC khi build tx; nếu `on_chain_seq >= anchor.seq` → trả `Ok(None)` (đã neo) HOẶC `Err(RollbackAttempt)` nếu `on_chain_seq > anchor.seq`. Chốt: `on_chain_seq == anchor.seq` → `Ok(None)` (idempotent no-op); `on_chain_seq > anchor.seq` → `RollbackAttempt`.
- **Phân tầng retryable:** chỉ `Network(_)` retry (backoff). `Rejected`/`RollbackAttempt`/`DatumTooLarge`/`InsufficientAda` là fail cứng — KHÔNG retry, trả lên daemon.
- **INV-E7 hai lớp — mức cross-process KHÁC theo backend (chốt rõ để reader bên-3 không tin quá):**
  - Lớp trong-tiến-trình: core `publish_anchor()` chặn rollback (mọi backend).
  - Lớp cross-process, theo backend:
    - **Settlement legacy (`beacon_policy=None`, quét địa chỉ):** *best-effort*. Bị flood-eviction làm mù (#14) — mù CẢ guard bên GHI (`publish_batch` dùng `resolve`) lẫn bên đọc. Đủ cho publisher-1-ref_id / reader tin daemon; **KHÔNG** đủ cho reader bên-3 không tin publisher.
    - **Settlement + beacon_mode (§8.1(d)):** chống flood (resolve theo asset). Đủ cho bên-3 về **chống-flood**; nhưng đơn-điệu **vẫn dựa khoá publisher** — KHÔNG chống publisher-tự-rollback / key-compromise.
    - **Mosaic A (Plutus validator, `seq' == seq + 1`):** **đã build + chạy Preview** — mã ở `VeDataIO/Code: mosaic/aiken/validators/strata_anchor.ak`, KHÔNG ở repo này (ranh giới "Mosaic giữ validator"). Đây là tier duy nhất **chống-tụt-lùi độc-lập-khoá**: kẻ chiếm khoá publisher cũng không hạ được `seq` on-chain. **Giới hạn phải nói thật:** validator KHÔNG kiểm `mmr_root'` là mở rộng của `mmr_root` (thiếu vế inclusion mà `Strata-Math §7.1` đòi) ⟹ **không** chống *rewrite-then-re-anchor*: kẻ chiếm khoá dựng nhánh lịch sử khác rồi neo tiến lên vẫn qua. Chống rewrite hiện dựa khoá author + ngưỡng operator. Xem `Strata-Tech §5.4` (kèm lệch pha gap off-chain↔on-chain, chưa giải).
  - Cả ba lớp đều phải test riêng.

**(c) Resolve ngược `anchor on-chain → verify mmr_root khớp chain`.** THÊM method vào trait (S1 DoD yêu cầu "proof resolvable on-chain"):

```rust
pub trait AnchorSink {
    fn publish(&self, anchor: &StrataAnchor, priority: AnchorPriority)
        -> Result<Option<AnchorReceipt>, AnchorError>;
    /// Đọc anchor mới nhất on-chain cho một ref_id. None nếu chưa neo bao giờ.
    fn resolve(&self, ref_id: &Hash32) -> Result<Option<StrataAnchor>, AnchorError>;
}
```

Thuật toán verify (daemon-side, sau `resolve`):
```
on_chain = sink.resolve(ref_id)?          // đọc datum/metadata → dựng lại StrataAnchor 4 trường
assert on_chain.ref_id == chain.ref_id     // định danh khớp
assert on_chain.seq   <= chain.head().seq  // on-chain KHÔNG được đi trước local (nếu > → local stale, đồng bộ lại)
// Chứng minh version tại on_chain.seq thuộc lịch sử local, root khớp cái đã neo:
let (proof, vh) = chain.prove_version_at(on_chain.seq, on_chain_mmr_size)?  // TÁI DỰNG ở size cũ
assert vh == on_chain.head_version_hash    // head đã neo == version local tại seq đó
assert StrataChain::verify_version(on_chain.mmr_root, &vh, on_chain.seq, on_chain_mmr_size, &proof)
// LƯU Ý: verify dưới mmr_root ĐÃ NEO (on_chain.mmr_root), với mmr_size TẠI THỜI ĐIỂM neo.
// prove_version_at(seq, mmr_size) TÁI DỰNG MMR từ versions[0..mmr_size] rồi prove — pure, no-I/O,
// cùng họ prove_version, KHÔNG phình state core. mmr_size lấy từ AnchoredTable (seq → mmr_root, mmr_size).
```

> **ĐÃ CHỐT (theo code PR #6):** `prove_version_at(seq, mmr_size)` ĐẶT TRONG CORE (`chain.rs`) — hàm thuần cùng họ `prove_version`, tái dựng MMR ở size lịch sử, KHÔNG phình state; bỏ được ràng buộc timing "record proof TRƯỚC append" của phương án lưu-proof cũ. `mmr_size = seq+1` tại checkpoint. Bảng `AnchoredTable: (ref_id, seq) → (mmr_root, mmr_size, head_version_hash)` là chuẩn DUY NHẤT (thay `anchored: Vec<...>` daemon cũ). `resolve` public trả `Option<StrataAnchor>` (đúng §4.1); backend nội bộ `read_anchor → Vec<ResolvedAnchor>` (mọi UTxO ứng viên kèm asset) để lọc anti-poisoning theo thread-token — KHÔNG được trả "UTxO mới nhất tại address".

**Tiêu chí test S1 (mở rộng DoD Handoff-Issues A/S1):**
1. `map_anchor_to_datum` round-trip: `anchor → datum → parse → anchor'`, assert `anchor == anchor'` (4 trường khớp bit).
2. Preview: neo 1 version thật → tx hash → `resolve` → assert `datum.mmr_root == chain.mmr_root()` + `datum.head_version_hash == chain.head_version_hash()`.
3. **INV-E7 on-chain (Mosaic A):** neo seq=1 → cố neo lại datum seq=0/seq=1 → validator/adapter reject (`RollbackAttempt`). Assert tx thứ hai fail.
4. **INV-E7 idempotent:** gọi `publish(seq=1)` hai lần → lần hai trả `Ok(None)`, KHÔNG tạo tx mới (assert số tx = 1).
5. Resolve sau append: neo seq=1, append tới seq=5 (chưa neo) → `resolve` vẫn trả seq=1; verify proof version seq=1 dưới `on_chain.mmr_root` (size cũ) PASS; version seq=5 chưa neo → không có anchor.
6. `DatumTooLarge`/`InsufficientAda`: mock backend từ chối → adapter trả đúng biến thể, KHÔNG panic.

**(d) Beacon mode (opt-in) — con-trỏ-latest xác thực chống flood-eviction (#14).**
`beacon_policy: Option<String>` trong `SinkConfig` (mặc định `None` = legacy metadata-only, resolve bằng quét địa chỉ — *best-effort*, xem lớp tin cậy ở (b)). Khi bật:
- Beacon = NFT **native**, `policy_id` = native minting policy `sig(publisher)` (KHÔNG Plutus validator — Settlement giữ "no on-chain script"), `assetName = ref_id` (32B), `unit = policy_id ‖ ref_id`.
- **beacon-walk (write):** mỗi anchor, publisher tiêu UTxO đang giữ beacon + gửi beacon sang UTxO mới mang metadata anchor (label 1234, record `{t,a}`). Beacon luôn nằm trên UTxO anchor mới nhất.
- **resolve (read):** `resolve(ref_id)` = tx gần nhất đụng `unit` (asset-index), **O(1)** theo lịch sử (hằng số ~3 request Blockfrost: asset→tx, tx→input-addr, tx→metadata; KHÔNG quét cửa sổ), miễn nhiễm flood; defense-in-depth: đối chiếu `input == publisher`, sai → `Rejected` (fail-closed).
- **Mô hình tin cậy — CHỐT RÕ:** trust root = **khoá publisher**. Beacon-native chống **outsider** (flood, mint giả) NHƯNG **KHÔNG** chống **publisher-tự-rollback / key-compromise**: native policy chỉ chặn *mint lại*, KHÔNG chặn *di chuyển* beacon sang UTxO có `seq` thấp hơn; không validator nên không ép đơn điệu `seq` on-chain. Đơn-điệu độc-lập-khoá là thuộc tính của **Mosaic A** (validator `Strata-Tech §5.4`, đã chạy Preview), KHÔNG phải beacon-native — và ngay ở Mosaic A cũng chỉ là *chống-tụt-lùi*, không phải *chống-rewrite* (xem (b)).
- **Giới hạn quy mô:** beacon per-ref_id (1 NFT / 1 UTxO / ref_id) phù hợp tới **~10⁷ ref_id/publisher**; lớn hơn (→10¹¹) min-ADA khoá vượt tổng cung ADA → cần **aggregated-root** (1 cây commit nhiều ref_id, proof O(log N)) — roadmap, ngoài S1.

### §8.2 S2 — `DerivedIndex` / columnar query: kiểu đủ để code + FieldProof xuyên version

§5.4 nêu trait `DerivedIndex` nhưng để `VersionLog`, `Query`, `Seq` **chưa định nghĩa** — dev mới không code được. Chốt kiểu tối thiểu:

```rust
pub type Seq = u64;
/// Log = SSoT. View chỉ-đọc của chuỗi version (daemon cấp từ StrataChain).
pub trait VersionLog {
    fn len(&self) -> u64;
    fn version(&self, seq: Seq) -> Option<&StrataVersion>;   // đọc canonical
    fn mmr_root(&self) -> Hash32;
    /// Field-value tại một version (đọc từ state_fields đã lưu kèm version off-chain).
    fn field_value_at(&self, seq: Seq, key: &[u8]) -> Option<Vec<u8>>;
    /// Liệt kê field-key tại một version (CẦN cho replay tất định — enumerate cột).
    /// Thứ tự/dup KHÔNG đảm bảo; replay tự sort+dedup. (Bổ sung theo code PR #5.)
    fn field_keys_at(&self, seq: Seq) -> Vec<Vec<u8>>;
}
pub enum Query {
    FieldEquals { key: Vec<u8>, value: Vec<u8> },  // "field X == v" xuyên version
    FieldLatest { key: Vec<u8> },                  // giá trị mới nhất của field X (head)
    SenderRange { sender: [u8;32], from_ts: u64, to_ts: u64 }, // index nóng (Math §13)
}
```

**FieldProof xuyên version (GAP cốt lõi S2 — chưa có trong §2.5).** `prove_field` hiện chỉ chứng minh field trong `state_root` của MỘT tập fields (một version). S2 cần "value của field X tại version k, có proof về `state_root` đã ký + version k thuộc lịch sử". Đây là **proof hai tầng ghép sẵn** (KHÔNG primitive mới):

```
query "field X tại version k" trả:
  1. FieldProof (state.rs::prove_field)  → chứng minh (X, value) ∈ state_root(v_k)
  2. state_root(v_k) chính là StrataVersion.state_root của v_k (đọc canonical, đã băm vào version_hash)
  3. InclusionProof (chain.prove_version(k)) → chứng minh v_k ∈ mmr_root đã neo
Verifier ghép: verify_field_proof(fp) ∧ fp.state_root == v_k.state_root ∧ verify_version(root, vh_k, k, size, ip)
```

- **Chốt:** engine KHÔNG cần commitment mới; nó chỉ **ghép** `FieldProof` + `InclusionProof` sẵn có. `lookup` trả `Vec<Seq>` (vị trí), client tự lấy hai proof rồi verify. INV-E6 (field-privacy) giữ nguyên vì mỗi proof độc lập.
- **Ranh giới ghi vòng (INV cứng §7.5):** columnar engine CHỈ đọc `VersionLog`; KHÔNG có method chạm `Mmr::append`. Trait cố tình không có `write_back`.

**Tham số/ngưỡng (chốt để test có số):**
- `replay` phải **tất định**: cùng `VersionLog` → cùng index byte-chính-xác (test `index_replay_root_bit_exact`, §5.4).
- Ngưỡng "khi nào bật columnar vs full-scan": theo đo thực, KHÔNG hard-code; nhưng test benchmark PHẢI báo cáo query-time vs số version tại `n ∈ {1e2, 1e3, 1e4, 1e5}`.

**Tiêu chí test S2:**
1. `query_field_at_version`: query field `X` ở version k → ghép proof → verify về `mmr_root`. PASS.
2. `oracle_vs_bruteforce`: kết quả `lookup(FieldEquals{X,v})` KHỚP full-scan tuyến tính trên toàn log (đối chiếu tập `Seq`).
3. `index_replay_root_bit_exact`: xóa index → `replay(log)` → mọi `state_root` + `mmr_root` khớp BIT (chốt chặn ghi-vòng).
4. `field_privacy_preserved`: proof field `X` tại version k KHÔNG chứa key/value field khác (tái dùng assert `state.rs::field_proof_no_leak_other_fields`, mở rộng xuyên version).
5. `benchmark_query_scaling`: bảng query-time theo `n` (số thật, không assert ngưỡng cứng).

### §8.3 S3 — `BatchPolicy` / checkpoint sub-MMR: thuật toán đóng epoch + inclusion hai tầng

§5.3 + Tech §7.2-7.3 có `BatchPolicy` struct + điều kiện đóng epoch, nhưng THIẾU: (a) sub-MMR dựng từ primitive nào; (b) entry-bytes canonical; (c) verify inclusion hai tầng cụ thể. Chốt:

**(a) Sub-MMR = `lampnet_merkle_anchor::mmr::Mmr<Blake3Hasher>` — KHÔNG primitive mới.** `Mmr::append(leaf_data)` tự băm `leaf_hash(TAG_LEAF, leaf_data)` nội tại (`mmr.rs:60`). Entry vào sub-MMR:

```rust
// entry_bytes canonical (chốt — length-prefixed, chống ambiguity như version.rs §1.7):
// entry_bytes = u64_be(entry_seq) ‖ u32_be(len(payload)) ‖ payload
// (payload = giá trị đo / CRDT-op serialize tất định)
let mut sub = Mmr::<Blake3Hasher>::new();
// Phủ đầu H_dom("LN/STRATA/entry/v1", ·) TÁCH MIỀN entry khỏi version-hash (phương án (1),
// xem blockquote dưới) RỒI mới append; Mmr::append còn bọc thêm leaf_hash(TAG_LEAF, ·).
for e in epoch_entries { sub.append(&h_dom("LN/STRATA/entry/v1", &entry_bytes(e))); }
let checkpoint_state_root = sub.root();                    // commit n (CHỐT-3) sẵn trong Mmr::root
```

> **Domain-tag — ĐÃ CHỐT phương án (1):** sub-MMR entry băm PHỦ ĐẦU bằng `H_dom("LN/STRATA/entry/v1", entry_bytes)` RỒI mới `sub.append(hashed)` — miền entry tách khỏi miền version-hash (một entry KHÔNG được nhầm là một version-hash), BẢO TOÀN `entry/v1` trong bảng CHỐT-2. (Code hợp nhất tại PR #7.)

**(b) Checkpoint = một `append_version` bình thường:**
```
content_cid = gen_content_cid(batch_entries_serialized)   // batch đẩy qua Mirage
state_root  = checkpoint_state_root (sub-MMR root ở trên)
→ chain.append_version(v_checkpoint, &policy)              // v_checkpoint như mọi version khác
```
Không có API MỚI ở lớp chain; checkpoint đi qua `append_version` bình thường. Lớp gộp (`BatchPolicy` + `EpochAccumulator` + verify hai tầng) thuộc core thuần (§5.3, code hợp nhất tại PR #7); vòng lặp gọi định kỳ + đẩy blob Mirage vẫn là **lớp daemon**.

**(c) Inclusion hai tầng (verify một entry thuộc checkpoint đã neo):**
```
tầng dưới: sub_proof = sub_mmr.prove(entry_index)
           verify(checkpoint_state_root, entry_leaf_hash, entry_index, sub_size, sub_proof)
tầng trên: (ver_proof, size, vh) = chain.prove_version(checkpoint_seq)
           verify_version(anchored_mmr_root, vh, checkpoint_seq, size, ver_proof)
ghép:      verify_two_tier(&v_checkpoint, ...) — nhận THẲNG &StrataVersion, hàm TỰ tính
           version_hash(v) và đọc v.state_root; LINK "state_root == checkpoint_state_root"
           thành HỆ QUẢ CẤU TRÚC, không phải phép so hai input prover-khai rời (chốt theo
           code PR #7 — chặt hơn pseudo-code so-sánh cũ).
```
Entry i được chứng minh "∈ checkpoint ∈ lịch sử đã neo" bằng hai `O(log)` proof. **Chốt lưu:** daemon PHẢI giữ sub-MMR leaves của mỗi epoch (hoặc batch blob qua Mirage) để sinh `sub_proof` sau này — nếu vứt leaves sau checkpoint thì mất khả năng prove entry lẻ (chỉ còn prove cả checkpoint). Đây là quyết định lưu-trữ tầng (c)/(d) theo giá trị.

**Tham số `BatchPolicy` (ĐÃ CHỐT — default để test tất định):**
- `epoch_secs = 3600` (khớp `EPOCH_DURATION_SECS` Reward).
- `max_entries = 10_000` (~2,7 entry/giây trong 1 giờ; sub-MMR 10k leaf ≈ 320KB hash trong RAM, chấp nhận được).
- `flush_max_age = 300` giây — đóng khi entry CŨ NHẤT trong epoch đã chờ 300s (thay ngữ nghĩa "im lặng" cũ, xem §5.3).
- ProofChat dùng profile riêng `BatchPolicy::proofchat()` = `{600, 4096, 180}`.

**Ngữ nghĩa đóng epoch (chốt theo code PR #7 — chặt hơn mô tả cũ):**
- **Thứ tự ưu tiên van** khi nhiều van cùng chạm: `MaxEntries` → `EpochElapsed` → `FlushMaxAge` (van cứng-tài-nguyên trước van thời-gian).
- **Hai góc chết fail-closed:** epoch RỖNG (`count == 0`) KHÔNG bao giờ đóng; `max_entries == 0` fail-closed (không đóng lặp vô hạn).
- **`max_entries` enforce TẠI ĐIỂM GHI** (`push` trả `EpochFull`, entry dư thuộc epoch SAU) — không chỉ trong `should_close`.
- **Van (c) đo tuổi entry CŨ NHẤT** bằng `oldest_ts = min(ts)` (không đòi ts đơn điệu), `now.saturating_sub(oldest_ts)` — `saturating_sub` chống clock-skew panic.
- **Chống replay:** watermark `last_entry_seq` tăng nghiêm ngặt, SỐNG XUYÊN `close()`; seq cũ/bằng → `ReplaySeq`, KHÔNG ghi gì.
- **DoS-guard `parse_batch`:** cap `count.min(bytes.len()/12)` TRƯỚC `Vec::with_capacity` (entry tối thiểu 12B) — blob untrusted từ Mirage không ép được alloc khổng lồ. `serialize_batch` cũng phải trả `Result` (không nuốt lỗi `PayloadTooLarge` im lặng làm blob tự-mâu-thuẫn count≠body).

(Bốn điểm byte-layout gia cố — blob header, `entry_seq` liên tục chống gap, van (c) arrival-time, `max_epoch_bytes` — xem **Addendum S3.1** §5.3, đã chốt 08/07. Seq đầu tiên khi watermark = None là **0**.)

**Tiêu chí test S3:**
1. `checkpoint_1000_versions`: gộp 1000 entry → 1 checkpoint → 1 anchor; assert số anchor = 1 (không phải 1000).
2. `prove_entry_in_checkpoint`: prove entry bất kỳ i ∈ [0,1000) thuộc checkpoint; assert `sub_proof` size ~`log2(1000)×32B ≈ 320B` (khớp "640B/1tr version" spec ở quy mô lớn hơn — báo cáo số thật).
3. `two_tier_inclusion`: ghép sub-proof + version-proof → verify về `mmr_root` đã neo. PASS.
4. `close_on_max_entries`: bơm `max_entries+1` entry → assert checkpoint đóng tại `max_entries` (không chờ hết `epoch_secs`).
5. `close_on_flush_max_age`: entry đầu già `flush_max_age` giây → assert checkpoint đóng, DÙ entry mới vẫn rả rích (chứng minh ngữ nghĩa oldest-age ≠ idle).
6. `entry_bytes_canonical`: hai entry khác payload cùng độ dài → leaf khác (length-prefix + entry/v1 tag tách miền).
7. (nếu làm CRDT §7.4) `crdt_deterministic_state_root`: cùng tập op, thứ tự nhận khác → cùng `checkpoint_state_root`.

### §8.4 Ranh giới liên-module (chốt để không code lấn)

| Cạnh | Ai giữ gì | Hợp đồng dữ liệu |
|---|---|---|
| **Strata ↔ Mirage** | Mirage lưu byte (`content_cid`, batch blob, value inline lớn); Strata chỉ giữ CID 32B thuần | `content_cid = gen_content_cid(bytes)` BLAKE3 thuần, KHÔNG class byte (INV-E5/CHỐT-4). Strata gọi Mirage `/internal/fetch/:cid` để lấy byte; KHÔNG băm hộ Mirage. Dữ liệu nhạy cảm cần mode `EncryptedDistributed` (Mirage backlog) → tới khi có, INV-E9 CHƯA đạt. |
| **Strata ↔ Mosaic** | Mosaic giữ tx on-chain + validator INV-E7; Strata giữ logic chain + sinh anchor 104B | Strata gọi `AnchorSink.publish/resolve` (§4.1 + §8.1c); KHÔNG tự dựng tx Cardano trong Strata core. Datum layout §8.1a. |
| **Strata ↔ VeData-Query** | VeData-Query đọc nhanh (columnar/index untrusted); Strata là SSoT (log+MMR) | Query trả `Vec<Seq>` (vị trí); VeData verify bằng `FieldProof`+`InclusionProof` (§8.2). CẤM query ghi ngược MMR (§7.5). VeData dùng `lampnet-merkle-anchor<Sha3>` cho chuỗi RIÊNG (A22), KHÔNG trộn `<Blake3>` của Strata. |
| **Strata ↔ Stamp** | Stamp gán `anchor_priority` (SSoT); Strata theo cadence | `AnchorPriority` 4-enum = Stamp 4-enum (`Immediate/Milestone/BatchDaily/NoAnchor`). Trường `stamp.*` vào `state_fields`, KHÔNG vào định danh (Stamp-Strata-Mapping §4). |

### §8.5 INV-E1..E9 — cách verify (chốt cho mỗi invariant có test)

`_CONTRACT.md` định nghĩa INV; đây là **cách verify** từng cái (để dev biết viết test nào). Đa số đã có test trong crate; ba cái phụ thuộc backend/daemon chưa có test đầy đủ — đánh dấu ⚠.

| INV | Cách verify | Test hiện có |
|---|---|---|
| E1 hash-linked | tamper version quá khứ → `version_hash` đổi → `prev_hash` version sau không khớp | `chain.rs::inv_e1_*` ✅ |
| E2 seq đơn điệu | seq nhảy/lùi → `SeqNotMonotonic` | `chain.rs::inv_e2_*` ✅ |
| E3 append-only | proof cũ verify dưới root MỚI | `chain.rs::inv_e3_old_proof_valid_under_new_root` ✅ |
| E4 quyền+sig | sai khóa/malleable/không-policy/policy_hash-giả → lỗi tương ứng | `chain.rs::inv_e4_*` (V1+V2) ✅ |
| E5 CID không lộ loại | cùng (author,nonce) khác "loại" → cùng ref_id | `refid.rs::ref_id_depends_only_on_hash_not_type_label` ✅ |
| E6 field-privacy | proof field X không chứa key/value field khác | `state.rs::field_proof_no_leak_other_fields` ✅ |
| E7 chống rollback | (core) neo lại seq cũ → `AnchorRollback`; (on-chain) validator/adapter reject | core ✅ `inv_e7_*`; **⚠ on-chain chưa test** (S1 test #3/#4) |
| E8 hashing an toàn | dup-leaf guard (carry, KHÔNG copy) + RFC6962 prefix + domain-sep | `state.rs::single_field_tree` + `mmr_tests.rs::{dup_leaf_guard_no_collision, second_preimage_leaf_vs_node_distinct, inclusion_n3/n7}` (lá lẻ carry, CVE-2012-2459) ✅ đã xác nhận |
| E9 mã hóa+tái-phân-tán nhạy cảm | content nhạy cảm mã hóa (AES-256-GCM) ∧ repair (Mirage `EncryptedDistributed`) | **⚠ CHƯA đạt** — mã hóa có (Vault), repair CHƯA (Mirage backlog). Test E9 đầy đủ BỊ CHẶN tới khi Mirage có mode. `privacy.rs` chỉ cover padding/decoy (chống suy loại qua size), KHÔNG phải E9 đầy đủ. |

> **GAP CẦN CHỐT (không phải code thuần):** (1) E7 on-chain phụ thuộc backend Mosaic/Settlement — chờ chốt backend mặc định (anh + GreenSun, §4.3). (2) E9 đầy đủ phụ thuộc Mirage `EncryptedDistributed` (Mirage backlog) — testnet tạm chấp nhận Vault-local + cảnh báo "no repair", ghi rõ ở DoD dữ liệu nhạy cảm. (E8 đã có test lá lẻ carry trong `mmr_tests.rs` — không còn là gap.)

---

## Liên kết
- `_CONTRACT.md` — khế ước (INV-E1..E9, domain-tag, CHỐT-1..5). Nguồn chuẩn.
- `Strata-Math.md` — toán + chứng minh (MMR §4, state §6, composite §12, gộp lô §8, truy vấn §13, MST §14).
- `Strata-Tech.md` — cài đặt nội bộ + endpoint gốc §6 + Mirage §4 + neo §5.
- `Strata-Feat.md` — tính năng + composite §6 + 4 tầng §7 + audit §10.
- `_shared/Stamp-Strata-Mapping.md` — (D8,D12)→MECE; trường `stamp.*` vào state §5; anchor_priority→cadence §4.
- Code: `lampnet-hivemind/lampnet-strata/src/{version,chain,state,refid,audit,privacy}.rs` + `lampnet-merkle-anchor/src/{hash,mmr,sumtree}.rs`.
