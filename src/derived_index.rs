//! S2 — `DerivedIndex` / columnar query + `FieldProof` xuyên version (Strata-API §5.4, §8.2).
//!
//! Trả lời "trường X tại version k = gì, có bằng chứng về `mmr_root` đã neo". Hai
//! nguyên tắc cứng của spec:
//! - **Không primitive mới:** chỉ GHÉP [`crate::state::prove_field`] (tầng trong,
//!   field ∈ `state_root`) với [`crate::chain::StrataChain::prove_version`] (tầng
//!   ngoài, version ∈ `mmr_root`). Verifier nối hai tầng qua `version.state_root`.
//! - **Cấm ghi vòng (INV cứng §7.5):** index là materialized view KHẢ BIẾN, UNTRUSTED;
//!   chỉ ĐỌC [`VersionLog`], KHÔNG bao giờ chạm `Mmr::append`. Trait cố tình không có
//!   `write_back`. Tin cậy đến TỪ proof, không từ index.
//!
//! `StrataVersion` on-chain chỉ giữ `state_root` (không giá trị thô — INV-E5/E6). Nên
//! daemon lưu kèm `state_fields` off-chain: [`InMemoryVersionLog`] giữ cả version đã ký
//! lẫn các trường, và ràng buộc `build_state_root(fields) == version.state_root` khi nạp
//! (off-chain phải khớp root đã ký — nếu không, từ chối).

use crate::chain::StrataChain;
use crate::state::{FieldProof, build_state_root, prove_field, verify_field_proof};
use crate::version::{Hash32, StrataVersion};
use lampnet_merkle_anchor::Blake3Hasher;
use lampnet_merkle_anchor::mmr::{InclusionProof, Mmr};
use std::collections::BTreeMap;

/// Vị trí một version trong log (== `StrataVersion.seq`).
pub type Seq = u64;

/// View CHỈ-ĐỌC của chuỗi version (daemon cấp từ `StrataChain` + kho `state_fields`).
/// SSoT vẫn là log + MMR; đây chỉ là giao diện để index/columnar đọc.
pub trait VersionLog {
    /// Số version trong log.
    fn len(&self) -> u64;
    /// Version canonical tại `seq` (đã ký) — `None` nếu vượt biên.
    fn version(&self, seq: Seq) -> Option<&StrataVersion>;
    /// `mmr_root` hiện tại (cam kết lịch sử).
    fn mmr_root(&self) -> Hash32;
    /// Giá trị trường tại một version (đọc từ `state_fields` lưu kèm off-chain).
    fn field_value_at(&self, seq: Seq, key: &[u8]) -> Option<Vec<u8>>;
    /// Liệt kê khoá trường có mặt ở `seq` — cần cho replay columnar tất định.
    fn field_keys_at(&self, seq: Seq) -> Vec<Vec<u8>>;

    /// Log rỗng? (mặc định từ `len`).
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Truy vấn đọc (không ghi). `SenderRange` = index nóng `(sender, ts)` (Math §13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    /// Mọi version có trường `key == value`.
    FieldEquals { key: Vec<u8>, value: Vec<u8> },
    /// Version mới nhất có trường `key` (giá trị head của trường).
    FieldLatest { key: Vec<u8> },
    /// Version của `sender` trong khoảng `[from_ts, to_ts]` (đơn điệu theo seq).
    SenderRange {
        sender: [u8; 32],
        from_ts: u64,
        to_ts: u64,
    },
}

/// Index dẫn xuất: dựng lại tất định từ log, trả `Vec<Seq>` (vị trí). Client tự lấy
/// [`CompositeFieldProof`] và verify — index KHÔNG được tin.
pub trait DerivedIndex: Sized {
    /// Dựng lại từ log. Tất định: cùng log → cùng index byte-chính-xác.
    fn replay<L: VersionLog>(log: &L) -> Self;
    /// Tra vị trí khớp query (chưa kèm proof).
    fn lookup(&self, q: &Query) -> Vec<Seq>;
}

/// Columnar engine derived: cột theo khoá trường + index nóng theo sender.
/// UNTRUSTED — chỉ tăng tốc; đúng-sai kiểm bằng proof, không bằng index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColumnarIndex {
    /// `key → [(value, seq)]` theo seq tăng dần.
    columns: BTreeMap<Vec<u8>, Vec<(Vec<u8>, Seq)>>,
    /// `sender → [(ts, seq)]` theo seq tăng dần (index nóng).
    by_sender: BTreeMap<[u8; 32], Vec<(u64, Seq)>>,
    /// Ảnh chụp `mmr_root` lúc replay — để phát hiện index cũ (không tin, chỉ tiện).
    mmr_root: Hash32,
}

