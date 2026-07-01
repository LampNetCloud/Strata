//! `AuditLog` — nhật ký truy cập append-only bất biến (Strata loại #2, INV-E3).
//!
//! Mỗi object nhạy cảm gắn một audit-log. Mỗi lần truy cập/ký sinh một [`AuditEntry`],
//! băm (domain-sep, RFC6962 leaf) thành leaf của MMR. Bất biến kế thừa INV-E3
//! (append-only): mở rộng không phá proof cũ; không sửa/xoá entry đã ghi.

use crate::chain::StrataError;
use crate::u32_be;
use crate::version::Hash32;
use lampnet_merkle_anchor::Blake3Hasher;
use lampnet_merkle_anchor::mmr::{InclusionProof, Mmr, verify as mmr_verify};

/// Loại sự kiện audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditAction {
    /// Tạo object.
    Create,
    /// Đọc/truy cập.
    Read,
    /// Ký.
    Sign,
    /// Chia sẻ proof chọn lọc.
    ShareProof,
    /// Cập nhật.
    Update,
}

impl AuditAction {
    fn code(self) -> u8 {
        match self {
            AuditAction::Create => 0,
            AuditAction::Read => 1,
            AuditAction::Sign => 2,
            AuditAction::ShareProof => 3,
            AuditAction::Update => 4,
        }
    }
}

/// Một mục audit append-only — leaf của audit-log MMR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    /// "Khi nào" — unix secs (đơn điệu không-giảm trong log).
    pub created_ts: u64,
    /// "Ai" — DID actor (đã băm; Did → pubkey qua key-registry, CHỐT-5).
    pub actor_did: [u8; 32],
    /// Loại sự kiện.
    pub action: AuditAction,
    /// "Ký cái gì" — version_hash HOẶC content_cid được truy cập/ký.
    pub signed_hash: Hash32,
    /// "Ở đâu" — commitment vị trí/ngữ cảnh (hash thuần, KHÔNG toạ độ thô).
    pub location: Hash32,
}

impl AuditEntry {
    /// Bytes canonical của entry (tất định; u64 BE, trường cố định nguyên byte).
    pub fn canonical(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(8 + 32 + 1 + 32 + 32);
        b.extend_from_slice(&self.created_ts.to_be_bytes());
        b.extend_from_slice(&self.actor_did);
        b.push(self.action.code());
        b.extend_from_slice(&self.signed_hash);
        b.extend_from_slice(&self.location);
        b
    }

    /// Leaf-hash của entry (MMR nền tự thêm RFC6962 leaf-prefix + tag).
    pub fn leaf_bytes(&self) -> Vec<u8> {
        // Bọc thêm length-prefix để an toàn nối chuỗi (defense-in-depth).
        let c = self.canonical();
        let mut out = Vec::with_capacity(4 + c.len());
        out.extend_from_slice(&u32_be(c.len()));
        out.extend_from_slice(&c);
        out
    }
}

/// Audit-log append-only dùng MMR nền (INV-E3). Không có hàm xoá/sửa.
///
/// `Mmr<H>` derive Debug/Clone yêu cầu `H: Debug+Clone` (do `PhantomData<H>`), mà
/// `Blake3Hasher` là unit struct không derive — nên ta hand-impl Debug/Clone qua
/// `entries` (đủ để debug; MMR tái dựng từ entries nếu cần).
pub struct AuditLog {
    mmr: Mmr<Blake3Hasher>,
    entries: Vec<AuditEntry>,
    /// `created_ts` của entry cuối — enforce đơn điệu không-giảm (M2).
    last_ts: u64,
}

impl std::fmt::Debug for AuditLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuditLog")
            .field("len", &self.entries.len())
            .field("root", &self.mmr.root())
            .finish()
    }
}

impl Clone for AuditLog {
    fn clone(&self) -> Self {
        let mut mmr = Mmr::<Blake3Hasher>::new();
        for e in &self.entries {
            mmr.append(&e.leaf_bytes());
        }
        Self {
            mmr,
            entries: self.entries.clone(),
            last_ts: self.last_ts,
        }
    }
}

impl Default for AuditLog {
    fn default() -> Self {
        Self::new()
    }
}

impl AuditLog {
    /// Log rỗng.
    pub fn new() -> Self {
        Self {
            mmr: Mmr::new(),
            entries: Vec::new(),
            last_ts: 0,
        }
    }

    /// Append một entry truy cập, ENFORCE `created_ts` đơn điệu không-giảm (M2).
    /// Trả về index (0-based) khi hợp lệ; `entry.created_ts < last_ts` (entry trước)
    /// → [`StrataError::TimestampRegress`]. Append-only — không sửa entry cũ.
    pub fn append_access(&mut self, entry: AuditEntry) -> Result<usize, StrataError> {
        if entry.created_ts < self.last_ts {
            return Err(StrataError::TimestampRegress {
                prev: self.last_ts,
                got: entry.created_ts,
            });
        }
        let idx = self.entries.len();
        self.last_ts = entry.created_ts;
        self.mmr.append(&entry.leaf_bytes());
        self.entries.push(entry);
        Ok(idx)
    }

