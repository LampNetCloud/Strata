//! `Checkpoint` (sub-MMR) + `BatchPolicy` — S3: gộp N entry theo epoch thành MỘT
//! checkpoint (sub-MMR root) rồi neo **một lần** thay vì N lần. Chứng minh một entry
//! lẻ bằng **inclusion hai tầng** (entry ∈ checkpoint ∈ lịch sử đã neo).
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

use crate::chain::StrataChain;
use crate::u32_be;
use crate::version::Hash32;
use lampnet_merkle_anchor::Blake3Hasher;
use lampnet_merkle_anchor::hash::h_dom;
use lampnet_merkle_anchor::mmr::{InclusionProof, Mmr, verify as mmr_verify};

/// Domain-tag entry sub-MMR — bảng CHỐT-2 `_CONTRACT.md` ("Batch entry (sub-MMR gộp lô)").
/// Tách miền khỏi `LN/STRATA/ver/v1` (version-hash) và `LN/STRATA/mmr/leaf/v1` (leaf nền).
pub const TAG_ENTRY: &str = "LN/STRATA/entry/v1";

/// `entry_bytes` canonical (§1.7): `u64_be(entry_seq) ‖ u32_be(len(payload)) ‖ payload`.
/// `payload` = giá trị đo / CRDT-op serialize tất định (caller lo tất định).
pub fn entry_bytes(entry_seq: u64, payload: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(8 + 4 + payload.len());
    b.extend_from_slice(&entry_seq.to_be_bytes()); // u64 BE
    b.extend_from_slice(&u32_be(payload.len())); // len-prefix u32 BE
    b.extend_from_slice(payload);
    b
}

/// Leaf-data đưa vào sub-MMR = `H_dom(TAG_ENTRY, entry_bytes)` (32B) — tách miền entry.
pub fn entry_leaf(entry_seq: u64, payload: &[u8]) -> Hash32 {
    h_dom(TAG_ENTRY, &entry_bytes(entry_seq, payload))
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
    pub fn append_entry(&mut self, entry_seq: u64, payload: &[u8]) -> usize {
        let leaf = entry_leaf(entry_seq, payload);
        let idx = self.leaves.len();
        self.sub.append(&leaf);
        self.leaves.push(leaf);
        idx
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

/// Tham số gộp lô (default tất định để test — Strata-API §8.3). `max_entries`/
/// `flush_on_idle` là **config runtime** (không đóng byte-layout — anh Đức chốt issue #3b).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchPolicy {
    /// Độ dài epoch (giây) — khớp `EPOCH_DURATION_SECS` Reward.
    pub epoch_secs: u64,
    /// Trần entry/epoch (chặn sub-MMR phình RAM) — đóng sớm khi vượt.
    pub max_entries: usize,
    /// Im lặng bao lâu (giây) thì đóng epoch sớm.
    pub flush_on_idle: u64,
}

impl Default for BatchPolicy {
    fn default() -> Self {
        Self {
            epoch_secs: 3600,
            max_entries: 10_000,
            flush_on_idle: 300,
        }
    }
}

/// Lý do đóng epoch (đóng checkpoint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    /// Đạt trần `max_entries`.
    MaxEntries,
    /// Hết `epoch_secs`.
    EpochElapsed,
    /// Im lặng ≥ `flush_on_idle`.
    Idle,
}

impl BatchPolicy {
    /// Quyết định **thuần** (không timer, không I/O): có đóng epoch chưa? Ưu tiên:
    /// `MaxEntries` (chặn RAM) → `EpochElapsed` → `Idle`. `None` = tiếp tục gộp.
    ///
    /// - `count` = số entry hiện có; `epoch_start_ts` = mốc mở epoch;
    /// - `last_entry_ts` = ts entry cuối; `now` = đồng hồ hiện tại (caller cấp).
    pub fn should_close(
        &self,
        count: usize,
        epoch_start_ts: u64,
        last_entry_ts: u64,
        now: u64,
    ) -> Option<CloseReason> {
        if count >= self.max_entries {
            return Some(CloseReason::MaxEntries);
        }
        if now.saturating_sub(epoch_start_ts) >= self.epoch_secs {
            return Some(CloseReason::EpochElapsed);
        }
        if count > 0 && now.saturating_sub(last_entry_ts) >= self.flush_on_idle {
            return Some(CloseReason::Idle);
        }
        None
    }
}

