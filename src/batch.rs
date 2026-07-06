//! `BatchPolicy` / checkpoint sub-MMR — gộp lô tần suất cao (S3, Strata-Math §8,
//! Strata-API §5.3/§8.3).
//!
//! Register/IoT/chat cập nhật mỗi giây: mỗi cập nhật KHÔNG đẻ một version. Thay vào
//! đó, các entry trong một **epoch** gom vào một **sub-MMR**; đóng epoch → root
//! sub-MMR trở thành `state_root` của MỘT version checkpoint bình thường
//! (`chain.append_version`). Từng entry vẫn có inclusion-proof hai tầng
//! `O(log N + log n)` về `mmr_root` đã neo.
//!
//! - Leaf sub-MMR = `Mmr::append(H_dom("LN/STRATA/entry/v1", entry_bytes))` — miền
//!   entry TÁCH khỏi miền version-hash (một entry không thể nhầm là một version).
//! - `entry_bytes = u64_be(entry_seq) ‖ u32_be(len(payload)) ‖ payload` (canonical,
//!   length-prefixed — chống nhập nhằng nối chuỗi như §1.7). `ts` KHÔNG vào leaf:
//!   nó chỉ điều khiển van đóng epoch; payload cần cam kết thời gian thì tự nhúng.
//! - Core THUẦN no-I/O: `now` do caller truyền vào (test tất định, không SystemTime);
//!   `content_cid` của blob lô do caller cung cấp (core không gọi Mirage).
//!
//! Ba van đóng epoch (BẤT KỲ van nào chạm là đóng):
//! (a) `now - epoch_start ≥ epoch_secs`;
//! (b) `entries ≥ max_entries`;
//! (c) `now - ts(entry cũ nhất) ≥ flush_max_age` — **tuổi entry cũ nhất**, KHÔNG
//!     phải "im lặng": chuỗi tin nhịp chậm rả rích vẫn gom được vào một checkpoint,
//!     nhưng không entry nào chờ quá `flush_max_age`.

use crate::chain::StrataChain;
use crate::u32_be;
use crate::version::{Hash32, StrataVersion};
use lampnet_merkle_anchor::Blake3Hasher;
use lampnet_merkle_anchor::hash::h_dom;
use lampnet_merkle_anchor::mmr::{InclusionProof, Mmr, verify as mmr_verify};

/// Tag miền cho batch entry (sub-MMR gộp lô) — khớp bảng CHỐT-2 `_CONTRACT.md`.
pub const TAG_ENTRY: &str = "LN/STRATA/entry/v1";

/// Lỗi lớp gộp lô (tách khỏi [`crate::chain::StrataError`] — vòng đời epoch là
/// lớp riêng, không đụng invariant chain).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchError {
    /// Idempotency: `entry_seq` phải TĂNG NGHIÊM NGẶT toàn-chain (xuyên epoch).
    /// Nhận lại seq cũ/bằng = replay → từ chối, không ghi.
    EntrySeqReplay { last: u64, got: u64 },
    /// Epoch hiện tại đã đầy (`entries == max_entries`): entry này thuộc epoch SAU.
    /// Caller phải [`EpochAccumulator::close`] rồi push lại.
    EpochFull { max_entries: u32 },
    /// Payload vượt `u32::MAX` byte — length-prefix u32 không biểu diễn được
    /// (chống truncate lặng lẽ khi encode canonical).
    PayloadTooLarge { len: usize },
}

impl std::fmt::Display for BatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for BatchError {}

