# Strata — Đặc tả kỹ thuật

**Module**: Strata (Evolving Content Record — "Hồ sơ Tiến hóa") — đề xuất crate `lampnet-strata` (Rust) + sửa `lampnet-codec`, mở rộng `lampnet-mirage`, neo on-chain qua `lampnet-settlement`.

> Đặc tả kỹ thuật cho nhà phát triển. Khế ước giao diện (tên/ký hiệu/invariant) nguồn chuẩn: `_CONTRACT.md`. Mô hình toán + chứng minh: Strata-Math.md. Tính năng + hành trình người dùng: Strata-Feat.md. Mọi mâu thuẫn với `_CONTRACT.md` là lỗi.

Tài liệu này diễn giải bằng lời mọi cấu trúc dữ liệu và mọi thuật toán, để cả người làm kỹ thuật lẫn người không chuyên đọc được.

### Liên kết & tham chiếu
- **Khế ước giao diện**: [`_CONTRACT.md`](./_CONTRACT.md) — nguồn chuẩn INV-E1..E9.
- **API public (cửa platform gọi)**: [`Strata-API.md`](./Strata-API.md) — signature crate thật + request/response HTTP + bảng lỗi + adapter anchor + bảng đối chiếu spec↔code. **Lập trình viên build platform đọc file này.**
- **Đặc tả cùng module**: [Strata-Math.md](./Strata-Math.md) · [Strata-Feat.md](./Strata-Feat.md)
- **Mã nguồn đụng tới**:
  - `lampnet-hivemind/lampnet-codec/src/cid.rs` — `gen_cid` / `gen_cid_v2` / `parse_root_hash` (sửa lỗi CID leak, INV-E5).
  - `lampnet-hivemind/lampnet-mirage/src/protocol/spec.rs` — `DataClass` / `LampUri` (quan hệ Strata ↔ DataClass).
  - `lampnet-hivemind/lampnet-mirage/src/distrecon/mod.rs` — `ShardMeta` / `peer_assignment` (lưu content qua Mirage).
  - `lampnet-hivemind/lampnet-mirage/src/repair/mod.rs` — repair Bulk (chưa mã hóa).
  - `lampnet-hivemind/lampnet-mirage/src/vault/mod.rs` — Vault mã hóa (chưa phân tán/repair).
  - `lampnet-hivemind/lampnet-mirage/src/bin/lampnet-node.rs` — endpoint daemon (style HTTP) + đường rẽ Vault/Bulk.
  - `lampnet-hivemind/lampnet-settlement/src/settle.ts` — metadata CIP-20 label 674 / lampnet label 1234 (anchor on-chain).

---

## Tóm tắt

Strata là **một primitive duy nhất** mô tả mọi dữ liệu thay đổi theo thời gian: tĩnh (1 version), chuỗi-thêm (MMR chính là log), thanh-ghi (đọc head), hồ sơ cấu trúc (state_root field-level + policy). Mỗi Strata có một định danh ổn định `ref_id` (`lnref1…`, opaque, KHÔNG đổi qua các phiên bản, KHÔNG nhúng loại/độ nhạy). Mỗi cập nhật là một `StrataVersion` hash-linked, được gắn vào một **Merkle Mountain Range (MMR)** append-only; mỗi version có `state_root` cho proof từng-trường không lộ trường khác. Nội dung mỗi version (`content_cid`) lưu off-chain qua Mirage theo lớp bảo mật; on-chain chỉ neo `anchor` = `(ref_id, head_version_hash, mmr_root, seq)` — **cam kết lịch sử là `mmr_root` 32 byte**; **anchor là cả 4 trường = 104 byte** (32+32+32+8), đơn điệu theo `seq` (chống rollback). (KHÔNG viết "anchor 32 byte".)

Bốn vấn đề kỹ thuật trọng tâm spec này giải:
1. **Leak loại trong định danh** — CID hiện tại nhúng `DataClass` byte vào payload bech32 (`gen_cid_v2` / `LampUri`). Strata tách: định danh là hash thuần, loại nằm trong state đã commit (INV-E5).
2. **Mirage chưa có mode mã-hóa-VÀ-tái-phân-tán** — Vault mã hóa nhưng dừng sớm, không vào `peer_assignment`/repair; Bulk phân tán + repair nhưng plaintext. Strata yêu cầu CẢ HAI cho dữ liệu nhạy cảm (INV-E9).
3. **Gộp lô tần suất cao** — register/IoT cập nhật mỗi giây sẽ đẻ version vô hạn; Strata dùng sub-MMR theo epoch-checkpoint.
4. **Neo on-chain rẻ + riêng tư** — không nhúng datum kiểu CIP-68 (đắt, lộ hết), chỉ neo anchor 104 byte (cam kết lịch sử = `mmr_root` 32 byte).

---

## §0.5 Phạm vi (đối chiếu crate)

| Crate / file | Strata đụng tới gì | Thay đổi |
|---|---|---|
| **`lampnet-codec` / `cid.rs`** | Sinh định danh + parse hash | Thêm `gen_ref_id` (bech32 `lnref1…`, KHÔNG class byte). `gen_cid_v2`/`LampUri` giữ cho backward-compat nhưng ĐÁNH DẤU deprecated cho định danh công khai (xem §2). |
| **`lampnet-merkle-anchor`** *(crate mới đề xuất)* | Sub-primitive cây băm append-only | MMR + Merkle Sum Tree + inclusion-proof, dup-leaf guard, **hash-agnostic** (`<H: MerkleHash>`). Strata dùng `<Blake3>`; VeData (sau) dùng `<Sha3>`. Một cài-đặt, một audit (§0.5). |
| **`lampnet-types`** *(đề xuất)* | Kiểu dùng chung | `StrataRef`, `StrataVersion`, `StrataAnchor`, `StateField`, `MmrProof`, `FieldProof`, `AuditEntry`, `CompositeStrata`, `PrivacyPadding` (§1). |
| **`lampnet-strata`** *(crate mới đề xuất)* | Lõi vòng đời Strata | `create_strata`, `append_version`, đọc head/version-tại-t, build proof (§3), composite (§12-Math), audit-log (§1.6b). Dùng `lampnet-merkle-anchor<Blake3>` cho MMR/MST. Crate THUẦN (no I/O); orchestrator ngoài làm I/O + index dẫn xuất (§7.5). |
| **`lampnet-mirage` / `distrecon`, `repair`, `vault`** | Lưu content mỗi version | Yêu cầu mode mới `DataClass::Vault` ĐI QUA distribution + repair (hiện chưa có — §4). Không sửa code lần này; spec ra yêu cầu. |
| **`lampnet-mirage` / `lampnet-node.rs`** | Endpoint HTTP | Thêm route `/v1/strata/*` (§6), style khớp `Router::new().route(...)` hiện có. |
| **`lampnet-settlement` / `settle.ts`** | Neo on-chain | Anchor Strata qua metadata label 1234 (đối chiếu label hiện dùng cho `merkle_root`) HOẶC reference UTxO CIP-68 (§5). |

Strata là **tầng TRÊN** Mirage/codec. Mirage vẫn lưu/repair từng shard; Strata chỉ thêm tầng version + MMR + anchor. Loại dữ liệu (Vault/Bulk) nằm trong **state đã commit của Strata**, không trong định danh.

### §0.6 Đặt module — `lampnet-strata` độc lập + sub-primitive `lampnet-merkle-anchor`

Quyết định kiến trúc đã chốt qua phản biện đối kháng về **nơi đặt Strata và tách primitive mật mã**:

**Strata là module ĐỘC LẬP `lampnet-strata` trên Mirage.** KHÔNG gộp Strata vào Mirage. Lý do nguyên lý gốc: Mirage giải bài toán lưu/phân tán/repair byte; Strata giải bài toán định danh↔nội dung↔thời gian. Trộn hai mối quan tâm vào một crate làm cả hai khó kiểm và khó tái dùng. Strata gọi Mirage qua giao diện, không nằm trong Mirage.

**Rút MMR/anchor thành sub-primitive `lampnet-merkle-anchor`, hash-agnostic.** Phần cây băm append-only (MMR, bag-of-peaks, inclusion-proof, Merkle Sum Tree §14-Math) tách thành một crate riêng **tham số hóa hàm băm**:
- Strata dùng `lampnet-merkle-anchor<Blake3>` (BLAKE3 — `_CONTRACT.md`).
- VeData (GreenSun) dùng `lampnet-merkle-anchor<Sha3>` — tham số hóa **sẵn** SHA3 để dùng sau, KHÔNG cài hai lần.

```rust
/// Sub-primitive hash-agnostic. Strata: H = Blake3. VeData (sau): H = Sha3.
pub trait MerkleHash {
    fn leaf(domain: &[u8], data: &[u8]) -> [u8; 32];
    fn node(domain: &[u8], left: &[u8; 32], right: &[u8; 32]) -> [u8; 32];
}
pub struct MerkleAnchor<H: MerkleHash> { /* MMR + MST, append-only, dup-leaf guard */ }
```

**KHÔNG gộp Mirage. KHÔNG gộp VeData Stamp.** Strata và VeData ở **khác hệ sinh thái** (LampNet vs GreenSun), **khác hàm băm** (BLAKE3 vs SHA3). VeData được dùng chung `lampnet-merkle-anchor` **CÓ ĐIỀU KIỆN** — chỉ khi GreenSun cam kết (commitment chính thức), không mặc định.

Lý do tách (ba trục):
- **Khác hệ sinh thái**: Strata thuộc LampNet, VeData/Stamp thuộc GreenSun. Một crate dùng chung phải có chủ rõ ràng; sub-primitive trung lập (`lampnet-merkle-anchor`) là chỗ chung hợp lý, hai module ứng dụng vẫn riêng.
- **Khác hàm băm**: ép một hàm băm cho cả hai là sai — tham số hóa qua trait giữ mỗi bên hàm băm đúng của nó.
- **Một-cài-đặt-một-audit cho primitive mật mã**: cây băm append-only là phần dễ sai và đắt để audit (CVE-2012-2459, second-preimage). Tách thành một crate có **một** cài đặt + **một** lần audit, mọi người dùng (Strata, VeData) thừa hưởng. Cài hai lần = hai bề mặt lỗi, hai lần audit.