/// Bằng chứng inclusion hai tầng: entry `entry_idx` ∈ checkpoint ∈ lịch sử đã neo.
#[derive(Debug, Clone)]
pub struct TwoTierProof {
    // ---- tầng dưới (entry ∈ checkpoint) ----
    /// Index entry trong sub-MMR.
    pub entry_idx: usize,
    /// leaf-data entry (`H_dom(entry/v1, entry_bytes)`).
    pub entry_leaf: Hash32,
    /// Proof entry dưới `checkpoint_state_root`.
    pub sub_proof: InclusionProof,
    /// Số lá sub-MMR.
    pub sub_size: u64,
    /// Root sub-MMR (checkpoint_state_root) — phải KHỚP `version.state_root`.
    pub checkpoint_state_root: Hash32,
    // ---- tầng trên (checkpoint version ∈ lịch sử) ----
    /// seq của version-checkpoint trong chain.
    pub checkpoint_seq: u64,
    /// version_hash của version-checkpoint.
    pub checkpoint_vh: Hash32,
    /// Proof version-checkpoint dưới `anchored_mmr_root`.
    pub ver_proof: InclusionProof,
    /// Số version trong chain (n của MMR chính).
    pub chain_size: u64,
}

/// Verify hai tầng dưới `anchored_mmr_root` (MMR root đã neo on-chain, INV-E7):
/// 1. entry ∈ checkpoint (sub_proof dưới `checkpoint_state_root`);
/// 2. version-checkpoint ∈ lịch sử (ver_proof dưới `anchored_mmr_root`);
/// 3. **LINK**: `version.state_root == checkpoint_state_root` (verifier đọc canonical
///    version-checkpoint để lấy `version_state_root`, chống ghép nhầm sub-MMR khác).
///
/// Trả `true` chỉ khi CẢ BA đúng. Bất kỳ tầng nào sai ⇒ `false` (fail-closed).
pub fn verify_two_tier(
    anchored_mmr_root: Hash32,
    version_state_root: Hash32,
    p: &TwoTierProof,
) -> bool {
    // (3) LINK trước — rẻ nhất, chặn ghép sub-MMR không thuộc version đã neo.
    if version_state_root != p.checkpoint_state_root {
        return false;
    }
    // (1) tầng dưới.
    if !Checkpoint::verify_entry(
        p.checkpoint_state_root,
        &p.entry_leaf,
        p.entry_idx,
        p.sub_size,
        &p.sub_proof,
    ) {
        return false;
    }
    // (2) tầng trên (tái dùng verify_version của StrataChain).
    StrataChain::verify_version(
        anchored_mmr_root,
        &p.checkpoint_vh,
        p.checkpoint_seq,
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
    /// version có `state_root` chỉ định (dùng để gán checkpoint_state_root).
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

    /// Dựng chain gồm genesis (seq0) + version-checkpoint (seq1) mang `state_root`=cp_root.
    /// Trả `(chain, checkpoint_seq)`.
    fn chain_with_checkpoint(a: &Author, cp_root: Hash32) -> (StrataChain, u64) {
        let pol = policy_with(a);
        let ph = pol.policy_hash();
        let v0 = signed_sr(0, [0u8; 32], 100, a, ph, [0x11; 32]);
        let mut chain = StrataChain::genesis([0xAB; 32], v0, &pol).unwrap();
        let v1 = signed_sr(1, chain.head_version_hash(), 200, a, ph, cp_root);
        chain.append_version(v1, &pol).unwrap();
        (chain, 1)
    }

    // ===== §8.3 tiêu chí test =====

    /// #1 `checkpoint_1000_versions`: 1000 entry → 1 checkpoint → 1 anchor (KHÔNG 1000).
    #[test]
    fn checkpoint_1000_versions_one_anchor() {
        let mut cp = Checkpoint::new();
        for i in 0..1000u64 {
            cp.append_entry(i, format!("measure-{i}").as_bytes());
        }
        assert_eq!(cp.len(), 1000, "gộp đủ 1000 entry vào 1 checkpoint");

        let a = mk_author(1);
        let (mut chain, _cp_seq) = chain_with_checkpoint(&a, cp.state_root());
        // Neo MỘT lần cho cả 1000 entry.
        let anchor = chain.publish_anchor().unwrap();
        // Chain chỉ có genesis + 1 version-checkpoint → 1 anchor gói trọn 1000 entry.
        assert_eq!(chain.len(), 2, "chỉ genesis + 1 checkpoint-version");
        assert_eq!(anchor.seq, 1, "một anchor duy nhất tại head=checkpoint");
        // Không-batch sẽ cần 1000 version = 1000 lần neo; batch = 1.
    }

    /// #2 `prove_entry_in_checkpoint`: prove entry bất kỳ; báo cáo kích thước proof.
    #[test]
    fn prove_entry_in_checkpoint_size() {
        let mut cp = Checkpoint::new();
        for i in 0..1000u64 {
            cp.append_entry(i, format!("m-{i}").as_bytes());
        }
        let (proof, size, leaf) = cp.prove_entry(637).unwrap();
        assert!(Checkpoint::verify_entry(
            cp.state_root(),
            &leaf,
            637,
            size,
            &proof
        ));
        let bytes = proof_hash_bytes(&proof);
        // log2(1000)≈10 → ~10 sibling + vài peak; đặt trần rộng, in số thật.
        println!(
            "[S3 #2] sub_proof(N=1000, idx=637) = {bytes}B ({} siblings + {} peaks)",
            proof.siblings.len(),
            proof.peaks.len()
        );
        assert!(
            bytes < 1024,
            "proof tầng dưới phải O(log N), thực tế {bytes}B"
        );
        // sai leaf → verify fail (fail-closed).
        let bad = entry_leaf(637, b"khac");
        assert!(!Checkpoint::verify_entry(
            cp.state_root(),
            &bad,
            637,
            size,
            &proof
        ));
    }

    /// #3 `two_tier_inclusion`: ghép sub-proof + version-proof → verify về mmr_root neo.
    #[test]
    fn two_tier_inclusion_verifies() {
        let mut cp = Checkpoint::new();
        for i in 0..500u64 {
            cp.append_entry(i, format!("e{i}").as_bytes());
        }
        let a = mk_author(2);
        let (chain, cp_seq) = chain_with_checkpoint(&a, cp.state_root());
        let anchored_root = chain.mmr_root();

        // tầng dưới.
        let idx = 321usize;
        let (sub_proof, sub_size, entry_leaf_v) = cp.prove_entry(idx).unwrap();
        // tầng trên.
        let (ver_proof, chain_size, checkpoint_vh) = chain.prove_version(cp_seq).unwrap();
        let proof = TwoTierProof {
            entry_idx: idx,
            entry_leaf: entry_leaf_v,
            sub_proof,
            sub_size,
            checkpoint_state_root: cp.state_root(),
            checkpoint_seq: cp_seq,
            checkpoint_vh,
            ver_proof,
            chain_size,
        };
        // verifier đọc state_root của version-checkpoint từ canonical chain.
        let version_state_root = chain.version(cp_seq).unwrap().state_root;
        assert!(verify_two_tier(anchored_root, version_state_root, &proof));

        // LINK sai (sub-MMR khác) → reject.
        let mut forged = proof.clone();
        forged.checkpoint_state_root = [0xFF; 32];
        assert!(!verify_two_tier(anchored_root, version_state_root, &forged));
        // entry_idx sai → reject.
        let mut wrong_idx = proof.clone();
        wrong_idx.entry_idx = 320;
        assert!(!verify_two_tier(
            anchored_root,
            version_state_root,
            &wrong_idx
        ));
    }

    /// #4 `close_on_max_entries`: vượt `max_entries` → đóng tại đúng trần.
    #[test]
    fn close_on_max_entries() {
        let pol = BatchPolicy::default(); // max=10_000
        // trong epoch, không idle → chỉ đóng khi đạt trần.
        assert_eq!(pol.should_close(9_999, 0, 100, 200), None);
        assert_eq!(
            pol.should_close(10_000, 0, 100, 200),
            Some(CloseReason::MaxEntries)
        );
        assert_eq!(
            pol.should_close(10_001, 0, 100, 200),
            Some(CloseReason::MaxEntries),
            "vượt trần vẫn đóng vì MaxEntries"
        );
    }

    /// #5 `close_on_idle`: im lặng ≥ `flush_on_idle` → đóng epoch sớm.
    #[test]
    fn close_on_idle() {
        let pol = BatchPolicy::default(); // idle=300, epoch=3600
        // 250s im lặng, còn trong epoch → chưa đóng.
        assert_eq!(pol.should_close(5, 0, 1000, 1250), None);
        // 300s im lặng → Idle.
        assert_eq!(pol.should_close(5, 0, 1000, 1300), Some(CloseReason::Idle));
        // count=0 → không đóng vì idle (không có gì để chốt).
        assert_eq!(
            pol.should_close(0, 0, 1000, 5000),
            Some(CloseReason::EpochElapsed)
        );
        // epoch hết trước idle vẫn đóng.
        assert_eq!(
            pol.should_close(5, 0, 3000, 3700),
            Some(CloseReason::EpochElapsed)
        );
    }

    /// #6 `entry_bytes_canonical`: khác payload (cùng độ dài) → leaf khác; length-prefix
    /// chống nhập nhằng ranh giới; khác entry_seq → leaf khác.
    #[test]
    fn entry_bytes_canonical() {
        // cùng độ dài, khác nội dung → khác leaf.
        assert_ne!(entry_leaf(0, b"aaaa"), entry_leaf(0, b"bbbb"));
        // khác seq, cùng payload → khác leaf (u64 prefix).
        assert_ne!(entry_leaf(0, b"x"), entry_leaf(1, b"x"));
        // length-prefix: (payload="ab","c") vs ("a","bc") KHÔNG thể va nhau vì payload
        // là 1 trường len-prefixed — minh hoạ hai payload khác nhau cho leaf khác.
        assert_ne!(entry_leaf(9, b"abc"), entry_leaf(9, b"ab"));
        // entry_bytes có đúng khung u64‖u32‖payload.
        let eb = entry_bytes(0x0102030405060708, b"hi");
        assert_eq!(&eb[0..8], &0x0102030405060708u64.to_be_bytes());
        assert_eq!(&eb[8..12], &2u32.to_be_bytes());
        assert_eq!(&eb[12..], b"hi");
    }

    /// #7 `crdt_deterministic_state_root` (CRDT §7.4): cùng TẬP op, thứ tự nhận khác →
    /// caller sort tất định trước khi append → cùng `checkpoint_state_root`.
    /// (Sub-MMR KHÔNG tự sort — tất định là hợp đồng của caller; test minh hoạ hợp đồng
    ///  bằng cách append theo entry_seq đã sort.)
    #[test]
    fn crdt_deterministic_state_root() {
        // "op" gắn entry_seq ổn định; hai thứ tự NHẬN khác nhau.
        let ops: Vec<(u64, &[u8])> = vec![(0, b"op-a"), (1, b"op-b"), (2, b"op-c")];
        let build = |order: &[usize]| {
            let mut cp = Checkpoint::new();
            // sort tất định theo entry_seq (hợp đồng caller) bất kể thứ tự nhận.
            let mut items: Vec<_> = order.iter().map(|&i| ops[i]).collect();
            items.sort_by_key(|(seq, _)| *seq);
            for (seq, payload) in items {
                cp.append_entry(seq, payload);
            }
            cp.state_root()
        };
        let r1 = build(&[0, 1, 2]);
        let r2 = build(&[2, 0, 1]); // nhận đảo thứ tự
        assert_eq!(
            r1, r2,
            "cùng tập op → cùng checkpoint_state_root (sau sort tất định)"
        );
    }

    /// Tất định tổng: cùng chuỗi entry → cùng state_root (không phụ thuộc phiên).
    #[test]
    fn deterministic_same_entries_same_root() {
        let build = || {
            let mut cp = Checkpoint::new();
            for i in 0..64u64 {
                cp.append_entry(i, format!("d{i}").as_bytes());
            }
            cp.state_root()
        };
        assert_eq!(build(), build());
    }
}