/// Chính sách đóng epoch. Ba van — xem doc module. Defaults khớp spec §5.3/§8.3:
/// `epoch_secs=3600` (khớp `EPOCH_DURATION_SECS` Reward), `max_entries=10_000`
/// (chống RAM phình), `flush_max_age=300`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchPolicy {
    /// Van (a): tuổi epoch tối đa (giây) kể từ push đầu tiên.
    pub epoch_secs: u64,
    /// Van (b): số entry tối đa một epoch. `0` = fail-closed (mọi push bị
    /// [`BatchError::EpochFull`]) — cấu hình vô nghĩa, cố ý không lách.
    pub max_entries: u32,
    /// Van (c): tuổi tối đa (giây) của entry CŨ NHẤT đang chờ trong epoch.
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
    /// Profile ProofChat: epoch 10 phút, lô 4096 tin, không tin nào chờ quá 3 phút.
    pub fn proofchat() -> Self {
        Self {
            epoch_secs: 600,
            max_entries: 4096,
            flush_max_age: 180,
        }
    }
}

/// Một entry tần suất cao trong epoch (cập nhật giá trị / CRDT-op / tin nhắn).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchEntry {
    /// Số thứ tự toàn-chain, tăng nghiêm ngặt xuyên epoch (idempotency).
    pub entry_seq: u64,
    /// Unix secs lúc entry sinh ra — CHỈ điều khiển van (c), KHÔNG vào leaf.
    pub ts: u64,
    /// Nội dung serialize tất định (giá trị đo / CRDT-op).
    pub payload: Vec<u8>,
}

impl BatchEntry {
    /// Bytes canonical: `u64_be(entry_seq) ‖ u32_be(len(payload)) ‖ payload`.
    /// Tự-phân-định (self-delimiting) → nối chuỗi nhiều entry parse lại không nhập nhằng.
    pub fn entry_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(8 + 4 + self.payload.len());
        b.extend_from_slice(&self.entry_seq.to_be_bytes());
        b.extend_from_slice(&u32_be(self.payload.len()));
        b.extend_from_slice(&self.payload);
        b
    }

    /// Dữ liệu lá đưa vào sub-MMR: `H_dom(TAG_ENTRY, entry_bytes)` — tách miền
    /// entry khỏi version-hash TRƯỚC khi `Mmr::append` bọc thêm RFC6962 leaf-prefix.
    pub fn leaf_data(&self) -> Hash32 {
        h_dom(TAG_ENTRY, &self.entry_bytes())
    }
}

/// Dữ liệu lá của một entry từ cặp `(entry_seq, payload)` — cho verifier KHÔNG giữ
/// [`BatchEntry`] (ts không tham gia leaf).
pub fn entry_leaf_data(entry_seq: u64, payload: &[u8]) -> Hash32 {
    let mut b = Vec::with_capacity(8 + 4 + payload.len());
    b.extend_from_slice(&entry_seq.to_be_bytes());
    b.extend_from_slice(&u32_be(payload.len()));
    b.extend_from_slice(payload);
    h_dom(TAG_ENTRY, &b)
}

/// Bộ gom epoch: nhận entry, hỏi [`EpochAccumulator::should_close`], đóng bằng
/// [`EpochAccumulator::close`]. Vòng lặp caller (daemon):
///
/// ```text
/// if acc.should_close(now) {
///     let epoch = acc.close().unwrap();
///     let cid   = /* đẩy epoch.entries_serialized qua Mirage (ngoài core) */;
///     let v     = /* StrataVersion: state_root = epoch.sub_mmr_root, content_cid = cid */;
///     chain.append_version(v, &policy)?;   // checkpoint = một version BÌNH THƯỜNG
/// }
/// acc.push(entry_seq, ts, payload, now)?;  // entry đến trong lúc đóng → epoch SAU
/// ```
#[derive(Debug, Clone)]
pub struct EpochAccumulator {
    policy: BatchPolicy,
    entries: Vec<BatchEntry>,
    /// `now` tại push ĐẦU TIÊN của epoch hiện tại (van a). Chỉ có nghĩa khi có entry.
    epoch_start: u64,
    /// `ts` nhỏ nhất trong epoch hiện tại (van c — không đòi ts đơn điệu).
    oldest_ts: u64,
    /// `entry_seq` cuối đã nhận — SỐNG XUYÊN epoch (idempotency toàn-chain).
    last_entry_seq: Option<u64>,
}