| Crate | Vai trò | Hàm băm |
|---|---|---|
| `lampnet-merkle-anchor` | Sub-primitive: MMR + MST + proof, append-only, dup-leaf guard, hash-agnostic | tham số `<H: MerkleHash>` |
| `lampnet-strata` | Lõi vòng đời Strata (version, state_root, composite, audit-log) trên Mirage | dùng `<Blake3>` |
| `lampnet-mirage` | Lưu/phân tán/repair byte (KHÔNG gộp Strata) | — |
| VeData Stamp (GreenSun) | Chuỗi đo lường A22 (KHÔNG gộp Strata) — dùng chung sub-primitive CÓ ĐIỀU KIỆN | sẽ dùng `<Sha3>` |

---

## §1. Cấu trúc dữ liệu

> **Kiểu chuẩn (khớp code thật) ở [`Strata-API.md §1`](./Strata-API.md) + bảng đối chiếu §6.** Một số struct mô tả dưới đây (`StrataRef` §1.2, `MmrProof`/`FieldProof` §1.6, `AuditEntry` §1.6b) là **đề xuất thiết kế ban đầu**; code thật khác tên/trường — đã đính chính ở `Strata-API.md §6` (SSoT). Cụ thể: KHÔNG có struct `StrataRef` (trạng thái sống trong `StrataChain`, đọc qua `chain.anchor()`); `MmrProof` thật là `InclusionProof { siblings: Vec<(Hash32,bool)>, peak_index, peaks }` (`mmr_size` truyền riêng); `FieldProof` thật là `{ key, value, fvh, siblings: Vec<(Hash32,bool)>, state_root }` (KHÔNG `value_cid`/`leaf_idx`/`version_seq`); `AuditEntry` thật là `{ created_ts, actor_did, action, signed_hash, location }` (5 trường, KHÔNG `target_ref_id`/`sig` trong leaf). Giữ nguyên §1 ở đây để bảo toàn phần đã audit; đọc kèm đính chính API §6.

### §1.1 Hằng số + alias

```rust
/// BLAKE3 32 byte — hash nền thống nhất toàn module (INV-E8).
pub type H32 = [u8; 32];

/// Domain tags (ASCII, prefix "LN/STRATA/"). Domain-sep: H_dom(tag, x) = BLAKE3(tag ‖ 0x00 ‖ x).
/// COPY NGUYÊN VĂN bảng domain-tag CHUẨN của _CONTRACT.md (CHỐT-2) — KHÔNG tự đặt tên khác.
pub const TAG_REF:        &[u8] = b"LN/STRATA/ref/v1";          // sinh ref_id
pub const TAG_VER:        &[u8] = b"LN/STRATA/ver/v1";          // băm version (core, KHÔNG sig)
pub const TAG_POLICY:     &[u8] = b"LN/STRATA/policy/v1";       // policy commitment (tập author, INV-E4)
pub const TAG_MMR_LEAF:   &[u8] = b"LN/STRATA/mmr/leaf/v1";     // MMR leaf (RFC6962 0x00)
pub const TAG_MMR_NODE:   &[u8] = b"LN/STRATA/mmr/node/v1";     // MMR internal (RFC6962 0x01)
pub const TAG_MMR_ROOT:   &[u8] = b"LN/STRATA/mmr/root/v1";     // MMR root = bag + commit n (CHỐT-3)
pub const TAG_STATE_FVAL: &[u8] = b"LN/STRATA/state/fval/v1";   // băm giá trị trường
pub const TAG_STATE_LEAF: &[u8] = b"LN/STRATA/state/leaf/v1";   // state leaf (key + fval)
pub const TAG_STATE_NODE: &[u8] = b"LN/STRATA/state/node/v1";   // state internal node
pub const TAG_STATE_PAD:  &[u8] = b"LN/STRATA/state/pad/v1";    // padding giấu số trường (INV-E6)
pub const TAG_ENTRY:      &[u8] = b"LN/STRATA/entry/v1";        // batch entry (sub-MMR gộp lô §7)

/// Phân cách domain-sep (1 byte, tất định).
pub const DOM_SEP: u8 = 0x00;
/// RFC 6962 prefix chống second-preimage / leaf-vs-node (INV-E8).
pub const RFC6962_LEAF: u8 = 0x00;
pub const RFC6962_NODE: u8 = 0x01;

pub type Did   = [u8; 32];   // băm DID người tạo/sửa (BLAKE3 của DID bytes)
pub type Cid   = Vec<u8>;    // content CID — hash thuần BLAKE3 (KHÔNG class byte)
pub type Sig   = [u8; 64];   // Ed25519
pub type Seq   = u64;
```

### §1.2 `StrataRef` — định danh + con trỏ head

```rust
/// Định danh ổn định + trạng thái head của một Strata. ref_id KHÔNG đổi qua các version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrataRef {
    /// Định danh opaque `lnref1…` (bech32). Sinh 1 lần lúc genesis (§2).
    /// KHÔNG nhúng loại/độ nhạy (INV-E5).
    pub ref_id: H32,                 // 32 byte hash thô; biểu diễn bech32 ở lớp hiển thị
    /// DID người tạo (genesis author) — đã băm.
    pub genesis_author: Did,
    /// nonce genesis (32 byte ngẫu nhiên) — tham số sinh ref_id, lưu để tái lập + chống va chạm.
    pub genesis_nonce: H32,
    /// version_hash của head hiện tại.
    pub head_version_hash: H32,
    /// seq của head (đơn điệu — INV-E2/E7).
    pub head_seq: Seq,
    /// mmr_root hiện tại (root trên dãy leaf = version_hash).
    pub mmr_root: H32,
}
```

### §1.3 `StrataVersion` — một nút phiên bản

Thứ tự trường **canonical** đúng `_CONTRACT.md`: `{ seq, prev_hash, content_cid, state_root, author_did, policy_hash, ts, sig }`.

```rust
/// Một phiên bản. version_hash tính trên canonical(version KHÔNG gồm sig); sig ràng buộc riêng (§1.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrataVersion {
    pub seq:         Seq,    // 0 cho genesis, +1 mỗi lần (INV-E2)
    pub prev_hash:   H32,    // version_hash(seq-1); 0^32 nếu seq==0 (INV-E1)
    pub content_cid: Cid,    // CID nội dung off-chain (Mirage). Hash thuần; mã hóa nếu nhạy cảm.
    pub state_root:  H32,    // Merkle root field-level (§1.5) — cho FieldProof (INV-E6)
    pub author_did:  Did,    // DID người sửa (đã băm)
    pub policy_hash: H32,    // hash policy quyền sửa (INV-E4) — ai sửa được field nào
    pub ts:          u64,    // unix secs, đơn điệu không-giảm theo seq (cho "giá trị tại t")
    pub sig:         Sig,    // Ed25519 (canonical low-S) của author trên version_hash (§1.7, CHỐT-1)
}
```

### §1.4 `StrataAnchor` — neo on-chain (4 trường)

```rust
/// Neo on-chain = 104 byte (3×32 + 8). "Cam kết lịch sử" = riêng mmr_root 32 byte.
/// Đặt vào tx metadata HOẶC reference UTxO datum (§5). KHÔNG viết "anchor 32 byte".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrataAnchor {
    pub ref_id:            H32,   // 32 byte
    pub head_version_hash: H32,   // 32 byte
    pub mmr_root:          H32,   // 32 byte
    pub seq:               Seq,   // 8 byte big-endian — đơn điệu (INV-E7)
}
// Tổng on-chain commit: 3×32 + 8 = 104 byte. So CIP-68 datum đầy đủ: lớn/đắt/lộ hết (§5).
```

### §1.5 `StateField` — một trường trong hồ sơ cấu trúc

```rust
/// Một trường (key→value) của hồ sơ cấu trúc (#4). Tập field → state_root qua Merkle (§3.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateField {
    pub key:        Vec<u8>,   // tên trường (UTF-8 bytes), VD b"diagnosis"
    /// field_value_bytes: giá trị INLINE (nhỏ) HOẶC content_cid THUẦN 32B.
    /// CHỐT-4: nếu là CID thì PHẢI là content_cid thuần (gen_content_cid — KHÔNG class byte,
    /// KHÔNG doc_type). FieldProof trả value_cid công khai (§1.6), nên CID có class byte sẽ
    /// leak loại qua field-proof (INV-E5/E6). value_cid = field_value_bytes khi là CID.
    pub value_cid:  Cid,
    /// fvh = H_dom("LN/STRATA/state/fval/v1", field_value_bytes) — băm giá trị trường (CHỐT-4).
    pub fvh:        H32,
    /// leaf = H_dom("LN/STRATA/state/leaf/v1", u32_be(len(key)) ‖ key ‖ fvh) — leaf state tree.
    pub leaf:       H32,
}
```

Quan trọng (INV-E6): leaf của cây state KHÔNG phải value thô. Cấu trúc hai tầng theo CHỐT-4:
`fvh = H_dom(TAG_STATE_FVAL, field_value_bytes)` rồi `leaf = H_dom(TAG_STATE_LEAF, u32_be(len(key)) ‖ key ‖ fvh)`.
Proof một trường lộ `value_cid + fvh + path`; KHÔNG lộ `key`/`value` của trường khác (chỉ lộ sibling hash đã băm).
CHỐT-4 (ràng buộc cứng): `value_cid` công khai trong FieldProof PHẢI là content_cid thuần (`gen_content_cid`,
§2) — nếu nhúng class byte/doc_type sẽ leak loại qua field-proof. Để giấu cả SỐ trường,
state tree dùng leaf padding `TAG_STATE_PAD` — xem Strata-Math.

