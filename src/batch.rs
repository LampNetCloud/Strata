//! `Checkpoint` (sub-MMR) + `BatchPolicy` + `EpochAccumulator` — S3: gộp N entry theo
//! epoch thành MỘT checkpoint (sub-MMR root) rồi neo **một lần** thay vì N lần. Chứng
//! minh một entry lẻ bằng **inclusion hai tầng** (entry ∈ checkpoint ∈ lịch sử đã neo).
//!
//! Dựng TRÊN [`lampnet_merkle_anchor::mmr::Mmr`] — KHÔNG primitive mới (Strata-API §8.3).
//!
//! Byte-layout đã-neo (khớp `_CONTRACT.md` CHỐT-2 + §1.7 canonical):
//! - `entry_bytes = u64_be(entry_seq) ‖ u32_be(len(payload)) ‖ payload` (length-prefix
//!   chống nhập nhằng nối chuỗi — như `version.rs §1.7`).
//! - **entry leaf = `H_dom("LN/STRATA/entry/v1", entry_bytes)`** — tách MIỀN entry khỏi
//!   miền version-hash (`ver/v1`): một entry KHÔNG được nhầm là một version_hash
//!   (chống type-confusion/second-preimage giữa các cây — anh Đức chốt, issue #1/#3).
//!   Rồi `sub.append(leaf)` — `Mmr::append` tự phủ `leaf_hash(TAG_LEAF, ·)` nội tại.
//! - `checkpoint_state_root = sub.root()` → gán vào `StrataVersion.state_root` của
//!   version-checkpoint khi `chain.append_version` (§8.3b). Checkpoint là một version
//!   BÌNH THƯỜNG; không có API mới ở core, vòng gộp epoch là lớp daemon.
//!
//! Ngữ nghĩa đóng epoch (§5.3, addendum 04/07 — PR #8): đóng khi BẤT KỲ (1) hết
//! `epoch_secs`; (2) đủ `max_entries`; (3) **tuổi entry CŨ NHẤT ≥ `flush_max_age`**
//! (van (3) KHÔNG phải "im lặng" — tin rả rích vẫn gom một checkpoint nhưng không entry
//! nào chờ quá hạn). `now` do caller truyền (core THUẦN, không `SystemTime`).

use crate::chain::StrataChain;
use crate::u32_be;
use crate::version::{Hash32, StrataVersion};
use lampnet_merkle_anchor::Blake3Hasher;
use lampnet_merkle_anchor::hash::h_dom;
use lampnet_merkle_anchor::mmr::{InclusionProof, Mmr, verify as mmr_verify};

/// Domain-tag entry sub-MMR — bảng CHỐT-2 `_CONTRACT.md` ("Batch entry (sub-MMR gộp lô)").
/// Tách miền khỏi `LN/STRATA/ver/v1` (version-hash) và `LN/STRATA/mmr/leaf/v1` (leaf nền).
pub const TAG_ENTRY: &str = "LN/STRATA/entry/v1";

/// Lỗi lớp gộp lô (fail-closed — không bao giờ ghi một phần).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchError {
    /// `entry_seq` ≤ watermark toàn-chain → replay / out-of-order (kể cả xuyên epoch).
    ReplaySeq { last: u64, got: u64 },
    /// Epoch đã đủ `max_entries` — entry này thuộc epoch SAU. Caller `close()` rồi push lại.
    /// (`max_entries = 0` cũng trả biến thể này ở mọi push → fail-closed, không commit gì.)
    EpochFull { max_entries: u32 },
    /// `payload` dài hơn `u32::MAX` — length-prefix `u32` sẽ truncate lặng lẽ (chặn trước).
    PayloadTooLarge { len: usize },
    /// Blob lô hỏng khi parse (count sai / byte cụt / thừa byte đuôi).
    MalformedBatch,
}

/// `entry_bytes` canonical (§1.7): `u64_be(entry_seq) ‖ u32_be(len(payload)) ‖ payload`.
/// `payload` = giá trị đo / CRDT-op serialize tất định (caller lo tất định).
/// Guard `payload > u32::MAX` (Strata-API §8.3 / review #4) trước khi encode — không để
/// `u32_be` truncate lặng lẽ làm gãy canonical encoding.
pub fn entry_bytes(entry_seq: u64, payload: &[u8]) -> Result<Vec<u8>, BatchError> {
    if payload.len() > u32::MAX as usize {
        return Err(BatchError::PayloadTooLarge { len: payload.len() });
    }
    let mut b = Vec::with_capacity(8 + 4 + payload.len());
    b.extend_from_slice(&entry_seq.to_be_bytes()); // u64 BE
    b.extend_from_slice(&u32_be(payload.len())); // len-prefix u32 BE (đã guard ≤ u32::MAX)
    b.extend_from_slice(payload);
    Ok(b)
}

/// Leaf-data đưa vào sub-MMR = `H_dom(TAG_ENTRY, entry_bytes)` (32B) — tách miền entry.
pub fn entry_leaf(entry_seq: u64, payload: &[u8]) -> Result<Hash32, BatchError> {
    Ok(h_dom(TAG_ENTRY, &entry_bytes(entry_seq, payload)?))
}

