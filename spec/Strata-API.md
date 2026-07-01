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
| **Settlement** (LampNet) | tx metadata **label 1234** `{ ref_id, head_version_hash, mmr_root, seq }` hex (đối chiếu `settle.ts:358` đang dùng 1234 cho `{merkle_root,epoch,...}`); message người-đọc label **674** CIP-20 | Strata cập nhật dày, gộp lô (priority `batch_daily`/`milestone`) — rẻ nhất | `lampnet-settlement/src/settle.ts:344-391` |
| **Mosaic** (VeData/GreenSun) | reference UTxO CIP-68 spend-recreate, validator enforce `seq'==seq+1` on-chain (INV-E7) | giá trị cao cần on-chain state + finality (priority `immediate`) | `Strata-Tech.md §5.2 Lựa chọn A`, `Stamp-Strata-Mapping §4` |

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
| **(b) Checkpoint gộp lô** | sub-MMR epoch → **một** version checkpoint (`content_cid`=batch CID, `state_root`=sub-MMR root) | cuối epoch / vượt `max_entries` / `flush_on_idle` | cần bền (không mất) → (c); cần finality → (d) |
| **(c) Phân tán chọn lọc** | blob version + checkpoint qua Mirage (peer_assignment + repair) | priority ≥ `batch_daily` (Stamp anchor_priority) hoặc dữ liệu cần bền | cần chống rollback on-chain → (d) |
| **(d) Anchor on-chain** | `StrataAnchor` 104B qua AnchorSink §4 | priority `immediate`/`milestone`, hoặc giá trị cao | — |

Nguyên tắc cứng (Feat §7, Stamp-Strata-Mapping §4): **chỉ (c)/(d) theo tầng giá trị** — không đẩy hết lên chuỗi. `no_anchor` dừng (a)/(b).

### §5.3 Checkpoint — khi nào đóng epoch (khớp `BatchPolicy`, Tech §7.3)

```rust
pub struct BatchPolicy { pub epoch_secs: u64, pub max_entries: u32, pub flush_on_idle: u64 }
// default: epoch_secs=3600 (khớp EPOCH_DURATION_SECS Reward), max_entries chống RAM phình,
//          flush_on_idle đóng epoch sớm khi ngừng đo.
```

Đóng checkpoint khi BẤT KỲ: (1) `now - epoch_start >= epoch_secs`; (2) `entries >= max_entries`; (3) im lặng `>= flush_on_idle`. Đóng = dựng `mmr_root(sub.leaves)` (CHỐT-3 commit n) → một `append_version`. **[SPEC-TODO]**: `BatchPolicy` + vòng gộp epoch chưa có trong code `lampnet-strata` (mới có `Mmr` nền hỗ trợ append); cần daemon implement gộp.

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
- `AnchorSink` adapter §4 — chưa có code; interface đã spec, chờ chốt backend (Settlement vs Mosaic) — **quyết định liên-nền-tảng, cần anh**.
- `BatchPolicy` + vòng gộp epoch §5.3 — core có `Mmr` nền, chưa có lớp checkpoint.
- `DerivedIndex` + columnar engine §5.4 — khung daemon, untrusted.

**Vẫn cần quyết định (không phải việc code thuần):**
1. **Backend neo mặc định** (Settlement metadata vs Mosaic UTxO) + OriLife 1454/1455 có hội tụ không — liên-nền-tảng, anh + GreenSun.
2. **INV-E4 field-level perm** — ĐÃ CÀI ĐẶT trong crate (`field_policy::FieldPolicy` + `append_version_fielded`); còn lại là quyết định vận hành: mỗi loại Strata (học bạ, sổ bệnh…) chọn bộ entry `(author_did, field_key)` nào — thuộc cấu hình platform, không phải core.
3. **Mirage mode `EncryptedDistributed`** (mã hóa ∧ repair) — Tech §4.3; tới khi có, dữ liệu nhạy cảm CHƯA đạt INV-E9 (mã hóa có, repair chưa). Việc của Mirage, không phải Strata core.

---

## Liên kết
- `_CONTRACT.md` — khế ước (INV-E1..E9, domain-tag, CHỐT-1..5). Nguồn chuẩn.
- `Strata-Math.md` — toán + chứng minh (MMR §4, state §6, composite §12, gộp lô §8, truy vấn §13, MST §14).
- `Strata-Tech.md` — cài đặt nội bộ + endpoint gốc §6 + Mirage §4 + neo §5.
- `Strata-Feat.md` — tính năng + composite §6 + 4 tầng §7 + audit §10.
- `_shared/Stamp-Strata-Mapping.md` — (D8,D12)→MECE; trường `stamp.*` vào state §5; anchor_priority→cadence §4.
- Code: `lampnet-hivemind/lampnet-strata/src/{version,chain,state,refid,audit,privacy}.rs` + `lampnet-merkle-anchor/src/{hash,mmr,sumtree}.rs`.