### §1.6 `MmrProof` + `FieldProof`

```rust
/// Inclusion proof một version trong MMR. Cho "giá trị tại thời điểm t" (proof tới version ts ≤ t).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmrProof {
    pub leaf_seq:    Seq,        // seq của version cần chứng minh
    pub leaf_hash:   H32,        // version_hash của version đó
    pub mmr_size:    u64,        // số leaf của MMR tại thời điểm chứng (root tương ứng)
    pub merkle_path: Vec<H32>,   // sibling hashes từ leaf lên peak
    pub peaks:       Vec<H32>,   // các peak còn lại để bag thành mmr_root
}

/// Proof một trường từ state_root của một version (INV-E6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldProof {
    pub key:         Vec<u8>,    // trường được chứng minh
    pub value_cid:   Cid,        // content_cid THUẦN (CHỐT-4 — KHÔNG class byte; verifier fetch + so fvh)
    pub fvh:         H32,        // H_dom(TAG_STATE_FVAL, field_value_bytes) — băm giá trị trường
    pub leaf_idx:    usize,      // vị trí trong cây state (đã sort theo key)
    pub merkle_path: Vec<H32>,   // sibling hashes (TAG_STATE_NODE) lên state_root
    pub state_root:  H32,        // gắn với một version cụ thể
    pub version_seq: Seq,        // version chứa field này
}
```

### §1.6b `AuditEntry` — một mục nhật ký truy cập bất biến

Mỗi object nhạy cảm gắn một **audit-log** = một Strata loại #2 (append-only) riêng (Strata-Feat §10). Mỗi lần truy cập hoặc lần ký sinh một `AuditEntry`, append vào log đó (một entry = một leaf trong MMR của audit-log, hoặc một version nếu cần neo riêng). Bất biến kế thừa INV-E3 (append-only) + INV-E1/E2 (hash-linked).

```rust
/// Một mục audit append-only. Ghi NĂM chiều: tạo-khi-nào, ai, khi-nào, ký-cái-gì, ở-đâu.
/// Là leaf của audit-log Strata #2 — không sửa/xóa được (INV-E3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    /// ref_id của object nhạy cảm được truy cập/ký (con trỏ tới Strata mục tiêu).
    pub target_ref_id:  H32,
    /// "Ký cái gì" — version_hash HOẶC content_cid của thứ được truy cập/ký.
    pub subject_hash:   H32,
    /// "Ai" — DID người thực hiện (đã băm; Did → pubkey qua key-registry, CHỐT-5).
    pub actor_did:      Did,
    /// Loại sự kiện: tạo / đọc / ký / chia-sẻ-proof …
    pub action:         AuditAction,
    /// "Khi nào" — unix secs của lần truy cập/ký (đơn điệu không-giảm trong log).
    pub ts:             u64,
    /// "Ở đâu" — commitment vị trí/ngữ cảnh qua Compass (hash thuần, KHÔNG lộ toạ độ thô).
    pub location_cid:   H32,
    /// Chữ ký của actor trên hash của entry (Ed25519 canonical low-S) — không chối được.
    pub sig:            Sig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditAction { Create, Read, Sign, ShareProof, Update }
```

Mỗi `AuditEntry` được băm (domain-sep, RFC6962 leaf) thành leaf của audit-log Strata. "Tạo khi nào" của bản thân object = entry `Create` đầu chuỗi; mọi `Read`/`Sign` sau là entry tiếp theo. Bên kiểm toán chứng minh "actor Z đã truy cập object lúc T" bằng inclusion-proof của entry dưới `mmr_root` audit-log đã neo — không cần tin máy chủ. `location_cid` là **commitment** qua Compass, không phải toạ độ thô (giữ riêng tư vị trí; lộ chọn lọc khi cần).

### §1.7 Canonical encoding rule (tái lập băm tất định)

Mọi băm phải tái lập bit-chính-xác trên mọi máy. Quy tắc:

1. **Thứ tự trường tất định**: đúng thứ tự khai báo trong `_CONTRACT.md` (§1.3). KHÔNG dựa vào thứ tự serde mặc định; viết encoder tay (`canonical_version_bytes`).
2. **Endianness**: mọi số nguyên `u64` → **big-endian 8 byte cố định** (`to_be_bytes`). Khớp convention `epoch.to_be_bytes()` đã dùng trong reward merkle leaf (`allocate.rs`).
3. **Độ dài thay đổi (`content_cid`, `key`)**: prefix bằng **độ dài u32 big-endian** rồi tới bytes — length-prefixed, chống ambiguity nối chuỗi (canonical encoding rule). **Trần cứng (hợp đồng, đồng bộ Spectra §1.9):** mỗi trường length-prefix `< 2³²` byte và mỗi danh sách count-prefix `< 2³²` phần tử. Input **vượt trần = fail-loud**: `u32_be` chặn bằng `assert!` (chạy CẢ release, KHÔNG phải `debug_assert!`) — thà panic tại chỗ còn hơn để `n as u32` truncate lặng lẽ → prefix cụt → **mất song ánh** → hai input khác cho cùng byte → `H_dom` trùng, vỡ tất định băm. `u32_be` là **van duy nhất** mà mọi length/count-prefix trong crate đi qua — hash-canonical (`version::content_cid`, **`state::leaf_hash`** — tiền tố `len(key)` của state leaf, chỗ INV-E6 field-proof đứng, `field_policy::field_key`, `audit`) **và** đường persist (`anchor_sink::AnchoredTable`, **count-prefix** `u32_be(entries.len())` trong `batch::serialize_batch`); riêng **payload length** trong `batch::entry_bytes` guard graceful trước bằng `PayloadTooLarge` (đường `Result`) nên không chạm assert — **chỉ payload length graceful, count-prefix của cả blob thì không**. Ở đường persist, hậu quả của truncate là parse strict trả `None` (mất bảng đã lưu) chứ không phải va chạm `H_dom` — vẫn chặn để không prefix nào nằm ngoài hợp đồng. Về mặt kiểu, `content_cid`/`key` là `Vec<u8>` nên trần phải là **hợp đồng tường minh** dù ngữ nghĩa `content_cid` = BLAKE3 32B.
4. **Trường cố định** (`H32`, `Did`, `Sig`): ghi nguyên 32/64 byte, KHÔNG length-prefix.
5. **KHÔNG đưa `sig` vào `version_signing_bytes`** (vì sig ký trên chính bytes đó).

```rust
/// Bytes ký + bytes để băm version (KHÔNG gồm sig).
fn canonical_version_bytes(v: &StrataVersion) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&v.seq.to_be_bytes());            // u64 BE
    b.extend_from_slice(&v.prev_hash);                    // 32B
    b.extend_from_slice(&(v.content_cid.len() as u32).to_be_bytes()); // len-prefix
    b.extend_from_slice(&v.content_cid);
    b.extend_from_slice(&v.state_root);                   // 32B
    b.extend_from_slice(&v.author_did);                   // 32B
    b.extend_from_slice(&v.policy_hash);                  // 32B
    b.extend_from_slice(&v.ts.to_be_bytes());             // u64 BE
    b   // KHÔNG sig
}

/// version_hash = H_dom(TAG_VER, canonical_version_bytes) — và sig được verify riêng (INV-E4).
fn version_hash(v: &StrataVersion) -> H32 {
    h_dom(TAG_VER, &canonical_version_bytes(v))
}

fn h_dom(tag: &[u8], x: &[u8]) -> H32 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(tag);
    hasher.update(&[DOM_SEP]);   // 0x00
    hasher.update(x);
    *hasher.finalize().as_bytes()
}
```

**CHỐT-1** (nguồn chuẩn `_CONTRACT.md`): `version_hash` KHÔNG trộn `sig`. `sig = Ed25519_sign(sk_author, version_hash(v))` ký TRÊN version_hash, KHÔNG băm sig vào hash. Tamper-evidence của sig đến từ yêu cầu chữ ký canonical, không từ việc băm sig — giữ version_hash thuần để inclusion-proof độc lập với chữ ký. KHÔNG được claim "đổi sig đổi version_hash".

Yêu cầu chữ ký:
- **Canonical Ed25519 (low-S)** bắt buộc (`_CONTRACT.md` dòng 16): scalar S phần thấp của curve order — chống signature malleability (một version không có hai sig hợp lệ khác nhau). Verifier reject sig non-canonical (S ≥ L/2).
- **CHỐT-5 — Did → pubkey**: `author_did` là `[u8;32]` (băm DID PhoenixKey), KHÔNG phải pubkey. Verify cần ánh xạ `Did → pubkey` qua key-registry của lampnet-join/PhoenixKey. KHÔNG giả định `Did == pubkey`. Verify: `pk = registry.resolve(author_did)?; Ed25519_verify(pk, version_hash(v), sig)` với check low-S (INV-E4).

---

## §2. Định danh `ref_id` — sửa lỗi CID leak (INV-E5)

### §2.1 Yêu cầu

`ref_id` là **hash thuần**, opaque, ổn định, KHÔNG nhúng nhãn loại/độ nhạy. Sinh từ:

```
ref_id = H_dom("LN/STRATA/ref/v1", author_did ‖ genesis_nonce)
```

`author_did` (32B băm DID) ‖ `genesis_nonce` (32B ngẫu nhiên) → BLAKE3 domain-sep → 32 byte. Biểu diễn công khai bech32 HRP `lnref`: `lnref1<bech32(ref_id)>`.

```rust
/// Sinh ref_id thô (32B) — THUẦN, không class byte, không nhãn loại (INV-E5).
pub fn gen_ref_id(author_did: &Did, genesis_nonce: &H32) -> H32 {
    let mut x = Vec::with_capacity(64);
    x.extend_from_slice(author_did);
    x.extend_from_slice(genesis_nonce);
    h_dom(TAG_REF, &x)
}

/// Biểu diễn bech32 cho hiển thị/URL: HRP "lnref", payload = 32B ref_id (KHÔNG version/class byte).
pub fn encode_ref_id_bech32(ref_id: &H32) -> String { /* bech32m, charset như cid.rs */ }
```