/// Số byte hash của một inclusion-proof (siblings + peaks) — để báo cáo kích thước
/// proof tầng dưới (test §8.3 #2: ~log2(N)×32B). KHÔNG gồm cờ hướng (1B/sibling).
pub fn proof_hash_bytes(p: &InclusionProof) -> usize {
    (p.siblings.len() + p.peaks.len()) * 32
}

/// Sub-MMR checkpoint gộp một epoch entry. Giữ `leaves` (leaf-data 32B mỗi entry) để
/// sinh/thẩm proof tầng dưới sau khi đã neo checkpoint.
///
/// Debug/Clone hand-impl: `Mmr<H>` derive cần `H: Debug+Clone` (PhantomData), mà
/// `Blake3Hasher` là unit struct không derive — nên MMR tái dựng từ `leaves`.
pub struct Checkpoint {
    sub: Mmr<Blake3Hasher>,
    leaves: Vec<Hash32>,
}

impl std::fmt::Debug for Checkpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Checkpoint")
            .field("entries", &self.leaves.len())
            .field("state_root", &self.sub.root())
            .finish()
    }
}

impl Clone for Checkpoint {
    fn clone(&self) -> Self {
        let mut sub = Mmr::<Blake3Hasher>::new();
        for leaf in &self.leaves {
            sub.append(leaf);
        }
        Self {
            sub,
            leaves: self.leaves.clone(),
        }
    }
}

impl Default for Checkpoint {
    fn default() -> Self {
        Self::new()
    }
}

impl Checkpoint {
    /// Checkpoint rỗng.
    pub fn new() -> Self {
        Self {
            sub: Mmr::new(),
            leaves: Vec::new(),
        }
    }

    /// Append một entry (đã phủ domain-tag `entry/v1`). Trả index (0-based).
    /// Lỗi `PayloadTooLarge` → KHÔNG ghi gì (fail-closed).
    pub fn append_entry(&mut self, entry_seq: u64, payload: &[u8]) -> Result<usize, BatchError> {
        let leaf = entry_leaf(entry_seq, payload)?;
        let idx = self.leaves.len();
        self.sub.append(&leaf);
        self.leaves.push(leaf);
        Ok(idx)
    }

    /// Số entry trong checkpoint.
    pub fn len(&self) -> usize {
        self.leaves.len()
    }

    /// Rỗng?
    pub fn is_empty(&self) -> bool {
        self.leaves.is_empty()
    }

    /// `checkpoint_state_root` = root sub-MMR — giá trị gán vào `version.state_root`.
    pub fn state_root(&self) -> Hash32 {
        self.sub.root()
    }

    /// Số lá sub-MMR (dùng làm `n` khi verify tầng dưới).
    pub fn size(&self) -> u64 {
        self.sub.len()
    }

    /// leaf-data của entry `idx` (để verifier tái tạo/đối chiếu).
    pub fn leaf_at(&self, idx: usize) -> Option<Hash32> {
        self.leaves.get(idx).copied()
    }

    /// Proof tầng dưới: entry `idx` ∈ checkpoint. Trả `(sub_proof, sub_size, entry_leaf)`.
    pub fn prove_entry(&self, idx: usize) -> Option<(InclusionProof, u64, Hash32)> {
        if idx >= self.leaves.len() {
            return None;
        }
        Some((self.sub.prove(idx), self.sub.len(), self.leaves[idx]))
    }

    /// Verify tầng dưới: `entry_leaf` ∈ checkpoint dưới `state_root` cho trước.
    pub fn verify_entry(
        state_root: Hash32,
        entry_leaf: &Hash32,
        idx: usize,
        size: u64,
        proof: &InclusionProof,
    ) -> bool {
        mmr_verify::<Blake3Hasher>(state_root, entry_leaf, idx, size, proof)
    }
}

/// Tham số gộp lô (§5.3 — Strata-API §8.3, addendum 04/07 chốt tại PR #8). Đây là **config
/// runtime** (không đóng byte-layout — anh Đức chốt issue #3b).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchPolicy {
    /// Độ dài epoch (giây) — khớp `EPOCH_DURATION_SECS` Reward.
    pub epoch_secs: u64,
    /// Trần entry/epoch (chặn sub-MMR phình RAM) — đóng sớm khi vượt. `u32` khớp §5.3.
    pub max_entries: u32,
    /// Đóng epoch khi **tuổi entry CŨ NHẤT** đạt ngưỡng (giây) — KHÔNG phải "im lặng".
    pub flush_max_age: u64,
}

impl Default for BatchPolicy {
    fn default() -> Self {
        Self {
            epoch_secs: 3600,
            max_entries: 10_000,
            flush_max_age: 300,
        }
    }
}

impl BatchPolicy {
    /// Profile ProofChat (panel phản biện 04/07 — §8.3): epoch 10 phút, 4096 entry,
    /// flush_max_age 180s.
    pub fn proofchat() -> Self {
        Self {
            epoch_secs: 600,
            max_entries: 4096,
            flush_max_age: 180,
        }
    }

