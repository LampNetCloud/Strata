//! `StrataVersion` — một nút phiên bản + canonical encoding + chữ ký (CHỐT-1, §1.7).
//!
//! - `version_hash = H_dom("LN/STRATA/ver/v1", canonical_core)` — KHÔNG gồm `sig` (CHỐT-1).
//! - `sig = Ed25519_sign(sk, version_hash)`, verify low-S canonical bằng `verify_strict`.
//! - Canonical: thứ tự trường tất định, u64 big-endian, length-prefix u32_be cho
//!   trường biến độ dài (`content_cid`); trường cố định ghi nguyên byte (§1.7).

use crate::u32_be;
use ed25519_dalek::{Signature, SigningKey, VerifyingKey};
use lampnet_merkle_anchor::hash::h_dom;

/// BLAKE3 32 byte (`H32`).
pub type Hash32 = [u8; 32];
/// DID đã băm (`Did`) — KHÔNG phải pubkey (CHỐT-5).
pub type Did = [u8; 32];
/// Chữ ký Ed25519.
pub type Sig = [u8; 64];

/// Tag băm version core (CHỐT-2).
pub const TAG_VER: &str = "LN/STRATA/ver/v1";

/// Một phiên bản. Thứ tự trường canonical đúng `_CONTRACT.md`:
/// `{ seq, prev_hash, content_cid, state_root, author_did, policy_hash, ts, sig }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrataVersion {
    /// 0 cho genesis, +1 mỗi lần (INV-E2).
    pub seq: u64,
    /// version_hash(seq-1); 0^32 nếu seq==0 (INV-E1).
    pub prev_hash: Hash32,
    /// CID nội dung off-chain (Mirage) — hash thuần (INV-E5). Biến độ dài.
    pub content_cid: Vec<u8>,
    /// Merkle root field-level (cho FieldProof — INV-E6).
    pub state_root: Hash32,
    /// DID người sửa (đã băm — CHỐT-5).
    pub author_did: Did,
    /// Hash policy quyền sửa (INV-E4).
    pub policy_hash: Hash32,
    /// Unix secs, đơn điệu không-giảm theo seq.
    pub ts: u64,
    /// Ed25519 (canonical low-S) của author trên `version_hash` (CHỐT-1).
    pub sig: Sig,
}

impl StrataVersion {
    /// Dựng version chưa ký (sig = 0^64). Caller gọi [`StrataVersion::sign`] sau.
    #[allow(clippy::too_many_arguments)]
    pub fn unsigned(
        seq: u64,
        prev_hash: Hash32,
        content_cid: Vec<u8>,
        state_root: Hash32,
        author_did: Did,
        policy_hash: Hash32,
        ts: u64,
    ) -> Self {
        Self {
            seq,
            prev_hash,
            content_cid,
            state_root,
            author_did,
            policy_hash,
            ts,
            sig: [0u8; 64],
        }
    }

    /// Encode tất định MỌI trường TRỪ `sig` (§1.7). u64 big-endian; length-prefix
    /// u32_be cho `content_cid`; trường cố định ghi nguyên 32 byte.
    pub fn canonical_core(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(8 + 32 + 4 + self.content_cid.len() + 32 + 32 + 32 + 8);
        b.extend_from_slice(&self.seq.to_be_bytes()); // u64 BE
        b.extend_from_slice(&self.prev_hash); // 32B
        b.extend_from_slice(&u32_be(self.content_cid.len())); // len-prefix u32 BE
        b.extend_from_slice(&self.content_cid);
        b.extend_from_slice(&self.state_root); // 32B
        b.extend_from_slice(&self.author_did); // 32B
        b.extend_from_slice(&self.policy_hash); // 32B
        b.extend_from_slice(&self.ts.to_be_bytes()); // u64 BE
        b // KHÔNG sig (CHỐT-1)
    }

    /// `version_hash = H_dom(TAG_VER, canonical_core)` — KHÔNG gồm sig (CHỐT-1).
    pub fn version_hash(&self) -> Hash32 {
        h_dom(TAG_VER, &self.canonical_core())
    }

    /// Ký version: `sig = Ed25519_sign(sk, version_hash)`. Set field `sig` tại chỗ.
    /// Ký TRỰC TIẾP trên 32 byte `version_hash` như message thuần (PureEdDSA), khớp
    /// CHỐT-1 (sig over version_hash). `author_did` lưu trong version; mapping
    /// `Did → pubkey` thuộc key-registry (CHỐT-5), verify ở [`StrataVersion::verify_sig`].
    pub fn sign(&mut self, sk: &SigningKey) {
        use ed25519_dalek::Signer;
        let vh = self.version_hash();
        let sig: Signature = sk.sign(&vh);
        self.sig = sig.to_bytes();
    }