### §2.2 Lỗi hiện tại trong `cid.rs` / `spec.rs`

Định danh công khai hiện **nhúng loại**:

- `gen_cid_v2(data, class)` (`cid.rs:78`) dựng `payload = vec![class]; payload.extend(hash)` → **byte đầu payload là DataClass** (0x01 Vault / 0x02 Bulk), rồi bech32. → URI `lamp://ln1…` LỘ độ nhạy: ai thấy CID biết ngay dữ liệu là "secret/encrypted" hay "media".
- `LampUri` (`protocol/spec.rs:52-57`): `struct { class: DataClass, hash }`, `to_payload()` (`spec.rs:125`) `payload.push(self.class.to_u8())` rồi hash → cùng leak. `PAYLOAD_BYTES = 33` = 1 class + 32 hash (`spec.rs:19`).
- `gen_cid` legacy (`cid.rs:66`) nhúng `safe_doc_type` vào CID text (`ln1q_<hash>_<safe_name>`) → leak tên loại tài liệu dạng plaintext.
- `parse_root_hash` (`cid.rs:190`) phản chiếu leak: nó **strip 1 byte class** (`bytes[1..33]`, `cid.rs:233`) — giả định mọi CID có class prefix.

### §2.3 Thay đổi cụ thể so với `gen_cid_v2`

| Khía cạnh | `gen_cid_v2` hiện tại | `gen_ref_id` (Strata) |
|---|---|---|
| Đầu vào sinh | `blake3(data)` (content) | `H_dom(TAG_REF, author_did ‖ nonce)` (định danh ổn định, KHÔNG phụ thuộc content → bất biến qua version) |
| Payload | `[class] ‖ hash` (33B) | `ref_id` thuần (32B), KHÔNG class |
| Leak loại | CÓ (byte đầu = DataClass) | KHÔNG (INV-E5) |
| Ổn định qua version | KHÔNG (CID đổi theo content) | CÓ (ref_id genesis-fixed) |
| HRP | `ln` (`lamp://ln1…`) | `lnref` (`lnref1…`) — tách namespace định danh-Strata khỏi content-CID |

`content_cid` của từng version cũng phải là **hash thuần** (bỏ class byte). Đề xuất: thêm `gen_content_cid(data) -> Cid = blake3(data)` (32B, no prefix). Loại/độ nhạy của content KHÔNG nằm trong CID mà nằm trong `state_root` của version (field `data_class` đã commit) + policy lưu trữ. `gen_cid_v2`/`LampUri` GIỮ cho backward-compat decode dữ liệu cũ, nhưng KHÔNG dùng cho định danh mới → đánh dấu `#[deprecated(note = "leaks DataClass — dùng gen_ref_id/gen_content_cid (INV-E5)")]`.

`parse_root_hash` cần một nhánh "no-class": với `lnref1…`, trả 32 byte nguyên (KHÔNG `bytes[1..33]`). Giữ nhánh `bytes[1..33]` chỉ cho `lamp://ln1…` legacy.

---

## §3. Vòng đời

> **API chuẩn (tên hàm thật) ở [`Strata-API.md`](./Strata-API.md).** Pseudo-code §3.1–§3.7 dưới đây mô tả LUỒNG; tên hàm/struct thật trong code khác (vd KHÔNG có free-fn `create_strata` — thật là `StrataChain::genesis(ref_id, v0, &policy)` + method `chain.append_version`; KHÔNG có struct `StrataRef` — trạng thái trong `StrataChain`). Bảng đối chiếu spec↔code đầy đủ: `Strata-API.md §6`.

### §3.1 `create_strata` — genesis (seq=0)

```
create_strata(author_did, genesis_nonce, content_cid, state_fields, policy_hash, ts, sign_fn):
  ref_id = gen_ref_id(author_did, genesis_nonce)
  state_root = build_state_root(state_fields)          // §3.6
  v0 = StrataVersion {
     seq: 0, prev_hash: [0u8;32], content_cid, state_root,
     author_did, policy_hash, ts, sig: [0u8;64],       // tạm
  }
  vh0 = version_hash(&v0)
  v0.sig = sign_fn(vh0)                                  // Ed25519 canonical low-S (§1.7)
  mmr = Mmr::new(); mmr.append(mmr_leaf_hash(vh0))      // §3.4
  StrataRef {
     ref_id, genesis_author: author_did, genesis_nonce,
     head_version_hash: vh0, head_seq: 0, mmr_root: mmr.root(),   // mmr.root() = mmr_root() §3.4 (commit n)
  }
```

Loại #1 (Tĩnh) = một Strata dừng ở đây (1 version). `content_cid` trỏ tới video/PDF/release đã đẩy qua Mirage.

### §3.2 `append_version` — thêm phiên bản (kiểm INV-E1/E2/E4)

```
append_version(strata_ref, prev_version, new_content_cid, new_state_fields,
               author_did, policy_hash, ts, sign_fn, verify_policy_fn) -> StrataVersion:

  // INV-E2: seq tăng đúng +1
  seq = prev_version.seq + 1                            // overflow-check; lỗi nếu == u64::MAX

  // INV-E1: hash-linked
  prev_hash = version_hash(&prev_version)
  assert prev_hash == strata_ref.head_version_hash          // chống fork: chỉ nối từ head

  // INV-E4: quyền + chữ ký
  assert verify_policy_fn(author_did, policy_hash, &new_state_fields)
      // policy_hash cho phép author_did sửa đúng các field thay đổi; nếu sai → Err(PolicyDenied)

  state_root = build_state_root(new_state_fields)
  v = StrataVersion { seq, prev_hash, content_cid: new_content_cid, state_root,
                   author_did, policy_hash, ts, sig: [0;64] }
  vh = version_hash(&v)
  v.sig = sign_fn(vh)                                    // ký bởi author (INV-E4 phần sig)
  v   // caller gọi extend_mmr + cập nhật StrataRef
```

Kiểm tra bắt buộc trước khi chấp nhận (validator off-chain): `prev_hash` khớp head (chống nhánh song song), `seq == head_seq+1` (INV-E2), `ts >= prev.ts` (đơn điệu thời gian — cho "giá trị tại t"), sig hợp lệ + policy cho phép (INV-E4), và `v.policy_hash == policy.policy_hash()` (chống commit bộ quyền giả — Mệnh đề 6).

> **INV-E4 — hai chế độ (đã cài đặt)**:
> - **V1 mức-chain**: `chain::Policy` cam kết tập author; `chain.rs::check_auth` kiểm tập author + khớp `policy_hash`. `append_version` / `genesis`.
> - **V2 field-level**: `field_policy::FieldPolicy` cam kết cây Merkle các entry `(author_did, field_key)`; khi sửa trường, author kèm `FieldAuthProof` (Merkle-proof entry dưới `policy_hash`). `chain.rs::check_auth_fielded` kiểm `policy_hash` khớp + sig + MỖI trường thay đổi có bằng chứng quyền hợp lệ của chính author. API: `append_version_fielded` / `genesis_fielded`. Lỗi: `FieldPolicyDenied { field_key }`, `FieldProofPolicyMismatch { field_key }`.

### §3.3 `extend_mmr` + cập nhật head

```
extend_mmr(mmr, version):
  vh = version_hash(&version)
  mmr.append(mmr_leaf_hash(vh))       // append-only — KHÔNG sửa leaf cũ (INV-E3)
  new_root = mmr_root(mmr.leaves)     // §3.4: commit u64_be(n) ‖ bag (CHỐT-3)
  // INV-E3: mọi inclusion-proof cũ vẫn đúng dưới new_root (MMR chỉ MỞ RỘNG, peaks bag lại)
  update StrataRef { head_version_hash: vh, head_seq: version.seq, mmr_root: new_root }
```

### §3.4 Build/extend MMR (INV-E8: domain-sep + RFC6962 + dup-leaf guard)

MMR = dãy "núi" hoàn hảo (perfect binary trees) theo binary của số leaf; root = "bag of peaks" (băm các peak từ phải sang trái).

```rust
fn mmr_leaf_hash(version_hash: &H32) -> H32 {
    // RFC6962 leaf prefix 0x00 + domain-sep (TAG_MMR_LEAF)
    h_dom(TAG_MMR_LEAF, &[&[RFC6962_LEAF], &version_hash[..]].concat())
}
fn mmr_node_hash(left: &H32, right: &H32) -> H32 {
    // RFC6962 internal prefix 0x01 — chống leaf-vs-node second-preimage (TAG_MMR_NODE)
    h_dom(TAG_MMR_NODE, &[&[RFC6962_NODE], &left[..], &right[..]].concat())
}

/// mmr_root COMMIT số lá n (CHỐT-3) — bag peaks rồi domain-hash với u64_be(n).
/// Củng cố dup-leaf guard: hai dãy KHÁC độ dài → root KHÁC kể cả khi bag trùng.
fn mmr_root(leaves: &[H32]) -> H32 {
    let n = leaves.len() as u64;
    let peaks = perfect_peaks(leaves);                 // các peak theo binary của n
    let bag = bag_of_peaks(&peaks);                    // fold mmr_node_hash từ peak phải nhất
    h_dom(TAG_MMR_ROOT, &[&n.to_be_bytes()[..], &bag[..]].concat())   // u64_be(n) ‖ bag
}
```

**Dup-leaf guard (CVE-2012-2459)**: KHÔNG nhân đôi leaf cuối khi số leaf lẻ. MMR xử lý leaf lẻ bằng **carry** (leaf đơn trở thành peak riêng), rồi bag các peak — không copy leaf. Đây là khác biệt cốt lõi so với `merkle.rs` của Reward (`build_root` nhân đôi lá cuối, `Reward-Tech §4.9`): Strata dùng MMR carry để (a) tránh CVE-2012-2459, (b) cho proof cũ vẫn đúng khi mở rộng (INV-E3).

