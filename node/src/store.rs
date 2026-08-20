//! Lưu trạng thái Strata phía daemon.
//!
//! Core THUẦN chỉ giữ `StrataChain` (version + MMR). Ba thứ daemon phải giữ thêm để phục
//! vụ §3 mà core cố tình không giữ:
//!
//! 1. **`policy`** đang thực thi — `append_version` nhận `&Policy` từ ngoài (INV-E4).
//! 2. **`fields` theo seq** — `state_root` là hash một chiều; muốn trả `prove_field` (§2.5)
//!    thì phải giữ được cặp `(key, value)` gốc. Bản thật: byte nằm ở Mirage, daemon giữ CID
//!    (§8.4) — ở đây giữ thẳng trong RAM để chạy được đầu-cuối.
//! 3. **`anchored`** — biên nhận neo gần nhất + **gương của `last_anchor_seq`**. Cần gương
//!    vì `StrataChain::publish_anchor()` *đã cập nhật* `last_anchor_seq` khi trả Ok: nếu gọi
//!    nó TRƯỚC khi đẩy on-chain mà backend hỏng thì ref bị "cháy" (thử lại → `AnchorRollback`
//!    vĩnh viễn). Daemon vì thế tự kiểm rollback bằng gương này TRƯỚC, đẩy on-chain, thành
//!    công mới `publish_anchor()` để chốt INV-E7 ở lõi — xem `routes::anchor`.
//!
//! Khoá: một `Mutex` **theo từng ref** (không phải khoá toàn cục) — vừa cho phép song song
//! giữa các ref, vừa cho `anchor` giữ trọn chuỗi *kiểm → đẩy → chốt* trong một vùng loại trừ.
//!
//! Bền vững (đĩa/Mirage) là milestone sau; struct này là chỗ cắm.

use crate::dto::StateFields;
use lampnet_strata::audit::AuditLog;
use lampnet_strata::chain::{Policy, StrataChain};
use lampnet_strata::version::Hash32;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

/// Biên nhận neo gần nhất của một ref.
#[derive(Debug, Clone)]
pub struct AnchorState {
    /// seq đã neo (gương `last_anchor_seq` của lõi).
    pub seq: u64,
    /// txid on-chain — `None` khi backend trả idempotent (đã neo từ trước).
    pub txid: Option<String>,
    /// Tên backend đã neo ("settlement" / "mosaic").
    pub backend: Option<String>,
}

/// Toàn bộ trạng thái daemon giữ cho MỘT Strata.
pub struct ChainEntry {
    pub chain: StrataChain,
    pub policy: Policy,
    /// `state_fields` theo seq (`fields[seq]`).
    pub fields: Vec<StateFields>,
    /// Audit-log loại #2 (§2.6 cách 2).
    pub audit: AuditLog,
    /// Lần neo gần nhất (None = chưa neo bao giờ).
    pub anchored: Option<AnchorState>,
}

impl ChainEntry {
    pub fn new(chain: StrataChain, policy: Policy, genesis_fields: StateFields) -> Self {
        Self {
            chain,
            policy,
            fields: vec![genesis_fields],
            audit: AuditLog::new(),
            anchored: None,
        }
    }

    /// `state_fields` của một seq (None nếu seq ngoài phạm vi).
    pub fn fields_at(&self, seq: u64) -> Option<&StateFields> {
        self.fields.get(seq as usize)
    }
}

/// Lỗi mức kho.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    /// `create` trên ref_id đã tồn tại — KHÔNG ghi đè lịch sử.
    RefExists,
}

/// Kho ref → trạng thái, khoá theo từng ref.
#[derive(Default)]
pub struct ChainStore {
    refs: RwLock<HashMap<Hash32, Arc<Mutex<ChainEntry>>>>,
}

impl ChainStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Thêm ref mới. `RefExists` nếu ref_id đã tồn tại (cùng `author_did`+`genesis_nonce`
    /// ⇒ cùng ref_id — `create` lần hai KHÔNG được ghi đè lịch sử).
    pub fn insert(&self, ref_id: Hash32, entry: ChainEntry) -> Result<(), StoreError> {
        let mut g = self.refs.write().unwrap_or_else(|e| e.into_inner());
        if g.contains_key(&ref_id) {
            return Err(StoreError::RefExists);
        }
        g.insert(ref_id, Arc::new(Mutex::new(entry)));
        Ok(())
    }

    /// Liệt kê **mọi** ref đang giữ, kèm handle khoá riêng của từng ref.
    ///
    /// Trả bản sao của `Vec` chứ không giữ khoá đọc: người gọi sẽ lần lượt khoá
    /// từng `ChainEntry` để đọc, và giữ khoá `refs` suốt quãng đó thì mọi `create`
    /// mới bị chặn theo — một route CHỈ ĐỌC không được làm đường ghi đứng lại.
    ///
    /// Ảnh chụp vì thế là **nhất quán theo từng ref, không nhất quán toàn cục**:
    /// ref mới tạo giữa chừng có thể vắng mặt. Đúng thứ người đọc cần — hàng đợi
    /// neo lấy lại ảnh mới ở lượt poll sau, và một ref đến muộn chỉ chậm một nhịp.
    pub fn all(&self) -> Vec<(Hash32, Arc<Mutex<ChainEntry>>)> {
        self.refs
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect()
    }

    /// Lấy handle có khoá riêng của ref.
    pub fn get(&self, ref_id: &Hash32) -> Option<Arc<Mutex<ChainEntry>>> {
        self.refs
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(ref_id)
            .cloned()
    }

    pub fn len(&self) -> usize {
        self.refs.read().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Khoá một entry, bỏ qua poison (một handler panic không được làm chết cả ref).
pub fn lock(entry: &Mutex<ChainEntry>) -> MutexGuard<'_, ChainEntry> {
    entry.lock().unwrap_or_else(|e| e.into_inner())
}