impl EpochAccumulator {
    /// Bộ gom rỗng với policy cho trước.
    pub fn new(policy: BatchPolicy) -> Self {
        Self {
            policy,
            entries: Vec::new(),
            epoch_start: 0,
            oldest_ts: 0,
            last_entry_seq: None,
        }
    }

    /// Policy đang áp dụng.
    pub fn policy(&self) -> &BatchPolicy {
        &self.policy
    }

    /// Số entry đang chờ trong epoch hiện tại.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Epoch hiện tại rỗng?
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Nhận một entry vào epoch hiện tại. `now` = giờ caller (core không tự lấy giờ).
    /// Trả index trong epoch (0-based). Enforce:
    /// - `entry_seq` tăng nghiêm ngặt toàn-chain → [`BatchError::EntrySeqReplay`];
    /// - epoch đầy → [`BatchError::EpochFull`] (entry thuộc epoch SAU — close rồi push lại);
    /// - payload ≤ u32::MAX byte → [`BatchError::PayloadTooLarge`].
    ///
    /// Fail-closed: mọi từ chối KHÔNG ghi gì (epoch giữ nguyên).
    pub fn push(
        &mut self,
        entry_seq: u64,
        ts: u64,
        payload: Vec<u8>,
        now: u64,
    ) -> Result<usize, BatchError> {
        if payload.len() > u32::MAX as usize {
            return Err(BatchError::PayloadTooLarge { len: payload.len() });
        }
        if let Some(last) = self.last_entry_seq
            && entry_seq <= last
        {
            return Err(BatchError::EntrySeqReplay {
                last,
                got: entry_seq,
            });
        }
        if self.entries.len() >= self.policy.max_entries as usize {
            return Err(BatchError::EpochFull {
                max_entries: self.policy.max_entries,
            });
        }
        if self.entries.is_empty() {
            self.epoch_start = now;
            self.oldest_ts = ts;
        } else if ts < self.oldest_ts {
            self.oldest_ts = ts;
        }
        let idx = self.entries.len();
        self.last_entry_seq = Some(entry_seq);
        self.entries.push(BatchEntry {
            entry_seq,
            ts,
            payload,
        });
        Ok(idx)
    }

    /// Epoch hiện tại NÊN đóng chưa? (ba van — BẤT KỲ van nào chạm). Epoch rỗng
    /// không bao giờ đóng. Saturating: lệch đồng hồ (`now` lùi) không panic, chỉ
    /// làm tuổi = 0.
    pub fn should_close(&self, now: u64) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        // (a) tuổi epoch.
        if now.saturating_sub(self.epoch_start) >= self.policy.epoch_secs {
            return true;
        }
        // (b) số entry.
        if self.entries.len() >= self.policy.max_entries as usize {
            return true;
        }
        // (c) tuổi entry cũ nhất — KHÔNG phải "im lặng": entry mới vẫn rả rích
        // không reset van này.
        if now.saturating_sub(self.oldest_ts) >= self.policy.flush_max_age {
            return true;
        }
        false
    }

    /// Đóng epoch hiện tại: dựng sub-MMR (leaf = `H_dom(entry/v1, entry_bytes)`),
    /// trả [`ClosedEpoch`]. `None` nếu epoch rỗng. Bộ gom reset cho epoch SAU;
    /// `last_entry_seq` GIỮ NGUYÊN (chống replay xuyên epoch).
    pub fn close(&mut self) -> Option<ClosedEpoch> {
        if self.entries.is_empty() {
            return None;
        }
        let entries = std::mem::take(&mut self.entries);
        self.epoch_start = 0;
        self.oldest_ts = 0;

        let mut mmr = Mmr::<Blake3Hasher>::new();
        let mut serialized = Vec::new();
        for e in &entries {
            let eb = e.entry_bytes();
            mmr.append(&h_dom(TAG_ENTRY, &eb));
            serialized.extend_from_slice(&eb);
        }
        Some(ClosedEpoch {
            sub_mmr_root: mmr.root(),
            sub_size: mmr.len(),
            entries,
            entries_serialized: serialized,
            mmr,
        })
    }
}