**CHỐT-3 — commit n**: `mmr_root = H_dom("LN/STRATA/mmr/root/v1", u64_be(n) ‖ bag_of_peaks)`. KHÔNG dừng ở `bag_peaks(peaks)` (sai — thiếu commit n): phải băm thêm với số lá `n` để hai dãy khác độ dài cho root khác nhau (lớp guard thứ hai chống forge). Tất định: chỉ phụ thuộc dãy leaf theo thứ tự seq + n (không sort — thứ tự là chính seq).

### §3.5 Đọc head

```
read_head(strata_ref) -> (head_seq, head_version_hash, mmr_root)
```

Loại #3 (Thanh-ghi): "giá trị mới nhất" = `content_cid`/`state` của version tại `head_seq`. Đếm view/like = register materialize từ append-log: log = chuỗi version (#2), head = giá trị cộng dồn hiện tại (#3). Không phải loại riêng.

### §3.6 build_state_root (field-level, cho INV-E6)

Theo khối "Mã hóa state leaf" của `_CONTRACT.md` (CHỐT-4) — Math & Tech GIỐNG HỆT:

```
build_state_root(fields):
  // 1. băm giá trị trường rồi leaf (HAI tầng — đúng CHỐT-4)
  for f in fields:
      f.fvh  = H_dom(TAG_STATE_FVAL, field_value_bytes(f))    // field_value_bytes = inline HOẶC content_cid thuần 32B
      f.leaf = H_dom(TAG_STATE_LEAF, u32_be(len(f.key)) ‖ f.key ‖ f.fvh)
  // 2. sort theo field_key tăng dần (tất định, không phụ thuộc thứ tự nhập)
  sort fields by key
  // 3. cây internal node DÙNG TAG_STATE_NODE (KHÔNG dùng leaf/node của MMR), guard dup-leaf
  state_root = state_tree_root(fields)                        // node = H_dom(TAG_STATE_NODE, left ‖ right)
  return state_root
```

KHÔNG tái dùng `mmr_leaf_hash`/`mmr_node_hash` cho state tree — tag KHÁC (TAG_STATE_LEAF/NODE) để hai cây không va chạm domain. State leaf đã có RFC6962-tương-đương qua tag riêng + cấu trúc `len(key) ‖ key ‖ fvh`; internal node dùng `TAG_STATE_NODE` với dup-leaf guard (carry, không copy lá cuối).

Loại #4 (Hồ sơ cấu trúc): cập nhật một trường = version mới với `state_root` mới; FieldProof từ `state_root` chứng minh giá trị một trường mà KHÔNG lộ trường khác (INV-E6). `value_cid` trong proof PHẢI là content_cid thuần (CHỐT-4 — chống type-leak qua field-proof).

### §3.7 Đọc version tại thời điểm t

```
version_at(strata, t) -> (version, MmrProof):
  // tìm version có ts lớn nhất với ts <= t (binary search trên seq vì ts đơn điệu)
  seq* = max { seq : version(seq).ts <= t }
  proof = mmr_inclusion_proof(seq*, mmr_size = head_seq+1)
  return (version(seq*), proof)
```

Vì `ts` đơn điệu không-giảm theo seq (kiểm ở §3.2), binary search hợp lệ. Proof MMR tới version `seq*` chứng minh giá trị tại t đã thực sự nằm trong lịch sử dưới `mmr_root` đã neo.

---

## §4. Lưu trữ qua Mirage

### §4.1 Nguyên tắc

Content mỗi version (`content_cid`, và `value_cid` từng field) lưu off-chain **qua Mirage** theo lớp bảo mật commit trong state:
- **Không nhạy cảm** → đường Bulk: LT-coded, phân tán `peer_assignment`, repair tự động.
- **Nhạy cảm** → đường mã hóa **VÀ** tái phân tán (INV-E9): bắt buộc CẢ HAI.

`ref_id`/`content_cid` vẫn là hash thuần (INV-E5); loại nằm trong state (field `data_class`), không trong định danh.

### §4.2 Hiện trạng Mirage (đối chiếu code — KHÔNG sửa, chỉ nêu yêu cầu)

Nói trung thực sau khi đọc lại mã: **HIỆN KHÔNG mode nào có CẢ mã hóa + repair.** Mấu chốt nằm ở repair loop `run_repair_loop` (`lampnet-node.rs:3520`): nó **bỏ qua mọi meta không phải Symmetric** —

```rust
// lampnet-node.rs:3567-3569
if meta.distribution_mode != DistributionMode::Symmetric {
    continue;
}
```

Hệ quả: chỉ shard ở mode **Symmetric** mới được repair. Có ba đường, cả ba đều thiếu một nửa:

1. **Vault (SSS) — mã hóa NHƯNG KHÔNG phân tán/repair.**
   - `vault/mod.rs:52 vault_encrypt`: AES-256-GCM (`aes_gcm`, `vault/mod.rs:2`) + KDF Argon2id (`argon2`, `vault/mod.rs:3`, m=64MiB t=3 p=4 tại `vault/mod.rs:17-19`) + Shamir SSS (`sharks`, threshold 20/100 tại `vault/mod.rs:10-11`). → đúng "AES-256-GCM, khóa qua Argon2id/threshold" của INV-E9.
   - NHƯNG ở daemon, đường Vault **dừng sớm**: `lampnet-node.rs:1309-1317` — `if data_class == DataClass::Vault { … encrypt_and_store_vault(...); return result; }`. Nó `return` TRƯỚC khối quota + LT-encode + phân tán peer (bắt đầu `lampnet-node.rs:1319`). Vault không sinh `peer_assignment` phân tán, `distribution_mode` không phải Symmetric → repair loop `continue` bỏ qua. **Mã hóa nhưng nằm local, mất node = mất dữ liệu nhạy cảm.**

2. **Hybrid (MẶC ĐỊNH) — phân tán parity NHƯNG KHÔNG repair, và plaintext.**
   - Đây là mode mặc định: `data_class = DataClass::Bulk` (`lampnet-node.rs:1263`), `distribution_mode = DistributionMode::Hybrid` (`lampnet-node.rs:1497`). Upload thường rơi vào đây.
   - Nhánh Hybrid (`lampnet-node.rs:1319+`): LT-encode (`MirageEncoder`, `lampnet-node.rs:1329`), phân tán N-K mảnh parity tới peers (`lampnet-node.rs:1476-1486`). NHƯNG `ShardMeta.peer_assignment` ghi **rỗng** (`lampnet-node.rs:1498 peer_assignment: Vec::new()`) và `distribution_mode: Hybrid` (`:1497`).
   - → repair loop `continue` bỏ qua (`:3567` chỉ Symmetric). Hybrid mặc định **KHÔNG được repair**. Và content **plaintext** (chỉ `apply_mask` BLAKE3-XOF `cid.rs:315` — mask tất định từ symbol_id, ai cũng tái tạo, KHÔNG phải mã hóa khóa-bí-mật).

3. **Symmetric — repair ĐẦY ĐỦ NHƯNG plaintext.**
   - Phải chọn rõ Symmetric: build `peer_assignment` thật (`lampnet-node.rs:2563-2583`, `distribution_mode: DistributionMode::Symmetric` `:2582`), phân tán shard theo `owner_of_symbol` (`:2629`).
   - Repair chạy: `run_repair_loop` (`:3520`) qua gate Symmetric (`:3567`) → `should_initiate_repair` (`repair/mod.rs:35`), `find_replacement_peer` (`repair/mod.rs:3`), `shards_owned_by` + `shrunk_assignment` (`repair/mod.rs:71`) / `updated_assignment`, re-distribute (`:3711-3725`). Coverage đầy đủ.
   - NHƯNG vẫn **plaintext** — Symmetric đi đường Bulk, không qua `vault_encrypt`.

Tóm lại đối chiếu mã: Vault = mã hóa, không repair. Hybrid (mặc định) = không repair, plaintext. Symmetric = repair, plaintext. **Không giao điểm "mã hóa ∧ repair"** — đây chính là lý do Strata cần mode thứ ba (§4.3).

### §4.3 Yêu cầu Strata đặt ra cho Mirage

Strata cần một **mode thứ ba**: `EncryptedDistributed` — mã hóa (như Vault) **rồi** đi tiếp qua LT-encode + `peer_assignment` Symmetric + repair. Cụ thể, đề xuất cho Mirage (backlog, không code lần này):

- Đường Vault KHÔNG `return` sớm ở `lampnet-node.rs:1316`. Sau `encrypt_and_store_vault`, **lấy ciphertext shards (đã AES-GCM) làm input cho LT-encode + distribution** giống Symmetric → set `distribution_mode: Symmetric` + `peer_assignment` thật (như `lampnet-node.rs:2582-2583`) → repair loop (`:3520`) qua gate Symmetric (`:3567`) áp dụng được.
- Điểm mấu chốt M4: phải dùng đường **Symmetric** (mode duy nhất repair loop nhận), KHÔNG dùng Hybrid (mặc định, peer_assignment rỗng `:1498`, bị `continue` bỏ qua). Hoặc nới gate `:3567` để nhận thêm mode `EncryptedDistributed`.
- `ShardMeta` thêm cờ `encrypted: bool` (hoặc dùng `class: DataClass`) để repair/verify biết shard đã mã hóa (vẫn so `shard_hashes[symbol_id]` trên **ciphertext** — chống poisoning vẫn chạy vì hash là của body sau mã hóa).
- Khóa: KHÔNG bao giờ rời client cho dữ liệu Vault; chỉ ciphertext + commitment hash công khai (INV-E9 "chỉ commitment hash công khai"). Reconstruct: client fetch đủ k ciphertext shard → LT-decode → AES-GCM decrypt với khóa derive Argon2id (giữ mô hình `load_vault_sss_bundle` `lampnet-node.rs:1960`, nhưng shard giờ phân tán Symmetric + repaired thay vì local).