impl ColumnarIndex {
    /// `mmr_root` mà index được dựng trên đó (so với `log.mmr_root()` để biết index cũ).
    pub fn indexed_root(&self) -> Hash32 {
        self.mmr_root
    }
}

impl DerivedIndex for ColumnarIndex {
    fn replay<L: VersionLog>(log: &L) -> Self {
        let mut columns: BTreeMap<Vec<u8>, Vec<(Vec<u8>, Seq)>> = BTreeMap::new();
        let mut by_sender: BTreeMap<[u8; 32], Vec<(u64, Seq)>> = BTreeMap::new();
        for seq in 0..log.len() {
            let Some(v) = log.version(seq) else { continue };
            by_sender
                .entry(v.author_did)
                .or_default()
                .push((v.ts, seq));
            // Duyệt khoá theo thứ tự tất định để index byte-chính-xác.
            let mut keys = log.field_keys_at(seq);
            keys.sort();
            keys.dedup();
            for key in keys {
                if let Some(val) = log.field_value_at(seq, &key) {
                    columns.entry(key).or_default().push((val, seq));
                }
            }
        }
        Self {
            columns,
            by_sender,
            mmr_root: log.mmr_root(),
        }
    }

    fn lookup(&self, q: &Query) -> Vec<Seq> {
        match q {
            Query::FieldEquals { key, value } => self
                .columns
                .get(key)
                .map(|col| {
                    col.iter()
                        .filter(|(v, _)| v == value)
                        .map(|(_, s)| *s)
                        .collect()
                })
                .unwrap_or_default(),
            Query::FieldLatest { key } => self
                .columns
                .get(key)
                .and_then(|col| col.last())
                .map(|(_, s)| vec![*s])
                .unwrap_or_default(),
            Query::SenderRange {
                sender,
                from_ts,
                to_ts,
            } => self
                .by_sender
                .get(sender)
                .map(|rows| {
                    rows.iter()
                        .filter(|(ts, _)| *ts >= *from_ts && *ts <= *to_ts)
                        .map(|(_, s)| *s)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// Full-scan tuyến tính trên log — ORACLE để đối chiếu index (test `oracle_vs_bruteforce`).
/// KHÔNG dùng index; chỉ đọc `VersionLog` trực tiếp.
pub fn brute_force<L: VersionLog>(log: &L, q: &Query) -> Vec<Seq> {
    let mut out = Vec::new();
    match q {
        Query::FieldEquals { key, value } => {
            for seq in 0..log.len() {
                if log.field_value_at(seq, key).as_deref() == Some(value.as_slice()) {
                    out.push(seq);
                }
            }
        }
        Query::FieldLatest { key } => {
            for seq in 0..log.len() {
                if log.field_value_at(seq, key).is_some() {
                    out.clear();
                    out.push(seq); // giữ seq lớn nhất có trường
                }
            }
        }
        Query::SenderRange {
            sender,
            from_ts,
            to_ts,
        } => {
            for seq in 0..log.len() {
                if let Some(v) = log.version(seq)
                    && v.author_did == *sender
                    && v.ts >= *from_ts
                    && v.ts <= *to_ts
                {
                    out.push(seq);
                }
            }
        }
    }
    out
}

/// Bằng chứng "trường X tại version k = value" ghép hai tầng (GAP cốt lõi S2, §8.2).
/// Mang FULL version core để verifier tự tính `version_hash` và bind `state_root`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompositeFieldProof {
    /// Vị trí version trong lịch sử.
    pub seq: Seq,
    /// Version core đã ký (verifier tính `version_hash` + đọc `state_root` từ đây).
    pub version: StrataVersion,
    /// Tầng trong: `(key, value) ∈ state_root(v_k)`.
    pub field_proof: FieldProof,
    /// Tầng ngoài: `version_hash(v_k) ∈ mmr_root`, tại vị trí `seq`.
    pub inclusion: InclusionProof,
    /// Số lá MMR tại thời điểm chứng minh (verify cần đúng size).
    pub mmr_size: u64,
}

/// Verify bằng chứng hai tầng dưới một `root` đã neo (tin cậy). Nối:
/// 1. `verify_field_proof` — (key,value) ∈ `field_proof.state_root`;
/// 2. `field_proof.state_root == version.state_root` (đã băm vào `version_hash`);
/// 3. `version.seq == seq` + `verify_version(root, version_hash, seq, size, inclusion)`.
///
/// Cả ba đúng ⇒ value của trường tại version k được cam kết dưới `root`. INV-E6 giữ:
/// mỗi tầng độc lập, không lộ trường khác.
pub fn verify_composite(root: Hash32, p: &CompositeFieldProof) -> bool {
    if !verify_field_proof(&p.field_proof) {
        return false;
    }
    if p.field_proof.state_root != p.version.state_root {
        return false;
    }
    if p.version.seq != p.seq {
        return false;
    }
    let vh = p.version.version_hash();
    StrataChain::verify_version(root, &vh, p.seq, p.mmr_size, &p.inclusion)
}

/// Log daemon in-memory: version đã ký + `state_fields` off-chain + MMR tái dựng.
/// Ràng buộc nạp: `build_state_root(fields) == version.state_root` (off-chain khớp root
/// đã ký) và `seq` tăng đúng 0,1,2,… Đây là view; SSoT vẫn là `StrataChain`.
#[derive(Debug, Default)]
pub struct InMemoryVersionLog {
    versions: Vec<StrataVersion>,
    fields: Vec<Vec<(Vec<u8>, Vec<u8>)>>,
    mmr: MmrHolder,
}

/// Wrapper giữ MMR (Blake3Hasher là unit struct không derive Debug/Default — tự cấp
/// Debug/Default để log dùng được). KHÔNG impl Clone: MMR không cho truy cập lá nội bộ
/// nên không tự tái dựng được; việc clone log do `InMemoryVersionLog::clone` tái dựng MMR
/// từ `versions`.
struct MmrHolder(Mmr<Blake3Hasher>);

impl Default for MmrHolder {
    fn default() -> Self {
        MmrHolder(Mmr::<Blake3Hasher>::new())
    }
}

impl std::fmt::Debug for MmrHolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MmrHolder").field("root", &self.0.root()).finish()
    }
}

/// Lỗi nạp log (view off-chain không khớp phần đã ký).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogError {
    /// seq nạp không phải `len` hiện tại (phải append đúng thứ tự).
    SeqOutOfOrder { expected: Seq, got: Seq },
    /// `build_state_root(fields)` không khớp `version.state_root` đã ký.
    StateRootMismatch { expected: Hash32, got: Hash32 },
}

impl InMemoryVersionLog {
    /// Log rỗng.
    pub fn new() -> Self {
        Self::default()
    }