    /// Quyết định **thuần** (không timer, không I/O): có đóng epoch chưa? Ưu tiên:
    /// `MaxEntries` (chặn RAM) → `EpochElapsed` → `FlushMaxAge`. `None` = tiếp tục gộp.
    ///
    /// - `count` = số entry hiện có; `epoch_start_ts` = mốc mở epoch (ts push ĐẦU tiên);
    /// - `oldest_ts` = ts NHỎ NHẤT trong epoch (min, không đòi ts đơn điệu);
    /// - `now` = đồng hồ hiện tại (caller cấp). `saturating_sub` chống clock-skew panic.
    ///
    /// Góc chết đã xử: **epoch RỖNG không bao giờ đóng** (`count == 0 → None`), diệt luôn
    /// bệnh `max_entries = 0` đóng lặp vô hạn trên epoch rỗng.
    pub fn should_close(
        &self,
        count: usize,
        epoch_start_ts: u64,
        oldest_ts: u64,
        now: u64,
    ) -> Option<CloseReason> {
        if count == 0 {
            return None; // epoch rỗng: không có gì để chốt.
        }
        if count as u64 >= self.max_entries as u64 {
            return Some(CloseReason::MaxEntries);
        }
        if now.saturating_sub(epoch_start_ts) >= self.epoch_secs {
            return Some(CloseReason::EpochElapsed);
        }
        if now.saturating_sub(oldest_ts) >= self.flush_max_age {
            return Some(CloseReason::FlushMaxAge);
        }
        None
    }
}

/// Lý do đóng epoch (đóng checkpoint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// Đạt trần `max_entries`.
    MaxEntries,
    /// Hết `epoch_secs`.
    EpochElapsed,
    /// Tuổi entry cũ nhất ≥ `flush_max_age` (thay ngữ nghĩa `Idle` cũ).
    FlushMaxAge,
}

/// Kết quả đóng epoch — checkpoint gộp lô đã sẵn để `append_version` + đẩy Mirage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedEpoch {
    /// `checkpoint_state_root` = root sub-MMR → gán `version.state_root`.
    pub sub_mmr_root: Hash32,
    /// Số lá sub-MMR (n khi verify tầng dưới).
    pub sub_size: u64,
    /// Số entry gộp.
    pub entries: usize,
    /// Blob lô canonical (nguồn `content_cid` qua Mirage; parse lại dựng đúng root).
    pub entries_serialized: Vec<u8>,
}

/// Bộ gộp epoch có state — driver §5.3 (`new/push/should_close/close`). Core THUẦN:
/// `now`/`ts` do caller cấp, blob + `content_cid` do caller đẩy Mirage; vòng lặp định kỳ
/// là lớp daemon.
///
/// Watermark `last_entry_seq` (chống replay) **SỐNG XUYÊN `close()`** — daemon retry sau
/// crash không băm đôi một entry vào hai checkpoint.
#[derive(Debug)]
pub struct EpochAccumulator {
    policy: BatchPolicy,
    cp: Checkpoint,
    /// (entry_seq, payload) theo thứ tự append — nguồn blob canonical khi đóng.
    entries: Vec<(u64, Vec<u8>)>,
    /// ts push đầu tiên của epoch hiện tại (mốc `epoch_secs`).
    epoch_start_ts: Option<u64>,
    /// ts nhỏ nhất trong epoch (mốc `flush_max_age`, min — không đòi đơn điệu).
    oldest_ts: Option<u64>,
    /// Watermark entry_seq toàn-chain — sống xuyên close, tăng nghiêm ngặt.
    last_entry_seq: Option<u64>,
}

impl EpochAccumulator {
    /// Bộ gộp mới với `policy`. (`max_entries = 0` là hợp lệ về kiểu nhưng mọi `push` sẽ
    /// `EpochFull` → fail-closed, không commit gì — thay vì đóng lặp vô hạn.)
    pub fn new(policy: BatchPolicy) -> Self {
        Self {
            policy,
            cp: Checkpoint::new(),
            entries: Vec::new(),
            epoch_start_ts: None,
            oldest_ts: None,
            last_entry_seq: None,
        }
    }

    /// Chính sách đang dùng.
    pub fn policy(&self) -> &BatchPolicy {
        &self.policy
    }

    /// Số entry epoch hiện tại.
    pub fn len(&self) -> usize {
        self.cp.len()
    }

    /// Epoch hiện tại rỗng?
    pub fn is_empty(&self) -> bool {
        self.cp.is_empty()
    }

    /// Watermark entry_seq đã thấy (None nếu chưa entry nào) — quan sát cho daemon.
    pub fn last_entry_seq(&self) -> Option<u64> {
        self.last_entry_seq
    }