Cho tới khi Mirage có mode này, Strata với content nhạy cảm **KHÔNG đạt INV-E9** (mã hóa có, repair không). Spec này ghi rõ làm điều kiện DoD cho dữ liệu nhạy cảm; testnet có thể tạm chấp nhận Vault-local + cảnh báo "no repair".

---

## §5. Neo on-chain (anchor)

### §5.1 Nội dung neo

Chỉ neo `StrataAnchor` = `(ref_id, head_version_hash, mmr_root, seq)` = 104 byte. KHÔNG neo content, KHÔNG neo datum đầy đủ. Bằng chứng đủ để: (a) chứng minh một version nằm trong lịch sử (`MmrProof` so với `mmr_root` đã neo), (b) chống rollback (`seq` đơn điệu).

### §5.2 Hai lựa chọn cơ chế

**Lựa chọn A — Reference UTxO kiểu CIP-68 (inline datum, spend-recreate).**
- Một UTxO mang inline datum `StrataAnchor`. Cập nhật anchor = **spend-recreate**: spend UTxO cũ, tạo UTxO mới với `seq' = seq+1` (validator kiểm `seq' == seq+1` — INV-E7).
- Ưu: trạng thái on-chain truy vấn trực tiếp (reference input cho dApp khác), finality kinh tế rõ.
- Nhược: mỗi cập nhật = 1 tx spend-recreate + min-ADA khóa trong UTxO (~1.5 ADA, đối chiếu `MIN_LOVELACE_PER_OUTPUT = 1_500_000n` `settle.ts:374`). Đắt hơn nếu cập nhật dày.

**Lựa chọn B — Tx metadata (đối chiếu `settle.ts` label 1234).**
- Ghi `StrataAnchor` vào metadata. `settle.ts:358-364` đã dùng label **1234** cho `{ merkle_root, epoch, total_distributed, node_count }` — Strata tái dùng pattern: label 1234 `{ ref_id, head_version_hash, mmr_root, seq }` (hex). Label **674** (CIP-20, `settle.ts:344-347`) cho message người-đọc-được nếu cần.
- Ưu: rẻ nhất (không khóa min-ADA, không UTxO state). Chống rollback bằng `seq` trong metadata + index off-chain (indexer từ chối anchor có seq ≤ seq đã thấy).
- Nhược: metadata KHÔNG được validator on-chain enforce nội tại (không có script kiểm `seq'==seq+1` trên metadata thuần) — phải dựa indexer/đồng thuận off-chain cho INV-E7.

### §5.3 So sánh chi phí / riêng tư với nhúng cả metadata CIP-68

| | Strata anchor (A hoặc B) | CIP-68 đầy đủ |
|---|---|---|
| On-chain bytes | 104 byte commit | datum lớn (mọi metadata/field) |
| Chi phí | A: min-ADA + spend-recreate; B: phí metadata nhỏ | min-ADA + datum size phí; tăng theo field |
| Riêng tư | chỉ hash 32-byte → KHÔNG lộ nội dung/loại (INV-E5) | LỘ hết datum on-chain (mọi field công khai) |
| Lịch sử / proof | MMR proof O(log n) off-chain | không có proof gọn nội tại; phải đọc lịch sử tx |
| Append-only | INV-E3 qua MMR | không nội tại |

→ Strata chọn **anchor 104-byte** thay nhúng CIP-68 đầy đủ: rẻ hơn, riêng tư hơn, có proof gọn. Khuyến nghị: **A cho Strata cập nhật thưa + cần on-chain state** (DID doc, hồ sơ pháp lý); **B cho Strata cập nhật dày** (đã gộp lô qua sub-MMR §7) để giảm phí.

### §5.4 Validator on-chain kiểm INV-E7 (chỉ Lựa chọn A)

```
validator spend StrataAnchor UTxO:
  datum_in  : StrataAnchor   (đang spend)
  datum_out : StrataAnchor   (UTxO mới tạo cùng tx)
  redeemer  : { new_head_version_hash, new_mmr_root, author_sig }
  REQUIRE:
    datum_out.ref_id == datum_in.ref_id              // ref_id bất biến (INV-E5: định danh ổn định)
    datum_out.seq    == datum_in.seq + 1             // INV-E7: đơn điệu, +1, KHÔNG lùi
    datum_out.head_version_hash == redeemer.new_head_version_hash
    datum_out.mmr_root          == redeemer.new_mmr_root
    verify(genesis_author_pk, sig, datum_out)        // chỉ author (hoặc policy-delegate) cập nhật được
```

Không thể neo lại version cũ: `seq` chỉ tăng, validator từ chối `seq' <= seq`. Đây là on-chain enforce của INV-E7 (chống rollback). Với Lựa chọn B, INV-E7 do indexer enforce off-chain.

---

## §6. API / Endpoint đề xuất (HTTP)

Style khớp `lampnet-node.rs` (`Router::new().route("/v1/...", post(handler))`, axum). Thêm vào bảng route hiện có (`lampnet-node.rs:1161+`). JSON in/out; hash hex-encode 64 char (khớp `merkle_root` hex trong settlement).

### POST `/v1/strata/create`
Tạo Strata genesis.
```jsonc
// req
{ "author_did": "<hex32>", "genesis_nonce": "<hex32>",
  "content_cid": "<hex>", "state_fields": [{ "key": "...", "value_cid": "<hex>" }],
  "policy_hash": "<hex32>", "ts": 1719600000, "sig": "<hex64>" }
// resp
{ "ref_id": "lnref1...", "head_seq": 0, "head_version_hash": "<hex32>", "mmr_root": "<hex32>" }
```

### POST `/v1/strata/:ref/version`
Thêm version (server kiểm INV-E1/E2/E4 trước khi nhận).
```jsonc
// req
{ "prev_seq": 4, "content_cid": "<hex>", "state_fields": [...],
  "author_did": "<hex32>", "policy_hash": "<hex32>", "ts": 1719603600, "sig": "<hex64>" }
// resp
{ "seq": 5, "version_hash": "<hex32>", "mmr_root": "<hex32>", "prev_hash": "<hex32>" }
// lỗi: 409 nếu prev_hash != head (fork); 403 nếu policy từ chối (INV-E4); 422 nếu seq nhảy
```

### GET `/v1/strata/:ref/head`
```jsonc
// resp
{ "ref_id": "lnref1...", "head_seq": 5, "head_version_hash": "<hex32>",
  "mmr_root": "<hex32>", "content_cid": "<hex>" }
```

### GET `/v1/strata/:ref/version?at=<unix_ts>`
Trả version có `ts <= at` lớn nhất + inclusion proof (§3.7).
```jsonc
// resp
{ "seq": 3, "version": { ...StrataVersion hex... },
  "proof": { "leaf_seq": 3, "leaf_hash": "<hex32>", "mmr_size": 6,
             "merkle_path": ["<hex32>", ...], "peaks": ["<hex32>", ...] } }
```

### GET `/v1/strata/:ref/proof/version/:seq`
Inclusion proof một version cụ thể dưới `mmr_root` head hiện tại.
```jsonc
// resp: MmrProof (như trên) — client verify so mmr_root đã neo on-chain (INV-E3)
```

### GET `/v1/strata/:ref/proof/field/:key`
FieldProof một trường từ `state_root` của head (INV-E6 — không lộ trường khác).
```jsonc
// resp
{ "key": "diagnosis", "value_cid": "<hex32-content_cid-thuần>", "fvh": "<hex32>",
  "leaf_idx": 2, "merkle_path": ["<hex32>", ...],
  "state_root": "<hex32>", "version_seq": 5 }
```

Tất cả handler chỉ trả **hash + proof + CID** (`value_cid` PHẢI là content_cid thuần — CHỐT-4, chống type-leak), KHÔNG trả value nhạy cảm thô (client tự fetch content qua Mirage `/internal/fetch/:cid` rồi so `fvh`). Giữ ranh giới: Strata core thuần (băm/MMR), daemon làm I/O + lưu, content qua Mirage.

---

## §7. Gộp lô tần suất cao (register / IoT)

### §7.1 Vấn đề
Register/IoT cập nhật mỗi giây → nếu mỗi cập nhật là một `StrataVersion` + một anchor on-chain thì version vô hạn + phí on-chain vô hạn. Cần gộp.

### §7.2 Sub-MMR theo epoch-checkpoint

- Trong một **epoch** (cấu hình, mặc định khớp `EPOCH_DURATION_SECS = 3600` của Reward), mọi entry (mỗi giá trị đo) là một **leaf của sub-MMR** trong RAM/local, KHÔNG sinh `StrataVersion` riêng.
- Cuối epoch: tạo **một** `StrataVersion` checkpoint với:
  - `content_cid` = CID của batch entries (toàn bộ entry epoch đó, lưu qua Mirage),
  - `state_root` = root của sub-MMR epoch (commitment mọi entry trong epoch),
  - `ts` = mốc cuối epoch.
- Anchor on-chain chỉ neo **head sau mỗi checkpoint** (1 anchor/epoch), không phải mỗi entry.

```
sub_mmr_epoch:
  for each reading r in epoch: sub.append(mmr_leaf_hash(H_dom(TAG_ENTRY, entry_bytes(r))))
  at epoch end: checkpoint_state_root = mmr_root(sub.leaves)   // §3.4 commit n (CHỐT-3)
                append_version(content_cid = batch_cid, state_root = checkpoint_state_root, ...)
```

Proof một entry: sub-MMR inclusion (entry ∈ epoch checkpoint) + MMR inclusion (checkpoint version ∈ Strata history). Hai tầng proof, vẫn O(log n).

### §7.3 Batching policy + cấu hình

