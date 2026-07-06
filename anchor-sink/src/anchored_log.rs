//! `AnchoredLog` — bảng daemon-state `(ref_id, seq) → (mmr_root, mmr_size)` tại MỖI
//! lần neo (§8.1c GAP đã chốt: "daemon giữ bảng anchored: Vec<(seq, mmr_root,
//! mmr_size)>"). Cần để verify ngược dưới root ĐÃ NEO: proof dưới root CŨ cần size
//! CŨ (INV-E3 chỉ bảo đảm chiều xuôi proof-cũ-dưới-root-mới).
//!
//! Đây là state DAEMON, KHÔNG nằm trong core thuần `lampnet-strata`.

use lampnet_strata::Hash32;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

/// Bảng anchored: `(ref_id, seq) → (mmr_root, mmr_size)`. Nhỏ: 1 dòng/lần neo.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnchoredLog {
    entries: BTreeMap<(Hash32, u64), (Hash32, u64)>,
}

impl AnchoredLog {
    /// Bảng rỗng.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ghi một lần neo: tại `seq`, chain có `mmr_root` với `mmr_size` lá.
    /// Append-only: ghi đè cùng (ref_id, seq) với giá trị KHÁC bị từ chối (false).
    pub fn record(&mut self, ref_id: Hash32, seq: u64, mmr_root: Hash32, mmr_size: u64) -> bool {
        match self.entries.get(&(ref_id, seq)) {
            Some(existing) => *existing == (mmr_root, mmr_size),
            None => {
                self.entries.insert((ref_id, seq), (mmr_root, mmr_size));
                true
            }
        }
    }

    /// `(mmr_root, mmr_size)` tại lần neo `seq` của `ref_id`.
    pub fn get(&self, ref_id: &Hash32, seq: u64) -> Option<(Hash32, u64)> {
        self.entries.get(&(*ref_id, seq)).copied()
    }

    /// Lần neo mới nhất (seq cao nhất) của `ref_id`.
    pub fn latest(&self, ref_id: &Hash32) -> Option<(u64, Hash32, u64)> {
        self.entries
            .range((*ref_id, 0)..=(*ref_id, u64::MAX))
            .next_back()
            .map(|((_, seq), (root, size))| (*seq, *root, *size))
    }

    /// Số dòng.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Rỗng?
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Serialize dạng dòng văn bản `refid_hex seq root_hex size` (dễ soi, không dep).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for ((ref_id, seq), (root, size)) in &self.entries {
            writeln!(
                out,
                "{} {} {} {}",
                hex::encode(ref_id),
                seq,
                hex::encode(root),
                size
            )
            .expect("Vec writer");
        }
        out
    }

    /// Parse từ [`AnchoredLog::to_bytes`]. Dòng hỏng → Err (fail-closed, không đoán).
    pub fn from_bytes(data: &[u8]) -> Result<Self, String> {
        let text = std::str::from_utf8(data).map_err(|e| e.to_string())?;
        let mut log = Self::new();
        for (i, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() != 4 {
                return Err(format!("dòng {}: cần 4 cột", i + 1));
            }
            let ref_id: Hash32 = hex::decode(parts[0])
                .map_err(|e| format!("dòng {}: ref_id hex: {e}", i + 1))?
                .try_into()
                .map_err(|_| format!("dòng {}: ref_id phải 32B", i + 1))?;
            let seq: u64 = parts[1].parse().map_err(|e| format!("dòng {}: seq: {e}", i + 1))?;
            let root: Hash32 = hex::decode(parts[2])
                .map_err(|e| format!("dòng {}: root hex: {e}", i + 1))?
                .try_into()
                .map_err(|_| format!("dòng {}: root phải 32B", i + 1))?;
            let size: u64 = parts[3].parse().map_err(|e| format!("dòng {}: size: {e}", i + 1))?;
            if !log.record(ref_id, seq, root, size) {
                return Err(format!("dòng {}: (ref_id, seq) trùng với giá trị khác", i + 1));
            }
        }
        Ok(log)
    }

    /// Lưu ra file (atomic-ish: ghi file tạm rồi rename).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.to_bytes())?;
        std::fs::rename(&tmp, path)
    }

    /// Nạp từ file.
    pub fn load(path: &Path) -> std::io::Result<Self> {
        let data = std::fs::read(path)?;
        Self::from_bytes(&data).map_err(std::io::Error::other)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_get_latest() {
        let mut log = AnchoredLog::new();
        assert!(log.record([1; 32], 0, [0xA0; 32], 1));
        assert!(log.record([1; 32], 5, [0xA5; 32], 6));
        assert!(log.record([2; 32], 3, [0xB3; 32], 4));
        assert_eq!(log.get(&[1; 32], 5), Some(([0xA5; 32], 6)));
        assert_eq!(log.latest(&[1; 32]), Some((5, [0xA5; 32], 6)));
        assert_eq!(log.latest(&[2; 32]), Some((3, [0xB3; 32], 4)));
        assert_eq!(log.latest(&[9; 32]), None);
    }

    #[test]
    fn append_only_no_silent_overwrite() {
        let mut log = AnchoredLog::new();
        assert!(log.record([1; 32], 0, [0xA0; 32], 1));
        // Ghi lại cùng giá trị → ok (idempotent).
        assert!(log.record([1; 32], 0, [0xA0; 32], 1));
        // Ghi đè giá trị KHÁC → từ chối.
        assert!(!log.record([1; 32], 0, [0xFF; 32], 1));
        assert_eq!(log.get(&[1; 32], 0), Some(([0xA0; 32], 1)));
    }

    #[test]
    fn serialize_round_trip() {
        let mut log = AnchoredLog::new();
        log.record([1; 32], 0, [0xA0; 32], 1);
        log.record([1; 32], 7, [0xA7; 32], 8);
        let bytes = log.to_bytes();
        assert_eq!(AnchoredLog::from_bytes(&bytes).unwrap(), log);
    }

    #[test]
    fn corrupt_line_rejected() {
        assert!(AnchoredLog::from_bytes(b"khong hop le").is_err());
    }
}