    /// Nạp một entry vào epoch hiện tại. Trả index (0-based) trong sub-MMR khi thành công.
    ///
    /// Thứ tự kiểm (fail-closed — lỗi nào cũng KHÔNG ghi gì, watermark KHÔNG đổi):
    /// 1. `ReplaySeq` nếu `entry_seq ≤ last_entry_seq` (chống retry băm đôi entry).
    /// 2. `EpochFull` nếu epoch đã đủ `max_entries` (entry thuộc epoch SAU — caller
    ///    `close()` rồi push lại; watermark chưa cập nhật nên push lại KHÔNG bị coi replay).
    /// 3. `PayloadTooLarge` nếu `payload > u32::MAX`.
    ///
    /// `ts` = thời điểm gốc của entry (dùng cho `oldest_ts`); `now` = đồng hồ mở-epoch.
    pub fn push(
        &mut self,
        entry_seq: u64,
        ts: u64,
        payload: &[u8],
        now: u64,
    ) -> Result<usize, BatchError> {
        // (1) chống replay/out-of-order xuyên epoch.
        if let Some(last) = self.last_entry_seq
            && entry_seq <= last
        {
            return Err(BatchError::ReplaySeq {
                last,
                got: entry_seq,
            });
        }
        // (2) trần RAM enforce NGAY tại điểm ghi (không đợi should_close).
        if (self.cp.len() as u64) >= self.policy.max_entries as u64 {
            return Err(BatchError::EpochFull {
                max_entries: self.policy.max_entries,
            });
        }
        // (3) encode + append (guard PayloadTooLarge nằm trong append_entry).
        let idx = self.cp.append_entry(entry_seq, payload)?;
        self.entries.push((entry_seq, payload.to_vec()));
        // Cập nhật mốc epoch — chỉ sau khi ghi chắc chắn thành công.
        self.epoch_start_ts.get_or_insert(now);
        self.oldest_ts = Some(match self.oldest_ts {
            Some(o) => o.min(ts),
            None => ts,
        });
        self.last_entry_seq = Some(entry_seq);
        Ok(idx)
    }

    /// Có nên đóng epoch tại `now`? (thuần — ủy quyền `BatchPolicy::should_close`).
    pub fn should_close(&self, now: u64) -> Option<CloseReason> {
        self.policy.should_close(
            self.cp.len(),
            self.epoch_start_ts.unwrap_or(now),
            self.oldest_ts.unwrap_or(now),
            now,
        )
    }

    /// Đóng epoch: trả `ClosedEpoch` (root/size/blob canonical) rồi **reset epoch** nhưng
    /// GIỮ watermark `last_entry_seq`. Gọi khi `should_close` báo (hoặc daemon chủ động).
    pub fn close(&mut self) -> ClosedEpoch {
        let closed = ClosedEpoch {
            sub_mmr_root: self.cp.state_root(),
            sub_size: self.cp.size(),
            entries: self.cp.len(),
            entries_serialized: serialize_batch(&self.entries),
        };
        // Reset epoch — watermark sống tiếp.
        self.cp = Checkpoint::new();
        self.entries.clear();
        self.epoch_start_ts = None;
        self.oldest_ts = None;
        closed
    }
}

/// Serialize blob lô canonical: `u32_be(count) ‖ entry_bytes(seq, payload) lặp`. Mỗi entry
/// tự mang length-prefix nên nối chuỗi bất nhập nhằng; `count` để parse strict biết dừng.
///
/// Không trả lỗi: `entries` đến từ `push` đã guard `PayloadTooLarge`; `count` bị chặn
/// bởi `max_entries` (≤ u32) trên đường push. Dùng `u32_be`/`entry_bytes` an toàn.
pub fn serialize_batch(entries: &[(u64, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&u32_be(entries.len()));
    for (seq, payload) in entries {
        // payload đã ≤ u32::MAX (đường push); entry_bytes chỉ fail khi vượt — bỏ qua an toàn.
        if let Ok(eb) = entry_bytes(*seq, payload) {
            out.extend_from_slice(&eb);
        }
    }
    out
}

/// Parse strict blob lô → `[(entry_seq, payload)]`. Byte cụt / count sai / **thừa byte đuôi**
/// đều `MalformedBatch` (không đoán). Dùng để dựng lại đúng `sub_mmr_root` (§8.3c).
pub fn parse_batch(bytes: &[u8]) -> Result<Vec<(u64, Vec<u8>)>, BatchError> {
    if bytes.len() < 4 {
        return Err(BatchError::MalformedBatch);
    }
    let count = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let mut pos = 4;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        if pos + 12 > bytes.len() {
            return Err(BatchError::MalformedBatch); // thiếu header entry
        }
        let seq = u64::from_be_bytes(bytes[pos..pos + 8].try_into().unwrap());
        let len = u32::from_be_bytes(bytes[pos + 8..pos + 12].try_into().unwrap()) as usize;
        pos += 12;
        if pos + len > bytes.len() {
            return Err(BatchError::MalformedBatch); // payload cụt
        }
        out.push((seq, bytes[pos..pos + len].to_vec()));
        pos += len;
    }
    if pos != bytes.len() {
        return Err(BatchError::MalformedBatch); // thừa byte đuôi
    }
    Ok(out)
}

/// Dựng lại `sub_mmr_root` từ danh sách entry (đã parse) — dùng đối chiếu blob với root neo.
pub fn batch_root(entries: &[(u64, Vec<u8>)]) -> Result<Hash32, BatchError> {
    let mut cp = Checkpoint::new();
    for (seq, payload) in entries {
        cp.append_entry(*seq, payload)?;
    }
    Ok(cp.state_root())
}