```rust
pub struct BatchPolicy {
    pub epoch_secs:      u64,    // checkpoint mỗi epoch (mặc định 3600)
    pub max_entries:     u32,    // ép checkpoint sớm nếu vượt (chống sub-MMR phình RAM)
    pub flush_max_age:   u64,    // checkpoint khi TUỔI entry cũ nhất ≥ N giây (đổi từ flush_on_idle — xem Strata-API §5.3)
}
```

Lựa chọn anchor: với register tần suất cao, dùng **Lựa chọn B (metadata)** §5.2 cho checkpoint để giảm phí; spend-recreate (A) chỉ khi cần on-chain state.

### §7.4 CRDT cho register hội tụ
Với register nhiều ghi đồng thời (multi-writer), `_CONTRACT.md` nêu CRDT là lựa chọn cho hội tụ: entry trong epoch là CRDT op (VD G-Counter cho đếm view/like, LWW-Register cho "giá trị hiện tại"); checkpoint state_root commit trạng thái CRDT đã merge cuối epoch. Đảm bảo các node ra cùng state_root (tất định) dù thứ tự nhận op khác nhau. Chi tiết merge → Strata-Math.

---

## §7.5 Index dẫn xuất (nguyên tắc cứng: log là nguồn sự thật duy nhất)

Truy vấn nhanh (tìm theo người gửi, lọc cột, full-text) cần **index**. Quyết định kiến trúc đã chốt qua phản biện: index KHÔNG bao giờ là nguồn sự thật. Nguyên tắc cứng:

- **MMR/log = nguồn sự thật DUY NHẤT.** Mọi trạng thái xác thực được nằm trong chuỗi version + MMR. Chỉ `mmr_root`/`state_root` được neo, được ký, được tin.
- **Index / query / columnar = materialized view KHẢ BIẾN, UNTRUSTED.** Index là cache để tăng tốc; nó có thể sai, cũ, hoặc bị bỏ — không sao, vì nó không được tin.
- **Tái dựng tất định từ log.** Mọi index dựng được lại từ log bằng một hàm tất định `replay: log → index`. Cùng log → cùng index, byte-chính-xác, trên mọi máy.
- **Một chiều: log → index.** Dữ liệu chảy MỘT chiều từ log sang index. **CẤM đường ghi vòng qua MMR**: index KHÔNG bao giờ được phép sửa/thêm leaf MMR. Một ghi mới đi vào log trước, index cập nhật sau (hoặc dựng lại).
- **Mọi kết quả verify được bằng inclusion-proof.** Một kết quả truy vấn từ index chỉ là "gợi ý vị trí"; client xác thực bằng inclusion-proof về `mmr_root` đã neo. Index sai → proof không khớp → bị phát hiện ngay.

```rust
/// Index là view dẫn xuất. KHÔNG có API ghi-ngược vào MMR.
pub trait DerivedIndex {
    /// Dựng lại tất định từ log. replay(log) phải cho cùng index trên mọi máy.
    fn replay(log: &VersionLog) -> Self;
    /// Truy vấn trả VỊ TRÍ (leaf_seq) — client tự verify bằng MmrProof.
    fn lookup(&self, q: &Query) -> Vec<Seq>;
    // KHÔNG có fn write_back(&mut self, ..) đụng tới MMR — một chiều log→index.
}
```

**Test bắt buộc (đưa vào §9):** xóa toàn bộ index, chạy `replay(log)` dựng lại, rồi so `mmr_root` (và mọi `state_root`) — phải **khớp bit**. Nếu lệch → có đường ghi vòng (index đã ảnh hưởng log) → lỗi nghiêm trọng. Test này là chốt chặn để nguyên tắc một-chiều không bị phá lén qua thời gian.

Áp dụng: index nóng `(sender, ts)` cho nhóm chat (Strata-Math §13), columnar engine cho lọc/join tabular (Strata-Feat §8) — tất cả là `DerivedIndex`, untrusted, dựng lại từ log.

---

## §7.6 Đệm nhiễu (decoy/padding) + chi phí (mở rộng INV-E9)

INV-E9 yêu cầu mã hóa + tái phân tán cho dữ liệu nhạy cảm. Mã hóa giấu **nội dung** nhưng KHÔNG giấu **kích thước** bản mã và **mẫu lưu lượng** — hai kênh phụ vẫn để lộ loại dữ liệu (suy luận: object lớn = sổ bệnh, object nhỏ = tin nhắn). Mở rộng INV-E9 cho kênh phụ này (đối chiếu Strata-Feat §9):

- **Bucket kích thước cố định.** Bản mã đệm lên một trong vài bucket cố định (ví dụ `[4 KiB, 64 KiB, 1 MiB]`). Mọi object trong cùng bucket đồng kích thước → không suy ra loại từ độ dài. Padding dùng byte ngẫu nhiên trong vùng đã mã hóa (không phân biệt được với nội dung thật).
- **Shard nhiễu kích thước khác nhau.** Chèn shard giả (decoy) kích thước khác nhau vào dòng phân tán Mirage. Phân tích lưu lượng (traffic-analysis) không tách được shard thật khỏi nhiễu — chống suy luận qua mẫu phân tán.

```rust
pub struct PrivacyPadding {
    pub size_buckets:   Vec<u64>,   // VD [4096, 65536, 1048576] — bản mã đệm lên bucket gần nhất
    pub decoy_shards:   u32,        // số shard nhiễu chèn vào dòng phân tán
    pub decoy_size_jitter: bool,    // shard nhiễu kích thước KHÁC nhau (chống size-fingerprint)
}
```

**Chi phí do user trả.** Đệm + shard nhiễu làm tăng dung lượng lưu và băng thông xử lý. Quyết định: chi phí tăng này **gắn vào pricing, user trả** — ai chọn mức riêng tư cao (bucket lớn, nhiều decoy) thì trả nhiều hơn. Không bắt mạng gánh chi phí riêng tư của một cá nhân. Pricing module tính phí theo `size_bucket_thực_tế + decoy_shards × kích_thước_shard`, không theo kích thước nội dung thật (vì kích thước thật được giấu).

> Đây là đánh đổi minh bạch: riêng tư cao hơn ⇒ chi phí cao hơn, hiện rõ trong giá. Object không nhạy cảm bỏ qua padding (không trả phí thừa).

---

## §8. Migration (dữ liệu tĩnh hiện tại → Strata 1-version)

### §8.1 Bọc CID Bulk/Vault cũ vào Strata 1-version

Dữ liệu tĩnh hiện tại trong Mirage (CID `lamp://ln1…` Bulk hoặc Vault) bọc thành **Strata loại #1** (1 version, seq=0):

```
migrate_static(old_cid, owner_did) -> StrataRef:
  // 1. content_cid mới = hash thuần của content (bỏ class byte leak — INV-E5)
  content_cid = strip_class(old_cid)     // parse_root_hash trả 32B; KHÔNG byte class
  // 2. data_class cũ (Vault/Bulk) chuyển vào STATE, không vào định danh
  state_fields = [ StateField { key: b"data_class", value_cid: cid_of(old_class_label) } ]
  // 3. genesis nonce ngẫu nhiên; ref_id THUẦN
  strata = create_strata(owner_did, random_nonce, content_cid, state_fields,
                   policy_hash = default_owner_policy, ts = now, sign_fn)
  // 4. map old_cid -> ref_id (bảng tra cứu để link cũ vẫn resolve)
  strata
```

Điểm mấu chốt: loại (Vault/Bulk) **chuyển từ định danh sang state** — sửa leak INV-E5 ngay khi migrate. `old_cid` cũ vẫn decode được (giữ `gen_cid_v2`/`LampUri` deprecated cho backward-compat), nhưng định danh Strata mới (`lnref1…`) không lộ loại.

### §8.2 Tương thích ngược
- Endpoint cũ (`/mirage/put`, `/v1/inspect/:cid`, `lampnet-node.rs:1163,1170`) vẫn chạy nguyên — Strata là tầng trên, không thay thế.
- `parse_root_hash` (`cid.rs:190`) giữ cả hai nhánh: legacy `ln1q_` (8B), `lamp://ln1` (32B sau strip class), thêm nhánh `lnref1` (32B no-class).
- Migration là **opt-in, không phá**: dữ liệu chưa migrate vẫn dùng CID cũ; chỉ dữ liệu cần version/anchor mới bọc Strata.

---

## §9. Kiểm thử

### §9.1 Test theo invariant (INV-E1..E9)