    /// Số entry.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Rỗng?
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Root cam kết audit-log (để neo).
    pub fn root(&self) -> Hash32 {
        self.mmr.root()
    }

    /// Entry tại index.
    pub fn entry(&self, idx: usize) -> Option<&AuditEntry> {
        self.entries.get(idx)
    }

    /// Inclusion-proof "actor đã truy cập lúc T" — proof entry dưới root đã neo.
    pub fn prove(&self, idx: usize) -> Option<(InclusionProof, u64, Vec<u8>)> {
        if idx >= self.entries.len() {
            return None;
        }
        Some((
            self.mmr.prove(idx),
            self.mmr.len(),
            self.entries[idx].leaf_bytes(),
        ))
    }

    /// Verify một audit-proof dưới `root`.
    pub fn verify(
        root: Hash32,
        leaf_bytes: &[u8],
        idx: usize,
        size: u64,
        proof: &InclusionProof,
    ) -> bool {
        mmr_verify::<Blake3Hasher>(root, leaf_bytes, idx, size, proof)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ts: u64, actor: u8, action: AuditAction) -> AuditEntry {
        AuditEntry {
            created_ts: ts,
            actor_did: [actor; 32],
            action,
            signed_hash: [0x22; 32],
            location: [0x33; 32],
        }
    }

    #[test]
    fn append_and_prove() {
        let mut log = AuditLog::new();
        log.append_access(entry(100, 1, AuditAction::Create))
            .unwrap();
        log.append_access(entry(200, 2, AuditAction::Read)).unwrap();
        log.append_access(entry(300, 3, AuditAction::Sign)).unwrap();
        assert_eq!(log.len(), 3);

        let (proof, size, leaf) = log.prove(1).unwrap();
        assert!(AuditLog::verify(log.root(), &leaf, 1, size, &proof));
    }

    #[test]
    fn append_only_old_proof_survives_growth() {
        // INV-E3: proof entry cũ vẫn verify dưới root sau khi append thêm.
        let mut log = AuditLog::new();
        log.append_access(entry(100, 1, AuditAction::Create))
            .unwrap();
        let (proof0, size0, leaf0) = log.prove(0).unwrap();
        assert!(AuditLog::verify(log.root(), &leaf0, 0, size0, &proof0));

        log.append_access(entry(200, 2, AuditAction::Read)).unwrap();
        log.append_access(entry(300, 2, AuditAction::Read)).unwrap();
        let (proof0b, size0b, leaf0b) = log.prove(0).unwrap();
        assert!(AuditLog::verify(log.root(), &leaf0b, 0, size0b, &proof0b));
    }

    #[test]
    fn tampered_entry_fails_verify() {
        let mut log = AuditLog::new();
        log.append_access(entry(100, 1, AuditAction::Create))
            .unwrap();
        let (proof, size, _leaf) = log.prove(0).unwrap();
        // Verify với leaf KHÁC (entry bị đổi) → fail.
        let tampered = entry(100, 9, AuditAction::Create).leaf_bytes();
        assert!(!AuditLog::verify(log.root(), &tampered, 0, size, &proof));
    }

    #[test]
    fn root_changes_on_append() {
        let mut log = AuditLog::new();
        log.append_access(entry(100, 1, AuditAction::Create))
            .unwrap();
        let r0 = log.root();
        log.append_access(entry(200, 1, AuditAction::Read)).unwrap();
        assert_ne!(r0, log.root());
    }

    #[test]
    fn ts_equal_allowed() {
        // M2: ts bằng entry trước (không-giảm) → chấp nhận.
        let mut log = AuditLog::new();
        log.append_access(entry(100, 1, AuditAction::Create))
            .unwrap();
        assert!(log.append_access(entry(100, 2, AuditAction::Read)).is_ok());
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn ts_regress_rejected() {
        // M2: ts lùi (< entry trước) → TimestampRegress, log KHÔNG đổi.
        let mut log = AuditLog::new();
        log.append_access(entry(200, 1, AuditAction::Create))
            .unwrap();
        let root_before = log.root();
        let res = log.append_access(entry(100, 2, AuditAction::Read));
        assert!(matches!(
            res,
            Err(StrataError::TimestampRegress {
                prev: 200,
                got: 100
            })
        ));
        assert_eq!(log.len(), 1, "entry vi phạm KHÔNG được ghi");
        assert_eq!(log.root(), root_before, "root không đổi sau từ chối");
    }
}