/// Kết quả đóng một epoch — đủ để caller: (1) dựng version checkpoint
/// (`state_root = sub_mmr_root`, `content_cid` = CID của `entries_serialized`
/// caller đẩy qua Mirage); (2) sinh sub-proof cho entry lẻ về sau.
///
/// **Chốt lưu (§8.3c):** vứt `entries`/blob = mất khả năng prove entry lẻ
/// (chỉ còn prove cả checkpoint). Blob parse lại bằng [`parse_entries`].
pub struct ClosedEpoch {
    /// Root sub-MMR (commit n, CHỐT-3) — chính là `state_root` của checkpoint.
    pub sub_mmr_root: Hash32,
    /// Số entry trong lô (n của sub-MMR — verifier cần).
    pub sub_size: u64,
    /// Các entry theo thứ tự append (đủ sinh proof).
    pub entries: Vec<BatchEntry>,
    /// Bytes canonical của cả lô: nối `entry_bytes` (tự-phân-định) — caller đẩy
    /// Mirage làm blob, `content_cid = gen_content_cid(entries_serialized)`.
    pub entries_serialized: Vec<u8>,
    mmr: Mmr<Blake3Hasher>,
}

impl std::fmt::Debug for ClosedEpoch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClosedEpoch")
            .field("sub_mmr_root", &self.sub_mmr_root)
            .field("sub_size", &self.sub_size)
            .finish()
    }
}

impl ClosedEpoch {
    /// Sub-proof cho entry tại `index` (0-based trong lô). Trả
    /// `(proof, sub_size, leaf_data)`. `None` nếu index ngoài phạm vi.
    pub fn prove_entry(&self, index: usize) -> Option<(InclusionProof, u64, Hash32)> {
        if index >= self.entries.len() {
            return None;
        }
        Some((
            self.mmr.prove(index),
            self.sub_size,
            self.entries[index].leaf_data(),
        ))
    }
}

/// Parse blob `entries_serialized` về danh sách `(entry_seq, payload)` — để dựng
/// lại sub-MMR/proof từ blob Mirage (`ts` không nằm trong blob: nó không vào leaf).
/// `None` nếu blob cụt/thừa byte (strict — không nhận blob nhập nhằng).
pub fn parse_entries(bytes: &[u8]) -> Option<Vec<(u64, Vec<u8>)>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes.len() - i < 12 {
            return None;
        }
        let seq = u64::from_be_bytes(bytes[i..i + 8].try_into().ok()?);
        let len = u32::from_be_bytes(bytes[i + 8..i + 12].try_into().ok()?) as usize;
        i += 12;
        if bytes.len() - i < len {
            return None;
        }
        out.push((seq, bytes[i..i + len].to_vec()));
        i += len;
    }
    Some(out)
}

/// Verify TẦNG DƯỚI: entry `(entry_seq, payload)` tại `index` thuộc sub-MMR có
/// root `sub_root` (= `state_root` của version checkpoint).
pub fn verify_entry(
    sub_root: Hash32,
    entry_seq: u64,
    payload: &[u8],
    index: usize,
    sub_size: u64,
    sub_proof: &InclusionProof,
) -> bool {
    let leaf = entry_leaf_data(entry_seq, payload);
    mmr_verify::<Blake3Hasher>(sub_root, &leaf, index, sub_size, sub_proof)
}