    /// Verify chữ ký bằng `verify_strict` (chặn malleability — yêu cầu low-S canonical
    /// + reject các điểm thuộc nhóm con nhỏ). `pk` lấy từ key-registry theo `author_did`.
    pub fn verify_sig(&self, pk: &VerifyingKey) -> bool {
        let vh = self.version_hash();
        let sig = Signature::from_bytes(&self.sig);
        pk.verify_strict(&vh, &sig).is_ok()
    }
}

/// Lỗi giải mã canonical (`parse_canonical_core`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalError {
    /// Buffer ngắn hơn số byte cần đọc cho trường tiếp theo.
    Truncated {
        /// Tên trường đang đọc.
        field: &'static str,
        /// Số byte cần.
        need: usize,
        /// Số byte còn lại.
        have: usize,
    },
    /// `len(content_cid)` khai báo vượt phần byte còn lại.
    LengthOverflow {
        /// Độ dài khai báo trong prefix.
        declared: usize,
        /// Số byte thực còn lại.
        remaining: usize,
    },
    /// Còn byte thừa sau khi đọc hết các trường core.
    TrailingBytes {
        /// Số byte thừa ở đuôi.
        extra: usize,
    },
}

impl std::fmt::Display for CanonicalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated { field, need, have } => {
                write!(
                    f,
                    "canonical cụt tại '{field}': cần {need} byte, còn {have}"
                )
            }
            Self::LengthOverflow {
                declared,
                remaining,
            } => write!(
                f,
                "canonical len-prefix khai {declared} byte nhưng chỉ còn {remaining}"
            ),
            Self::TrailingBytes { extra } => {
                write!(f, "canonical thừa {extra} byte ở đuôi")
            }
        }
    }
}

impl std::error::Error for CanonicalError {}