    /// Nạp một version + các trường off-chain của nó. Kiểm khớp `state_root` + thứ tự seq.
    /// KHÔNG kiểm chữ ký/policy ở đây (đó là việc của `StrataChain` khi append vào SSoT);
    /// log chỉ chịu trách nhiệm off-chain-khớp-root.
    pub fn push(
        &mut self,
        version: StrataVersion,
        fields: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<(), LogError> {
        let expected_seq = self.versions.len() as u64;
        if version.seq != expected_seq {
            return Err(LogError::SeqOutOfOrder {
                expected: expected_seq,
                got: version.seq,
            });
        }
        let sr = build_state_root(&fields);
        if sr != version.state_root {
            return Err(LogError::StateRootMismatch {
                expected: version.state_root,
                got: sr,
            });
        }
        self.mmr.0.append(&version.version_hash());
        self.versions.push(version);
        self.fields.push(fields);
        Ok(())
    }

    /// Bằng chứng hai tầng cho trường `key` tại `seq`. `None` nếu seq/key vắng.
    ///
    /// TODO(tích hợp S1): dùng `mmr_size`/`root` HIỆN TẠI. Khi verify dưới `mmr_root` ĐÃ NEO
    /// cũ (size cũ từ bảng anchored §8.1c) cần biến thể `prove_composite_at(seq, key, mmr_size)`
    /// tái dựng inclusion tại size chỉ định. Bổ sung khi hợp nhất với AnchorSink (PR #6).
    pub fn prove_composite(&self, seq: Seq, key: &[u8]) -> Option<CompositeFieldProof> {
        let idx = seq as usize;
        let version = self.versions.get(idx)?.clone();
        let fields = self.fields.get(idx)?;
        let field_proof = prove_field(fields, key)?;
        let inclusion = self.mmr.0.prove(idx);
        Some(CompositeFieldProof {
            seq,
            version,
            field_proof,
            inclusion,
            mmr_size: self.mmr.0.len(),
        })
    }
}

impl Clone for InMemoryVersionLog {
    fn clone(&self) -> Self {
        let mut mmr = Mmr::<Blake3Hasher>::new();
        for v in &self.versions {
            mmr.append(&v.version_hash());
        }
        Self {
            versions: self.versions.clone(),
            fields: self.fields.clone(),
            mmr: MmrHolder(mmr),
        }
    }
}

impl VersionLog for InMemoryVersionLog {
    fn len(&self) -> u64 {
        self.versions.len() as u64
    }
    fn version(&self, seq: Seq) -> Option<&StrataVersion> {
        self.versions.get(seq as usize)
    }
    fn mmr_root(&self) -> Hash32 {
        self.mmr.0.root()
    }
    fn field_value_at(&self, seq: Seq, key: &[u8]) -> Option<Vec<u8>> {
        self.fields
            .get(seq as usize)?
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }
    fn field_keys_at(&self, seq: Seq) -> Vec<Vec<u8>> {
        self.fields
            .get(seq as usize)
            .map(|f| f.iter().map(|(k, _)| k.clone()).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::Policy;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

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
    fn signed(
        seq: u64,
        prev: Hash32,
        ts: u64,
        a: &Author,
        ph: Hash32,
        fields: &[(Vec<u8>, Vec<u8>)],
    ) -> StrataVersion {
        let sr = build_state_root(fields);
        let mut v = StrataVersion::unsigned(seq, prev, b"cid".to_vec(), sr, a.did, ph, ts);
        v.sign(&a.sk);
        v
    }

    // Log 3 version với trường "status" đổi giá trị + author khác nhau.
    fn build_log() -> (InMemoryVersionLog, Hash32) {
        let a = mk_author(1);
        let ph = policy_with(&a).policy_hash();
        let mut log = InMemoryVersionLog::new();
        let f0 = vec![
            (b"status".to_vec(), b"open".to_vec()),
            (b"owner".to_vec(), b"alice".to_vec()),
        ];
        let v0 = signed(0, [0u8; 32], 100, &a, ph, &f0);
        log.push(v0, f0).unwrap();
        let f1 = vec![
            (b"status".to_vec(), b"closed".to_vec()),
            (b"owner".to_vec(), b"alice".to_vec()),
        ];
        let v1 = signed(1, log.version(0).unwrap().version_hash(), 200, &a, ph, &f1);
        log.push(v1, f1).unwrap();
        let f2 = vec![
            (b"status".to_vec(), b"open".to_vec()),
            (b"owner".to_vec(), b"bob".to_vec()),
        ];
        let v2 = signed(2, log.version(1).unwrap().version_hash(), 300, &a, ph, &f2);
        log.push(v2, f2).unwrap();
        let root = log.mmr_root();
        (log, root)
    }

    #[test]
    fn s2_query_field_at_version() {
        // Query field ở version k → ghép proof → verify về mmr_root. PASS.
        let (log, root) = build_log();
        for seq in 0..log.len() {
            let p = log.prove_composite(seq, b"status").expect("có proof");
            assert!(verify_composite(root, &p), "verify status@{seq}");
        }
        // Giá trị đúng.
        assert_eq!(
            log.prove_composite(1, b"status").unwrap().field_proof.value,
            b"closed"
        );
    }

    #[test]
    fn s2_oracle_vs_bruteforce() {
        // lookup(index) KHỚP full-scan trên toàn log.
        let (log, _) = build_log();
        let idx = ColumnarIndex::replay(&log);
        let queries = [
            Query::FieldEquals {
                key: b"status".to_vec(),
                value: b"open".to_vec(),
            },
            Query::FieldEquals {
                key: b"status".to_vec(),
                value: b"closed".to_vec(),
            },
            Query::FieldEquals {
                key: b"owner".to_vec(),
                value: b"missing".to_vec(),
            },
            Query::FieldLatest {
                key: b"status".to_vec(),
            },
            Query::SenderRange {
                sender: [1u8; 32],
                from_ts: 150,
                to_ts: 300,
            },
        ];
        for q in &queries {
            let mut a = idx.lookup(q);
            let mut b = brute_force(&log, q);
            a.sort();
            b.sort();
            assert_eq!(a, b, "index vs brute-force lệch cho {q:?}");
        }
        // Sanity giá trị cụ thể.
        assert_eq!(
            idx.lookup(&Query::FieldEquals {
                key: b"status".to_vec(),
                value: b"open".to_vec()
            }),
            vec![0, 2]
        );
        assert_eq!(
            idx.lookup(&Query::FieldLatest {
                key: b"status".to_vec()
            }),
            vec![2]
        );
    }

    #[test]
    fn s2_index_replay_root_bit_exact() {
        // Xóa index → replay(log) → mmr_root khớp + index byte-chính-xác (chốt ghi-vòng).
        let (log, root) = build_log();
        let idx1 = ColumnarIndex::replay(&log);
        let idx2 = ColumnarIndex::replay(&log);
        assert_eq!(idx1, idx2, "replay phải tất định (byte-chính-xác)");
        assert_eq!(idx1.indexed_root(), root, "index root == log mmr_root");
        // Mỗi version: state_root dựng lại từ fields khớp state_root đã ký.
        for seq in 0..log.len() {
            let fields: Vec<(Vec<u8>, Vec<u8>)> = log
                .field_keys_at(seq)
                .into_iter()
                .map(|k| {
                    let val = log.field_value_at(seq, &k).unwrap();
                    (k, val)
                })
                .collect();
            assert_eq!(
                build_state_root(&fields),
                log.version(seq).unwrap().state_root,
                "state_root replay khớp bit @{seq}"
            );
        }
    }

    #[test]
    fn s2_field_privacy_preserved() {
        // Proof field X tại version k KHÔNG chứa key/value trường khác (xuyên version).
        let (log, _) = build_log();
        for seq in 0..log.len() {
            let p = log.prove_composite(seq, b"status").unwrap();
            let fields: Vec<(Vec<u8>, Vec<u8>)> = log
                .field_keys_at(seq)
                .into_iter()
                .map(|k| (k.clone(), log.field_value_at(seq, &k).unwrap()))
                .collect();
            for (k, val) in &fields {
                if k == b"status" {
                    continue;
                }
                for (sib, _) in &p.field_proof.siblings {
                    assert_ne!(sib.as_slice(), val.as_slice(), "lộ value trường khác");
                    assert_ne!(sib.as_slice(), k.as_slice(), "lộ key trường khác");
                }
            }
        }
    }

    #[test]
    fn composite_rejects_wrong_root_and_tamper() {
        // Chống: root sai, đổi value, đổi seq → verify fail.
        let (log, root) = build_log();
        let good = log.prove_composite(1, b"status").unwrap();
        assert!(verify_composite(root, &good));

        // root sai (root của version 0).
        let (log0, _) = {
            let a = mk_author(9);
            let ph = policy_with(&a).policy_hash();
            let mut l = InMemoryVersionLog::new();
            let f = vec![(b"x".to_vec(), b"y".to_vec())];
            let v = signed(0, [0u8; 32], 1, &a, ph, &f);
            l.push(v, f).unwrap();
            let r = l.mmr_root();
            (l, r)
        };
        assert!(!verify_composite(log0.mmr_root(), &good), "root lạ phải fail");

        // Đổi value trong field_proof → field-proof fail.
        let mut tv = good.clone();
        tv.field_proof.value = b"HACK".to_vec();
        assert!(!verify_composite(root, &tv));

        // Đổi seq claim → inclusion fail.
        let mut ts = good.clone();
        ts.seq = 0;
        assert!(!verify_composite(root, &ts));
    }

    #[test]
    #[ignore = "benchmark — chạy: cargo test --release s2_benchmark -- --ignored --nocapture"]
    fn s2_benchmark_query_scaling() {
        use std::time::Instant;
        // Không ký thật (push chỉ kiểm state_root) — đo thuần replay + lookup theo n.
        let did = [7u8; 32];
        eprintln!("\n n | build_index | lookup_equals | lookup_latest | lookup_sender (µs)");
        for &n in &[100u64, 1_000, 10_000, 100_000] {
            let mut log = InMemoryVersionLog::new();
            let mut prev = [0u8; 32];
            for seq in 0..n {
                // status xoay vòng open/closed để FieldEquals có kết quả rải đều.
                let status = if seq % 2 == 0 { b"open".to_vec() } else { b"closed".to_vec() };
                let fields = vec![
                    (b"status".to_vec(), status),
                    (b"n".to_vec(), seq.to_be_bytes().to_vec()),
                ];
                let sr = build_state_root(&fields);
                let v = StrataVersion::unsigned(seq, prev, b"cid".to_vec(), sr, did, [0u8; 32], seq);
                prev = v.version_hash();
                log.push(v, fields).unwrap();
            }
            let t0 = Instant::now();
            let idx = ColumnarIndex::replay(&log);
            let build = t0.elapsed().as_micros();

            let q_eq = Query::FieldEquals { key: b"status".to_vec(), value: b"open".to_vec() };
            let q_lt = Query::FieldLatest { key: b"status".to_vec() };
            let q_sr = Query::SenderRange { sender: did, from_ts: 0, to_ts: n };
            let t1 = Instant::now();
            let r_eq = idx.lookup(&q_eq).len();
            let eq = t1.elapsed().as_micros();
            let t2 = Instant::now();
            idx.lookup(&q_lt);
            let lt = t2.elapsed().as_micros();
            let t3 = Instant::now();
            let r_sr = idx.lookup(&q_sr).len();
            let sr_us = t3.elapsed().as_micros();

            assert_eq!(r_eq, (n as usize).div_ceil(2), "FieldEquals đếm đúng");
            assert_eq!(r_sr, n as usize, "SenderRange phủ hết");
            eprintln!("{n:>7} | {build:>11} | {eq:>13} | {lt:>13} | {sr_us:>13}");
        }
    }

    #[test]
    fn log_rejects_offchain_state_root_mismatch() {
        // Off-chain fields KHÔNG khớp state_root đã ký → push fail (view phải trung thực).
        let a = mk_author(1);
        let ph = policy_with(&a).policy_hash();
        let real = vec![(b"k".to_vec(), b"v".to_vec())];
        let v = signed(0, [0u8; 32], 1, &a, ph, &real);
        let mut log = InMemoryVersionLog::new();
        // Nạp kèm fields KHÁC (state_root sẽ lệch).
        let wrong = vec![(b"k".to_vec(), b"TAMPER".to_vec())];
        assert!(matches!(
            log.push(v, wrong),
            Err(LogError::StateRootMismatch { .. })
        ));
    }
}
