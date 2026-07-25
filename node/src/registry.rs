//! Key-registry — phân giải `Did → VerifyingKey` (**CHỐT-5**: `Did` là *băm* DID
//! PhoenixKey, KHÔNG phải pubkey; không được giả định `Did == pubkey`).
//!
//! Đây là một trong ba việc mà §0 giao cho daemon (lưu / phân giải khoá / index dẫn xuất).
//! Bản trong repo là in-memory để chạy được đầu-cuối; bản thật cắm PhoenixKey resolver
//! (xem `OriLife-Core` / `Stamp M5 accreditation-resolver`) qua chính trait này.
//!
//! Không phân giải được ⇒ `StrataError::UnknownAuthor` ⇒ **424 Failed Dependency** (§3.1) —
//! fail-closed, KHÔNG rơi về "tin tạm rồi kiểm sau".

use ed25519_dalek::VerifyingKey;
use lampnet_strata::version::Did;
use std::collections::BTreeMap;
use std::sync::RwLock;

/// Cửa phân giải khoá của daemon.
pub trait KeyRegistry: Send + Sync + 'static {
    /// `None` = không biết Did này (KHÔNG phải "khoá sai") → `UnknownAuthor`.
    fn resolve(&self, did: &Did) -> Option<VerifyingKey>;
}

/// Registry in-memory (dev / test / node đơn lẻ).
#[derive(Debug, Default)]
pub struct InMemoryRegistry {
    map: RwLock<BTreeMap<Did, VerifyingKey>>,
}

impl InMemoryRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Đăng ký một ánh xạ `Did → pubkey`. Ghi đè nếu đã có.
    pub fn register(&self, did: Did, pk: VerifyingKey) {
        self.map
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(did, pk);
    }

    pub fn len(&self) -> usize {
        self.map.read().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl KeyRegistry for InMemoryRegistry {
    fn resolve(&self, did: &Did) -> Option<VerifyingKey> {
        self.map
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(did)
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;

    #[test]
    fn resolve_returns_none_for_unknown_did() {
        let r = InMemoryRegistry::new();
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        r.register([1u8; 32], sk.verifying_key());
        assert!(r.resolve(&[1u8; 32]).is_some());
        // Did khác — KHÔNG được suy ra khoá từ chính Did (CHỐT-5).
        assert!(r.resolve(&[2u8; 32]).is_none());
    }
}