/// Giải mã ngược [`StrataVersion::canonical_core`] — nghịch đảo của encoder (§1.7).
///
/// Trả version với `sig = 0^64`: `sig` **không nằm trong** canonical (CHỐT-1) nên
/// không khôi phục được từ byte — đúng phát biểu P4 (§9.3) "cùng `StrataVersion`
/// **trừ sig**". `version_hash` của kết quả trùng với bản gốc.
///
/// **Chặt có chủ đích.** Từ chối byte cụt, `len(content_cid)` khai vượt buffer, và
/// **byte thừa ở đuôi**. Decoder lỏng ở bất kỳ điểm nào trong ba điểm đó sẽ cho hai
/// chuỗi byte khác nhau cùng giải ra một version ⇒ mở lại đúng lớp nhập nhằng mà
/// canonical §1.7 sinh ra để đóng (cùng họ với trần `< 2³²` ở [`crate::u32_be`],
/// issue #18).
pub fn parse_canonical_core(bytes: &[u8]) -> Result<StrataVersion, CanonicalError> {
    struct Cursor<'a> {
        b: &'a [u8],
        i: usize,
    }
    impl<'a> Cursor<'a> {
        fn take(&mut self, n: usize, field: &'static str) -> Result<&'a [u8], CanonicalError> {
            let have = self.b.len() - self.i;
            if have < n {
                return Err(CanonicalError::Truncated {
                    field,
                    need: n,
                    have,
                });
            }
            let out = &self.b[self.i..self.i + n];
            self.i += n;
            Ok(out)
        }
        fn u64(&mut self, field: &'static str) -> Result<u64, CanonicalError> {
            let s = self.take(8, field)?;
            Ok(u64::from_be_bytes(s.try_into().expect("đúng 8 byte")))
        }
        fn h32(&mut self, field: &'static str) -> Result<Hash32, CanonicalError> {
            let s = self.take(32, field)?;
            Ok(s.try_into().expect("đúng 32 byte"))
        }
    }

    let mut c = Cursor { b: bytes, i: 0 };
    let seq = c.u64("seq")?;
    let prev_hash = c.h32("prev_hash")?;

    let len_bytes = c.take(4, "len(content_cid)")?;
    let declared = u32::from_be_bytes(len_bytes.try_into().expect("đúng 4 byte")) as usize;
    let remaining = bytes.len() - c.i;
    if declared > remaining {
        return Err(CanonicalError::LengthOverflow {
            declared,
            remaining,
        });
    }
    let content_cid = c.take(declared, "content_cid")?.to_vec();

    let state_root = c.h32("state_root")?;
    let author_did = c.h32("author_did")?;
    let policy_hash = c.h32("policy_hash")?;
    let ts = c.u64("ts")?;

    if c.i != bytes.len() {
        return Err(CanonicalError::TrailingBytes {
            extra: bytes.len() - c.i,
        });
    }

    Ok(StrataVersion::unsigned(
        seq,
        prev_hash,
        content_cid,
        state_root,
        author_did,
        policy_hash,
        ts,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::OsRng;

    fn mk_key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    #[test]
    fn canonical_core_excludes_sig() {
        // CHỐT-1: đổi sig KHÔNG đổi version_hash.
        let mut v = StrataVersion::unsigned(
            0,
            [0u8; 32],
            b"cid".to_vec(),
            [1u8; 32],
            [2u8; 32],
            [3u8; 32],
            100,
        );
        let vh1 = v.version_hash();
        v.sig = [0xAB; 64];
        let vh2 = v.version_hash();
        assert_eq!(vh1, vh2, "version_hash phải độc lập với sig (CHỐT-1)");
    }

    #[test]
    fn sign_then_verify_strict() {
        let sk = mk_key();
        let pk = sk.verifying_key();
        let mut v = StrataVersion::unsigned(
            0,
            [0u8; 32],
            b"x".to_vec(),
            [1u8; 32],
            [2u8; 32],
            [3u8; 32],
            1,
        );
        v.sign(&sk);
        assert!(v.verify_sig(&pk));
    }

    #[test]
    fn verify_strict_rejects_wrong_key() {
        // INV-E4: chữ ký bởi khóa khác → từ chối.
        let sk = mk_key();
        let other = mk_key();
        let mut v = StrataVersion::unsigned(
            0,
            [0u8; 32],
            b"x".to_vec(),
            [1u8; 32],
            [2u8; 32],
            [3u8; 32],
            1,
        );
        v.sign(&sk);
        assert!(!v.verify_sig(&other.verifying_key()));
    }

    #[test]
    fn verify_rejects_tampered_field() {
        // Đổi 1 trường core sau khi ký → version_hash đổi → verify fail.
        let sk = mk_key();
        let pk = sk.verifying_key();
        let mut v = StrataVersion::unsigned(
            0,
            [0u8; 32],
            b"x".to_vec(),
            [1u8; 32],
            [2u8; 32],
            [3u8; 32],
            1,
        );
        v.sign(&sk);
        v.ts = 999; // tamper
        assert!(!v.verify_sig(&pk));
    }

    /// Length-prefix của `content_cid`: giá trị đúng bằng độ dài, và decoder từ chối khi nó
    /// nói dối.
    ///
    /// **Đổi tên từ `canonical_length_prefix_avoids_concat_ambiguity` (2026-08-13).** Tên cũ
    /// hứa nhiều hơn thứ nó kiểm — bản cũ chỉ assert `cid="ab"` khác `cid="a"`, mà điều đó
    /// đúng hiển nhiên vì hai chuỗi khác **độ dài**, không dính gì tới nhập nhằng nối chuỗi.
    ///
    /// Và nhập nhằng nối chuỗi **không tồn tại** với layout này: `content_cid` là trường biến
    /// độ dài DUY NHẤT, kẹp giữa 40 byte đầu (`seq` 8 + `prev_hash` 32) và 104 byte đuôi
    /// (`state_root` + `author_did` + `policy_hash` + `ts`) đều cố định, nên phần giữa luôn
    /// khôi phục được từ tổng độ dài kể cả khi bỏ prefix. Phát hiện khi trả lời
    /// `OriLifeTrace/OriLife-Core#161` — chỗ đó từng nêu nhầm và đã đính chính trên issue.
    ///
    /// Prefix vẫn load-bearing, nhưng vì hai lẽ khác, và đó là thứ test này kiểm:
    /// (1) decoder cần nó để TỪ CHỐI input hỏng; (2) nó đi qua `u32_be`, tức mang van trần
    /// `< 2³²` fail-loud của §1.7 quy tắc 3 (issue #18).
    #[test]
    fn canonical_len_prefix_is_exact_and_lies_are_rejected() {
        const OFF: usize = 8 + 32; // ngay sau seq + prev_hash
        let v = StrataVersion::unsigned(
            0,
            [0u8; 32],
            b"hello".to_vec(),
            [0u8; 32],
            [0u8; 32],
            [0u8; 32],
            0,
        );
        let core = v.canonical_core();

        // (1) prefix phải ĐÚNG BẰNG len(cid) — không phải "có mặt là được".
        let declared = u32::from_be_bytes(core[OFF..OFF + 4].try_into().unwrap()) as usize;
        assert_eq!(declared, 5, "prefix phải khai đúng len(content_cid)");
        assert_eq!(core.len(), 148 + 5, "tổng = 148 + len(content_cid)");

        // (2) prefix khai THỪA → LengthOverflow; decoder không được cấp phát theo số khai.
        let mut over = core.clone();
        over[OFF..OFF + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(matches!(
            parse_canonical_core(&over),
            Err(CanonicalError::LengthOverflow { .. })
        ));

        // (3) prefix khai THIẾU → cid đọc hụt, phần dôi ra thành byte thừa ở đuôi.
        let mut under = core.clone();
        under[OFF..OFF + 4].copy_from_slice(&3u32.to_be_bytes());
        assert!(matches!(
            parse_canonical_core(&under),
            Err(CanonicalError::TrailingBytes { .. })
        ));

        // (4) bản nguyên vẹn vẫn round-trip.
        assert_eq!(parse_canonical_core(&core).unwrap().canonical_core(), core);
    }
}