/// Bằng chứng inclusion hai tầng: entry `entry_idx` ∈ checkpoint ∈ lịch sử đã neo.
///
/// KHÔNG chứa `checkpoint_vh`/`checkpoint_state_root`/`checkpoint_seq` prover tự khai:
/// `verify_two_tier` nhận thẳng `&StrataVersion` canonical của checkpoint và TỰ tính
/// `version_hash()` + đọc `state_root` từ chính version đó → LINK là hệ quả cấu trúc, không
/// còn là phép so sánh hai input rời (review #7).
#[derive(Debug, Clone)]
pub struct TwoTierProof {
    /// Index entry trong sub-MMR.
    pub entry_idx: usize,
    /// leaf-data entry (`H_dom(entry/v1, entry_bytes)`).
    pub entry_leaf: Hash32,
    /// Proof entry dưới `checkpoint.state_root`.
    pub sub_proof: InclusionProof,
    /// Số lá sub-MMR.
    pub sub_size: u64,
    /// Proof version-checkpoint dưới `anchored_mmr_root`.
    pub ver_proof: InclusionProof,
    /// Số version trong chain (n của MMR chính).
    pub chain_size: u64,
}

/// Verify hai tầng dưới `anchored_mmr_root` (MMR root đã neo on-chain, INV-E7). `checkpoint`
/// là `StrataVersion` canonical của version-checkpoint (verifier lấy từ chain/blob đã neo):
///
/// 1. entry ∈ checkpoint: `sub_proof` dưới `checkpoint.state_root` (LINK cấu trúc — root
///    lấy TỪ version đã neo, không phải input rời prover khai);
/// 2. version-checkpoint ∈ lịch sử: `ver_proof` dưới `anchored_mmr_root`, với
///    `version_hash()`+`seq` tính TỪ chính `checkpoint`.
///
/// Trả `true` chỉ khi CẢ HAI đúng. Bất kỳ tầng nào sai ⇒ `false` (fail-closed).
pub fn verify_two_tier(
    anchored_mmr_root: Hash32,
    checkpoint: &StrataVersion,
    p: &TwoTierProof,
) -> bool {
    // (1) tầng dưới — dưới state_root ĐỌC TỪ version đã neo (LINK cấu trúc).
    if !Checkpoint::verify_entry(
        checkpoint.state_root,
        &p.entry_leaf,
        p.entry_idx,
        p.sub_size,
        &p.sub_proof,
    ) {
        return false;
    }
    // (2) tầng trên — version_hash()+seq tính từ chính checkpoint.
    StrataChain::verify_version(
        anchored_mmr_root,
        &checkpoint.version_hash(),
        checkpoint.seq,
        p.chain_size,
        &p.ver_proof,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::{Policy, StrataChain};
    use crate::version::StrataVersion;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    // ---- helper dựng chain tối thiểu (mirror chain.rs test) ----
    struct Author {
        did: [u8; 32],
        sk: SigningKey,
    }
    fn mk_author(tag: u8) -> Author {
        Author {
            did: [tag; 32],
            sk: SigningKey::generate(&mut OsRng),
        }
    }
    fn policy_with(a: &Author) -> Policy {
        let mut p = Policy::new();
        p.allow(a.did, a.sk.verifying_key());
        p
    }
    fn signed_sr(
        seq: u64,
        prev: Hash32,
        ts: u64,
        a: &Author,
        ph: Hash32,
        sr: Hash32,
    ) -> StrataVersion {
        let mut v = StrataVersion::unsigned(seq, prev, b"cid".to_vec(), sr, a.did, ph, ts);
        v.sign(&a.sk);
        v
    }
    /// genesis (seq0) + version-checkpoint (seq1) mang `state_root`=cp_root.
    fn chain_with_checkpoint(a: &Author, cp_root: Hash32) -> (StrataChain, u64) {
        let pol = policy_with(a);
        let ph = pol.policy_hash();
        let v0 = signed_sr(0, [0u8; 32], 100, a, ph, [0x11; 32]);
        let mut chain = StrataChain::genesis([0xAB; 32], v0, &pol).unwrap();
        let v1 = signed_sr(1, chain.head_version_hash(), 200, a, ph, cp_root);
        chain.append_version(v1, &pol).unwrap();
        (chain, 1)
    }
    /// Checkpoint gộp `n` entry (seq=i, payload="m-i").
    fn cp_of(n: u64) -> Checkpoint {
        let mut cp = Checkpoint::new();
        for i in 0..n {
            cp.append_entry(i, format!("m-{i}").as_bytes()).unwrap();
        }
        cp
    }

    // ===== §8.3 tiêu chí test =====

    /// #1 `checkpoint_1000_versions`: 1000 entry → 1 checkpoint → 1 anchor (KHÔNG 1000).
    #[test]
    fn checkpoint_1000_versions_one_anchor() {
        let cp = cp_of(1000);
        assert_eq!(cp.len(), 1000);
        let a = mk_author(1);
        let (mut chain, _) = chain_with_checkpoint(&a, cp.state_root());
        let anchor = chain.publish_anchor().unwrap();
        assert_eq!(chain.len(), 2, "chỉ genesis + 1 checkpoint-version");
        assert_eq!(anchor.seq, 1, "một anchor duy nhất gói trọn 1000 entry");
    }

    /// #2 `prove_entry_in_checkpoint`: prove entry bất kỳ; báo cáo kích thước proof.
    #[test]
    fn prove_entry_in_checkpoint_size() {
        let cp = cp_of(1000);
        let (proof, size, leaf) = cp.prove_entry(637).unwrap();
        assert!(Checkpoint::verify_entry(
            cp.state_root(),
            &leaf,
            637,
            size,
            &proof
        ));
        let bytes = proof_hash_bytes(&proof);
        println!(
            "[S3 #2] sub_proof(N=1000, idx=637) = {bytes}B ({} siblings + {} peaks)",
            proof.siblings.len(),
            proof.peaks.len()
        );
        assert!(bytes < 1024, "proof tầng dưới O(log N), thực tế {bytes}B");
        let bad = entry_leaf(637, b"khac").unwrap();
        assert!(!Checkpoint::verify_entry(
            cp.state_root(),
            &bad,
            637,
            size,
            &proof
        ));
    }

    /// #3 `two_tier_inclusion`: ghép sub-proof + version-proof → verify về mmr_root neo.
    /// verify_two_tier nhận thẳng &StrataVersion (review #7) — LINK là hệ quả cấu trúc.
    #[test]
    fn two_tier_inclusion_verifies() {
        let cp = cp_of(500);
        let a = mk_author(2);
        let (chain, cp_seq) = chain_with_checkpoint(&a, cp.state_root());
        let anchored_root = chain.mmr_root();

        let idx = 321usize;
        let (sub_proof, sub_size, entry_leaf_v) = cp.prove_entry(idx).unwrap();
        let (ver_proof, chain_size, _vh) = chain.prove_version(cp_seq).unwrap();
        let proof = TwoTierProof {
            entry_idx: idx,
            entry_leaf: entry_leaf_v,
            sub_proof,
            sub_size,
            ver_proof,
            chain_size,
        };
        let cp_version = chain.version(cp_seq).unwrap();
        assert!(verify_two_tier(anchored_root, cp_version, &proof));

        // entry_idx sai → reject (fail-closed).
        let mut wrong_idx = proof.clone();
        wrong_idx.entry_idx = 320;
        assert!(!verify_two_tier(anchored_root, cp_version, &wrong_idx));

        // version có state_root khác (checkpoint giả) → tầng dưới fail (LINK cấu trúc).
        let genesis = chain.version(0).unwrap();
        assert!(!verify_two_tier(anchored_root, genesis, &proof));
    }

    /// #4 `close_on_max_entries`: vượt `max_entries` → đóng tại đúng trần.
    #[test]
    fn close_on_max_entries() {
        let pol = BatchPolicy::default(); // max=10_000
        assert_eq!(pol.should_close(9_999, 0, 100, 200), None);
        assert_eq!(
            pol.should_close(10_000, 0, 100, 200),
            Some(CloseReason::MaxEntries)
        );
        assert_eq!(
            pol.should_close(10_001, 0, 100, 200),
            Some(CloseReason::MaxEntries)
        );
    }

    /// #5 `close_on_flush_max_age`: entry đầu già `flush_max_age` → đóng DÙ entry mới rả
    /// rích (chứng minh ngữ nghĩa oldest-age ≠ idle). oldest_ts = min, epoch_start riêng.
    #[test]
    fn close_on_flush_max_age() {
        let pol = BatchPolicy::default(); // flush_max_age=300, epoch=3600
        // oldest=1000, epoch_start=1000: 250s chưa tới ngưỡng → None.
        assert_eq!(pol.should_close(5, 1000, 1000, 1250), None);
        // tuổi oldest = 300 → FlushMaxAge (dù epoch mới trôi 300s « 3600).
        assert_eq!(
            pol.should_close(5, 1000, 1000, 1300),
            Some(CloseReason::FlushMaxAge)
        );

        // "rả rích" qua accumulator: tin mới liên tục nhưng oldest_ts đứng yên.
        let mut acc = EpochAccumulator::new(pol);
        acc.push(0, 1000, b"first", 1000).unwrap();
        for (k, ts) in (1..6u64).zip((1060..).step_by(60)) {
            acc.push(k, ts, b"rar-rich", ts).unwrap(); // entry mới, ts tăng
        }
        // now=1300: entry mới nhất vừa tới (ts=1300) nhưng oldest=1000 đã chờ 300s.
        assert_eq!(acc.should_close(1300), Some(CloseReason::FlushMaxAge));
    }

    /// #6 `entry_bytes_canonical`: khác payload/seq → leaf khác; khung u64‖u32‖payload đúng.
    #[test]
    fn entry_bytes_canonical() {
        assert_ne!(
            entry_leaf(0, b"aaaa").unwrap(),
            entry_leaf(0, b"bbbb").unwrap()
        );
        assert_ne!(entry_leaf(0, b"x").unwrap(), entry_leaf(1, b"x").unwrap());
        assert_ne!(
            entry_leaf(9, b"abc").unwrap(),
            entry_leaf(9, b"ab").unwrap()
        );
        let eb = entry_bytes(0x0102030405060708, b"hi").unwrap();
        assert_eq!(&eb[0..8], &0x0102030405060708u64.to_be_bytes());
        assert_eq!(&eb[8..12], &2u32.to_be_bytes());
        assert_eq!(&eb[12..], b"hi");
    }

    /// #7 `crdt_deterministic_state_root`: cùng TẬP op, thứ tự nhận khác → caller sort tất
    /// định trước append → cùng `checkpoint_state_root`.
    #[test]
    fn crdt_deterministic_state_root() {
        let ops: Vec<(u64, &[u8])> = vec![(0, b"op-a"), (1, b"op-b"), (2, b"op-c")];
        let build = |order: &[usize]| {
            let mut cp = Checkpoint::new();
            let mut items: Vec<_> = order.iter().map(|&i| ops[i]).collect();
            items.sort_by_key(|(seq, _)| *seq);
            for (seq, payload) in items {
                cp.append_entry(seq, payload).unwrap();
            }
            cp.state_root()
        };
        assert_eq!(build(&[0, 1, 2]), build(&[2, 0, 1]));
    }

    /// Tất định tổng: cùng chuỗi entry → cùng state_root.
    #[test]
    fn deterministic_same_entries_same_root() {
        assert_eq!(cp_of(64).state_root(), cp_of(64).state_root());
    }

    // ===== review #8 — test bổ sung =====

    /// Chống replay `entry_seq` SỐNG XUYÊN close: sau close, seq cũ/bằng vẫn bị từ chối.
    #[test]
    fn replay_seq_across_epoch_close() {
        let mut acc = EpochAccumulator::new(BatchPolicy::default());
        acc.push(0, 100, b"a", 100).unwrap();
        acc.push(1, 101, b"b", 101).unwrap();
        let _ = acc.close(); // đóng epoch — watermark last=1 phải sống tiếp
        // seq bằng watermark → replay.
        assert_eq!(
            acc.push(1, 200, b"x", 200),
            Err(BatchError::ReplaySeq { last: 1, got: 1 })
        );
        // seq nhỏ hơn → replay.
        assert_eq!(
            acc.push(0, 200, b"x", 200),
            Err(BatchError::ReplaySeq { last: 1, got: 0 })
        );
        // seq lớn hơn → chấp nhận vào epoch mới.
        assert_eq!(acc.push(2, 200, b"c", 200), Ok(0));
    }

    /// `max_entries` enforce tại push (`EpochFull`); entry dư thuộc epoch SAU (push lại được
    /// sau close, KHÔNG bị coi replay vì watermark chưa cập nhật lúc EpochFull).
    #[test]
    fn max_entries_enforced_at_push() {
        let pol = BatchPolicy {
            epoch_secs: 3600,
            max_entries: 2,
            flush_max_age: 300,
        };
        let mut acc = EpochAccumulator::new(pol);
        assert_eq!(acc.push(0, 1, b"a", 1), Ok(0));
        assert_eq!(acc.push(1, 2, b"b", 2), Ok(1));
        // đầy → EpochFull, KHÔNG ghi, watermark giữ nguyên =1.
        assert_eq!(
            acc.push(2, 3, b"c", 3),
            Err(BatchError::EpochFull { max_entries: 2 })
        );
        assert_eq!(acc.len(), 2);
        assert_eq!(acc.last_entry_seq(), Some(1));
        // đóng rồi push lại chính entry bị EpochFull → nhận vào epoch mới (không replay).
        let _ = acc.close();
        assert_eq!(acc.push(2, 3, b"c", 3), Ok(0));
    }

    /// `max_entries = 0` fail-closed: mọi push EpochFull, epoch rỗng KHÔNG đóng lặp vô hạn.
    #[test]
    fn max_entries_zero_fail_closed() {
        let pol = BatchPolicy {
            epoch_secs: 3600,
            max_entries: 0,
            flush_max_age: 300,
        };
        let mut acc = EpochAccumulator::new(pol);
        assert_eq!(
            acc.push(0, 1, b"a", 1),
            Err(BatchError::EpochFull { max_entries: 0 })
        );
        assert_eq!(acc.len(), 0);
        // epoch rỗng: should_close luôn None (không đóng lặp vô hạn checkpoint rỗng).
        assert_eq!(acc.should_close(999_999), None);
    }

    /// Epoch RỖNG không bao giờ đóng (góc chết cũ: should_close(0,…) từng trả EpochElapsed).
    #[test]
    fn empty_epoch_never_closes() {
        let pol = BatchPolicy::default();
        assert_eq!(pol.should_close(0, 0, 1000, 5_000_000), None);
        let acc = EpochAccumulator::new(pol);
        assert_eq!(acc.should_close(5_000_000), None);
    }

    /// ts LÙI trong epoch: không đòi ts đơn điệu; `oldest_ts` = min → flush đo từ min.
    #[test]
    fn non_monotonic_ts_uses_min() {
        let mut acc = EpochAccumulator::new(BatchPolicy::default());
        acc.push(0, 1000, b"a", 1000).unwrap();
        acc.push(1, 980, b"b", 1000).unwrap(); // ts LÙI — vẫn chấp nhận
        acc.push(2, 1010, b"c", 1000).unwrap();
        // oldest=980: now=1279 → 299s chưa tới; now=1280 → 300s tới ngưỡng.
        assert_eq!(acc.should_close(1279), None);
        assert_eq!(acc.should_close(1280), Some(CloseReason::FlushMaxAge));
    }

    /// Clock-skew (now < mốc): saturating_sub → None, KHÔNG panic.
    #[test]
    fn clock_skew_no_panic() {
        let pol = BatchPolicy::default();
        assert_eq!(pol.should_close(5, 5000, 5000, 100), None);
        let mut acc = EpochAccumulator::new(pol);
        acc.push(0, 5000, b"a", 5000).unwrap();
        assert_eq!(acc.should_close(100), None); // now lùi sau mốc mở epoch
    }

    /// Push bị từ chối KHÔNG ghi gì (len/watermark bất biến) — ReplaySeq.
    #[test]
    fn rejected_push_writes_nothing() {
        let mut acc = EpochAccumulator::new(BatchPolicy::default());
        acc.push(5, 10, b"a", 10).unwrap();
        let before = acc.len();
        assert!(acc.push(5, 11, b"dup", 11).is_err());
        assert_eq!(acc.len(), before, "push từ chối không tăng len");
        assert_eq!(acc.last_entry_seq(), Some(5), "watermark bất biến");
    }

    /// Payload RỖNG hợp lệ.
    #[test]
    fn empty_payload_valid() {
        let mut acc = EpochAccumulator::new(BatchPolicy::default());
        assert_eq!(acc.push(0, 1, b"", 1), Ok(0));
        assert_eq!(acc.len(), 1);
        assert!(entry_leaf(0, b"").is_ok());
    }

    /// Lifecycle accumulator: push N → close → ClosedEpoch khớp Checkpoint dựng trực tiếp;
    /// blob canonical round-trip parse đúng root.
    #[test]
    fn accumulator_close_matches_direct_checkpoint() {
        let mut acc = EpochAccumulator::new(BatchPolicy::default());
        let mut direct = Checkpoint::new();
        for i in 0..500u64 {
            let p = format!("v{i}");
            acc.push(i, 1000 + i, p.as_bytes(), 1000 + i).unwrap();
            direct.append_entry(i, p.as_bytes()).unwrap();
        }
        let closed = acc.close();
        assert_eq!(closed.entries, 500);
        assert_eq!(closed.sub_mmr_root, direct.state_root());
        assert_eq!(closed.sub_size, direct.size());
        // blob parse strict → dựng lại đúng root.
        let parsed = parse_batch(&closed.entries_serialized).unwrap();
        assert_eq!(parsed.len(), 500);
        assert_eq!(batch_root(&parsed).unwrap(), closed.sub_mmr_root);
    }

    /// Tamper payload SAU đóng: đối chứng pass; sửa 1 byte → root lệch; cụt/thừa → Malformed.
    #[test]
    fn tamper_batch_blob_detected() {
        let mut acc = EpochAccumulator::new(BatchPolicy::default());
        for i in 0..10u64 {
            acc.push(i, 100 + i, format!("p{i}").as_bytes(), 100 + i)
                .unwrap();
        }
        let closed = acc.close();
        // đối chứng: blob nguyên vẹn → root khớp.
        let ok = parse_batch(&closed.entries_serialized).unwrap();
        assert_eq!(batch_root(&ok).unwrap(), closed.sub_mmr_root);

        // sửa 1 byte payload (byte cuối) → parse vẫn OK nhưng root lệch.
        let mut tampered = closed.entries_serialized.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF;
        let t = parse_batch(&tampered).unwrap();
        assert_ne!(
            batch_root(&t).unwrap(),
            closed.sub_mmr_root,
            "root phát hiện tamper"
        );

        // cụt đuôi → Malformed.
        let truncated = &closed.entries_serialized[..closed.entries_serialized.len() - 1];
        assert_eq!(parse_batch(truncated), Err(BatchError::MalformedBatch));
        // thừa byte đuôi → Malformed.
        let mut extra = closed.entries_serialized.clone();
        extra.push(0x00);
        assert_eq!(parse_batch(&extra), Err(BatchError::MalformedBatch));
        // count=0 nhưng có byte thừa → Malformed.
        assert_eq!(
            parse_batch(&[0, 0, 0, 0, 0xAA]),
            Err(BatchError::MalformedBatch)
        );
        // quá ngắn cho count → Malformed.
        assert_eq!(parse_batch(&[0, 0]), Err(BatchError::MalformedBatch));
    }

    /// Profile ProofChat đúng §8.3 {600, 4096, 180}.
    #[test]
    fn proofchat_profile() {
        let p = BatchPolicy::proofchat();
        assert_eq!(p.epoch_secs, 600);
        assert_eq!(p.max_entries, 4096);
        assert_eq!(p.flush_max_age, 180);
    }

    /// Ưu tiên close: MaxEntries > EpochElapsed > FlushMaxAge (khi nhiều điều kiện cùng đúng).
    #[test]
    fn close_priority_order() {
        let pol = BatchPolicy {
            epoch_secs: 100,
            max_entries: 3,
            flush_max_age: 50,
        };
        // count đủ trần + epoch hết + oldest già → ưu tiên MaxEntries.
        assert_eq!(
            pol.should_close(3, 0, 0, 1000),
            Some(CloseReason::MaxEntries)
        );
        // dưới trần, epoch hết + oldest già → EpochElapsed trước FlushMaxAge.
        assert_eq!(
            pol.should_close(2, 0, 0, 1000),
            Some(CloseReason::EpochElapsed)
        );
    }
}