/// Verify HAI TẦNG (§8.3c): entry ∈ checkpoint ∈ lịch sử đã neo.
///
/// - Tầng dưới: `(entry_seq, payload)` tại `entry_index` dưới
///   `checkpoint.state_root` (= sub-MMR root — điểm ghép: state_root đọc từ
///   version canonical, ĐÃ băm vào `version_hash` nên không giả được).
/// - Tầng trên: `checkpoint` (tại seq của chính nó) dưới `anchored_mmr_root` với
///   `chain_mmr_size` TẠI THỜI ĐIỂM neo (§8.1c — verify dưới root cũ cần size cũ).
///
/// Hai proof đều `O(log)`. Trả `true` chỉ khi CẢ HAI tầng pass.
#[allow(clippy::too_many_arguments)]
pub fn verify_entry_two_tier(
    anchored_mmr_root: Hash32,
    checkpoint: &StrataVersion,
    chain_mmr_size: u64,
    version_proof: &InclusionProof,
    entry_seq: u64,
    payload: &[u8],
    entry_index: usize,
    sub_size: u64,
    sub_proof: &InclusionProof,
) -> bool {
    // Tầng dưới: entry dưới state_root của checkpoint.
    if !verify_entry(
        checkpoint.state_root,
        entry_seq,
        payload,
        entry_index,
        sub_size,
        sub_proof,
    ) {
        return false;
    }
    // Tầng trên: checkpoint version thuộc MMR đã neo (leaf index = seq).
    StrataChain::verify_version(
        anchored_mmr_root,
        &checkpoint.version_hash(),
        checkpoint.seq,
        chain_mmr_size,
        version_proof,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_n(acc: &mut EpochAccumulator, n: u64, ts: u64, now: u64) {
        let start = acc.last_entry_seq.map_or(0, |s| s + 1);
        for i in 0..n {
            acc.push(start + i, ts, format!("p{i}").into_bytes(), now)
                .unwrap();
        }
    }

    #[test]
    fn defaults_and_proofchat_profile() {
        let d = BatchPolicy::default();
        assert_eq!((d.epoch_secs, d.max_entries, d.flush_max_age), (3600, 10_000, 300));
        let p = BatchPolicy::proofchat();
        assert_eq!((p.epoch_secs, p.max_entries, p.flush_max_age), (600, 4096, 180));
    }

    #[test]
    fn empty_epoch_never_closes_and_close_returns_none() {
        let mut acc = EpochAccumulator::new(BatchPolicy::default());
        assert!(!acc.should_close(u64::MAX), "epoch rỗng không bao giờ đóng");
        assert!(acc.close().is_none());
    }

    #[test]
    fn valve_a_epoch_secs() {
        let mut acc = EpochAccumulator::new(BatchPolicy::default());
        acc.push(0, 1_000, b"x".to_vec(), 1_000).unwrap();
        assert!(!acc.should_close(1_000 + 3599 - 3599)); // ngay sau push
        assert!(!acc.should_close(1_000 + 299), "chưa chạm van nào");
        // flush_max_age=300 chạm TRƯỚC epoch_secs — kiểm van (a) riêng cần policy khác:
        let mut acc2 = EpochAccumulator::new(BatchPolicy {
            epoch_secs: 100,
            max_entries: 10,
            flush_max_age: 10_000,
        });
        acc2.push(0, 1_000, b"x".to_vec(), 1_000).unwrap();
        assert!(!acc2.should_close(1_099));
        assert!(acc2.should_close(1_100), "van (a): now - epoch_start >= epoch_secs");
    }

    #[test]
    fn clock_skew_saturating_no_panic() {
        // now LÙI trước epoch_start/ts (lệch đồng hồ) → tuổi 0, không panic/đóng.
        let mut acc = EpochAccumulator::new(BatchPolicy::default());
        acc.push(0, 5_000, b"x".to_vec(), 5_000).unwrap();
        assert!(!acc.should_close(0));
    }

    #[test]
    fn oldest_ts_tracks_minimum_not_first() {
        // ts KHÔNG bị buộc đơn điệu; van (c) theo entry CŨ NHẤT (min ts).
        let mut acc = EpochAccumulator::new(BatchPolicy::proofchat());
        acc.push(0, 1_000, b"a".to_vec(), 1_000).unwrap();
        acc.push(1, 900, b"b".to_vec(), 1_001).unwrap(); // ts lùi — entry cũ hơn
        assert!(!acc.should_close(1_079), "tuổi min-ts = 179 < 180");
        assert!(acc.should_close(1_080), "tuổi min-ts = 180 → đóng");
    }

    #[test]
    fn close_resets_epoch_but_keeps_seq_watermark() {
        let mut acc = EpochAccumulator::new(BatchPolicy::default());
        push_n(&mut acc, 3, 1_000, 1_000);
        let closed = acc.close().unwrap();
        assert_eq!(closed.sub_size, 3);
        assert!(acc.is_empty());
        // Replay seq 2 (đã vào epoch TRƯỚC) → từ chối xuyên epoch.
        assert_eq!(
            acc.push(2, 2_000, b"replay".to_vec(), 2_000),
            Err(BatchError::EntrySeqReplay { last: 2, got: 2 })
        );
        // seq mới tiếp tục bình thường.
        assert_eq!(acc.push(3, 2_000, b"ok".to_vec(), 2_000), Ok(0));
    }

    #[test]
    fn zero_max_entries_fail_closed() {
        let mut acc = EpochAccumulator::new(BatchPolicy {
            epoch_secs: 100,
            max_entries: 0,
            flush_max_age: 100,
        });
        assert_eq!(
            acc.push(0, 1, b"x".to_vec(), 1),
            Err(BatchError::EpochFull { max_entries: 0 })
        );
    }

    #[test]
    fn rejected_push_mutates_nothing() {
        let mut acc = EpochAccumulator::new(BatchPolicy {
            epoch_secs: 3600,
            max_entries: 2,
            flush_max_age: 300,
        });
        push_n(&mut acc, 2, 1_000, 1_000);
        let before = acc.clone();
        assert!(acc.push(9, 1_001, b"x".to_vec(), 1_001).is_err()); // EpochFull
        assert!(acc.push(1, 1_001, b"x".to_vec(), 1_001).is_err()); // Replay
        assert_eq!(acc.len(), before.len());
        assert_eq!(acc.last_entry_seq, before.last_entry_seq);
        assert_eq!(acc.oldest_ts, before.oldest_ts);
    }

    #[test]
    fn serialized_blob_parses_back_and_rebuilds_same_root() {
        let mut acc = EpochAccumulator::new(BatchPolicy::default());
        acc.push(10, 1_000, b"".to_vec(), 1_000).unwrap(); // payload rỗng hợp lệ
        acc.push(11, 1_000, b"hello".to_vec(), 1_000).unwrap();
        acc.push(12, 1_000, vec![0xFF; 100], 1_000).unwrap();
        let closed = acc.close().unwrap();

        let pairs = parse_entries(&closed.entries_serialized).unwrap();
        assert_eq!(pairs.len(), 3);
        assert_eq!(pairs[1], (11, b"hello".to_vec()));
        // Dựng lại sub-MMR từ blob → cùng root (ts không tham gia leaf).
        let mut mmr = Mmr::<Blake3Hasher>::new();
        for (seq, payload) in &pairs {
            mmr.append(&entry_leaf_data(*seq, payload));
        }
        assert_eq!(mmr.root(), closed.sub_mmr_root);
        // Blob cụt → strict reject.
        let truncated = &closed.entries_serialized[..closed.entries_serialized.len() - 1];
        assert!(parse_entries(truncated).is_none());
    }

    #[test]
    fn prove_entry_out_of_range_none() {
        let mut acc = EpochAccumulator::new(BatchPolicy::default());
        push_n(&mut acc, 2, 1_000, 1_000);
        let closed = acc.close().unwrap();
        assert!(closed.prove_entry(2).is_none());
        assert!(closed.prove_entry(1).is_some());
    }

    #[test]
    fn entry_leaf_domain_separated_from_version_leaf() {
        // Cùng một chuỗi byte, băm miền entry ≠ băm miền version — một entry
        // KHÔNG thể được diễn giải như một version-hash.
        let bytes = BatchEntry {
            entry_seq: 1,
            ts: 0,
            payload: b"x".to_vec(),
        }
        .entry_bytes();
        assert_ne!(
            h_dom(TAG_ENTRY, &bytes),
            h_dom(crate::version::TAG_VER, &bytes)
        );
    }
}