| # | Test | Kiểm tra |
|---|---|---|
| 1 | `version_hash_linked` | version seq=k có `prev_hash == version_hash(k-1)`; seq=0 → `prev_hash == 0^32` (INV-E1) |
| 2 | `seq_monotonic_plus_one` | append cho seq = head+1; nhảy/lùi seq → Err (INV-E2) |
| 3 | `mmr_append_only_old_proof_holds` | sau extend_mmr, mọi inclusion-proof cũ vẫn verify dưới root mới (INV-E3) |
| 4 | `policy_denies_unauthorized_field` | author không được policy cho phép sửa field → Err(PolicyDenied) (INV-E4) |
| 5 | `sig_required_and_verified` | sig sai khóa author → reject version (INV-E4) |
| 6 | `ref_id_no_type_byte` | `gen_ref_id` output KHÔNG có byte class; 32B thuần; đổi class không đổi ref_id (INV-E5) |
| 7 | `content_cid_pure_hash` | `gen_content_cid(data) == blake3(data)`, không prefix (INV-E5) |
| 8 | `field_proof_no_leak` | FieldProof(key A) KHÔNG cho suy ra key/value của field B (INV-E6) |
| 9 | `anchor_seq_monotonic` | validator/indexer từ chối anchor seq ≤ seq đã neo (INV-E7) |
| 10 | `hash_domain_separated` | leaf_hash ≠ node_hash cho cùng input (RFC6962 prefix); đổi tag → đổi hash (INV-E8) |
| 11 | `mmr_no_dup_leaf` | số leaf lẻ KHÔNG copy leaf cuối; root khác cây nhân-đôi-lá (CVE-2012-2459) (INV-E8) |
| 12 | `sensitive_requires_encrypt_and_repair` | content nhạy cảm: thiếu mã hóa HOẶC thiếu peer_assignment/repair → fail DoD (INV-E9) |
| 13 | `index_replay_root_bit_exact` | xóa toàn bộ index, `replay(log)` dựng lại, so `mmr_root` + mọi `state_root` → **khớp bit**; lệch ⇒ có đường ghi vòng qua MMR (vi phạm nguyên tắc một-chiều §7.5) |
| 14 | `index_is_untrusted` | index sai/cũ → kết quả lookup vẫn verify được bằng `MmrProof`; proof không khớp `mmr_root` đã neo ⇒ kết quả bị từ chối (§7.5) |
| 15 | `size_bucket_hides_type` | hai object loại khác nhau, cùng bucket → kích thước bản mã bằng nhau (không suy ra loại qua độ dài — INV-E9 mở rộng §7.6) |
| 16 | `composite_two_tier_proof` | proof "phần tử x thuộc đối tượng ghép C" = field-proof cha (`role → ref_id`) + proof con; gốc con khớp giá trị cha trả về (Strata-Feat §6, Strata-Math §12) |
| 17 | `merkle_sum_tree_total` | tổng cột = `total_sum` ở gốc MST; proof một hàng cộng dồn `(sum,count)` anh em ra đúng tổng; sửa giá trị hàng không giữ được root (Strata-Math §14) |

### §9.2 Redteam (tấn công lịch sử)

| # | Tấn công | Kỳ vọng |
|---|---|---|
| R1 | **Sửa lịch sử** — đổi `content_cid` version cũ | version_hash đổi → prev_hash version sau lệch → chuỗi gãy (INV-E1); mmr_root đổi → inclusion-proof cũ fail |
| R2 | **Rollback** — neo lại anchor với seq nhỏ hơn | validator/indexer reject (INV-E7) |
| R3 | **Fork head** — append từ version không phải head | `prev_hash != head_version_hash` → reject (chống nhánh song song) |
| R4 | **Dup-leaf (CVE-2012-2459)** — dựng cây giả nhân đôi lá cuối ra cùng root | MMR carry → root khác → forge fail (INV-E8) |
| R5 | **Field-leak** — từ FieldProof một trường suy ra trường khác | chỉ sibling hash lộ; key/value field khác không suy được (INV-E6) |
| R6 | **Type-leak** — từ ref_id/content_cid đoán Vault/Bulk | hash thuần → không phân biệt được (INV-E5) |
| R7 | **Second-preimage leaf-vs-node** — dùng node hash làm leaf | RFC6962 prefix 0x00/0x01 khác nhau → fail (INV-E8) |
| R8 | **Sensitive plaintext leak** — đẩy content nhạy cảm qua đường Bulk (plaintext) | policy/DoD bắt buộc đường EncryptedDistributed; Bulk-plaintext cho field nhạy cảm bị từ chối (INV-E9) |

### §9.3 Property tests

| # | Property |
|---|---|
| P1 | `mmr_root_deterministic`: cùng dãy version (cùng thứ tự seq) → cùng mmr_root trên mọi máy |
| P2 | `mmr_inclusion_complete`: ∀ seq ≤ head, `mmr_inclusion_proof(seq)` verify được dưới mmr_root |
| P3 | `mmr_extend_monotone`: ∀ proof hợp lệ dưới root_n vẫn hợp lệ dưới root_{n+1} (INV-E3) |
| P4 | `canonical_roundtrip`: `canonical_version_bytes` → parse → cùng StrataVersion (trừ sig); encode tất định byte-chính-xác |
| P5 | `state_root_order_independent`: hoán vị thứ tự nhập field → cùng state_root (sort theo key §3.6) |
| P6 | `ts_monotone_enables_version_at`: ts đơn điệu ⇒ `version_at(t)` đúng version (binary search) |
| P7 | `ref_id_collision_resistance`: author_did/nonce khác → ref_id khác (BLAKE3 32B) |

---

## §10. Ràng buộc triển khai

- **Định danh thuần (INV-E5).** Mọi `ref_id`/`content_cid` là hash thuần; CẤM nhúng class byte/nhãn loại. `gen_cid_v2`/`LampUri` chỉ cho decode legacy, deprecated cho định danh mới.
- **Băm an toàn (INV-E8).** Mọi cây dùng domain-sep + RFC6962 prefix + dup-leaf guard (MMR carry, KHÔNG copy leaf cuối). KHÔNG tái dùng `merkle.rs` của Reward cho Strata (nó nhân đôi lá cuối — không có guard).
- **Append-only (INV-E3).** Chỉ MỞ RỘNG MMR; không sửa/xóa leaf. Validator off-chain reject version không nối từ head.
- **Canonical tất định.** Encoder tay (`canonical_version_bytes`), u64 big-endian, length-prefix cho trường thay đổi; KHÔNG dựa serde mặc định cho bytes-để-băm.
- **Core thuần (no I/O).** `lampnet-strata` core (băm, MMR, proof) thuần — không file/network. Daemon (`lampnet-node.rs`) làm I/O; content qua Mirage; anchor qua settlement.
- **Nhạy cảm = mã hóa VÀ repair (INV-E9).** Content nhạy cảm phải đi đường EncryptedDistributed (mã hóa như Vault + phân tán/repair như Symmetric — KHÔNG phải Bulk/Hybrid mặc định, vì chỉ Symmetric mới được repair, xem §4.2). Cho tới khi Mirage có mode này, đánh dấu rõ "INV-E9 chưa đạt cho dữ liệu nhạy cảm".
- **Anchor tối thiểu.** On-chain chỉ 104-byte `StrataAnchor`; KHÔNG nhúng datum/content. seq đơn điệu enforce (validator A hoặc indexer B).
- **Không panic production.** Mọi lỗi qua `Result`; overflow seq (`u64::MAX`) → Err, không wrap.

---

## §11. So sánh bắt buộc (đối chiếu hệ thống khác)

### CIP-68
Anchor/cập nhật on-chain OK, nhưng: (a) lịch sử không có proof gọn (phải đọc cả chuỗi tx), (b) datum nhỏ/đắt/**lộ hết** (mọi field công khai on-chain), (c) không append-only nội tại. Strata: content off-chain, on-chain 104-byte commit, MMR cho proof O(log n), field-privacy (INV-E6). Xem so sánh chi phí/riêng tư §5.3.

### git
Hash-linked DAG bất biến (giống version chain của Strata), nhưng: inclusion proof = **cả path commit** (không compact), không neo on-chain/không finality kinh tế, branch (mutable ref) không tamper-evident (sửa được không để lại dấu). Strata: MMR compact proof + head ký + anchor on-chain đơn điệu (INV-E7) → mutable-ref tamper bị chặn.

---

## §12. Lỗi hệ thống cũ Strata khắc phục (đối chiếu nhất quán `_CONTRACT.md`)

| Lỗi cũ | Cơ chế Strata | Chứng cứ code |
|---|---|---|
| Merkle dup-leaf (CVE-2012-2459) | MMR carry, không copy leaf cuối (§3.4) | trái với `merkle.rs build_root` nhân-đôi-lá (`Reward-Tech §4.9`) |
| Second-preimage leaf-vs-node | RFC6962 prefix 0x00/0x01 (§1.1, §3.4) | — |
| Rollback/tua lùi version | seq + anchor đơn điệu (INV-E7, §5.4) | — |
| Mutable-ref tamper (git branch) | head ký + neo on-chain (§5) | đối chiếu label 1234 `settle.ts:358` |
| CIP-68 size/cost/leak | off-chain content + 104-byte commit (§5.3) | `MIN_LOVELACE_PER_OUTPUT` `settle.ts:374` |
| CID leak loại | ref_id/content_cid hash thuần (INV-E5, §2) | `gen_cid_v2` `cid.rs:78`, `LampUri.to_payload` `spec.rs:125` |
| Nhạy cảm không repair / Bulk không mã hóa | mode EncryptedDistributed (INV-E9, §4.3) | Vault `lampnet-node.rs:1309-1317`; repair `repair/mod.rs`; Bulk plaintext |

---

## §13. Liên kết spec khác
- **Strata-Math**: công thức `version_hash` (ràng buộc sig), MMR bag-peaks, state Merkle, chứng minh INV-E1..E9, đặc tả CRDT merge cho register, Composite Strata (§12), truy vấn lịch sử & số liệu nhóm 100k/3 năm (§13), Merkle Sum Tree (§14).
- **Strata-Feat**: tính năng + hành trình (tạo hồ sơ → cập nhật field → chứng minh → neo → đọc tại t); Composite Strata (§6); hai trục thiết kế + bốn tầng lưu trữ (§7); tabular per-row (§8); decoy/padding (§9); audit-log (§10).
- **VeData / GreenSun (Stamp, A22 MEASUREMENT_SERIES)**: dùng chung sub-primitive `lampnet-merkle-anchor` CÓ ĐIỀU KIỆN (`<Sha3>`), KHÔNG gộp module (§0.5).
- **Reward**: dùng chung BLAKE3 + RFC6962 ý tưởng, nhưng Strata có dup-leaf guard MMR (khác `merkle.rs`).
- **Mirage**: `vault/mod.rs` (mã hóa), `distrecon`/`repair` (phân tán) — nguồn yêu cầu mode EncryptedDistributed (§4.3).
- **Settlement**: `settle.ts` label 674/1234 — pattern neo anchor on-chain (§5.2).
- **Codec**: `cid.rs` — sửa leak định danh (§2), thêm `gen_ref_id`/`gen_content_cid`.

---

Nguồn chuẩn invariant: `_CONTRACT.md` (INV-E1..E9). Mọi mâu thuẫn là lỗi.
