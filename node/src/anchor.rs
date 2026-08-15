//! Cắm `AnchorSink` (§4.1) vào daemon.
//!
//! Trait đã sống ở crate lõi (`src/anchor_sink.rs`) và có sẵn hai cài đặt thật:
//! `MosaicAnchorSink` (CIP-68 spend-recreate) và `SettlementSink` (metadata label 1234).
//! Daemon chỉ cần *chọn* một cái lúc khởi động — S5 KHÔNG viết backend mới.
//!
//! Hai sink phụ ở đây là để daemon chạy được khi chưa cấu hình chuỗi:
//! - [`DisabledSink`] — mặc định. Mọi neo thật trả `NotConfigured` (→ **501**), riêng
//!   `NoAnchor` vẫn `Ok(None)` vì "không neo" là một kết quả hợp lệ (§4.2 cadence).
//! - [`MemorySink`] — ghi nhớ trong RAM, cho test đầu-cuối HTTP mà không đụng mạng.

use lampnet_strata::version::Hash32;
use lampnet_strata::{
    AnchorBackend, AnchorError, AnchorPriority, AnchorReceipt, AnchorSink, StrataAnchor,
};
use std::collections::HashMap;
use std::sync::Mutex;

/// Tên backend trả ra HTTP (`"settlement"` / `"mosaic"` — §3 ví dụ dùng `"settlement"`).
pub fn backend_name(b: AnchorBackend) -> &'static str {
    match b {
        AnchorBackend::Settlement => "settlement",
        AnchorBackend::Mosaic => "mosaic",
    }
}

/// Không có backend nào được cấu hình.
#[derive(Debug, Default, Clone, Copy)]
pub struct DisabledSink;

impl AnchorSink for DisabledSink {
    fn publish(
        &self,
        _anchor: &StrataAnchor,
        priority: AnchorPriority,
    ) -> Result<Option<AnchorReceipt>, AnchorError> {
        match priority {
            AnchorPriority::NoAnchor => Ok(None),
            _ => Err(AnchorError::NotConfigured),
        }
    }

    fn resolve(&self, _ref_id: &Hash32) -> Result<Option<StrataAnchor>, AnchorError> {
        Err(AnchorError::NotConfigured)
    }
}

/// Sink in-memory: "neo" = ghi vào `HashMap`. Chỉ dùng cho test/dev.
#[derive(Debug, Default)]
pub struct MemorySink {
    anchored: Mutex<HashMap<Hash32, StrataAnchor>>,
    /// Đếm số lần đẩy — test dùng để chứng minh backend hỏng thì KHÔNG đẩy trùng.
    pushes: Mutex<u64>,
}

impl MemorySink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pushes(&self) -> u64 {
        *self.pushes.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl AnchorSink for MemorySink {
    fn publish(
        &self,
        anchor: &StrataAnchor,
        priority: AnchorPriority,
    ) -> Result<Option<AnchorReceipt>, AnchorError> {
        if matches!(priority, AnchorPriority::NoAnchor) {
            return Ok(None);
        }
        let mut g = self.anchored.lock().unwrap_or_else(|e| e.into_inner());
        // INV-E7 phía backend: neo lại seq ≤ đã thấy là rollback (§4.3 — backend metadata
        // không có validator nội tại nên indexer/off-chain phải tự từ chối).
        if let Some(prev) = g.get(&anchor.ref_id)
            && anchor.seq <= prev.seq
        {
            return Err(AnchorError::Rejected(format!(
                "rollback: đã neo seq={}, thử neo seq={}",
                prev.seq, anchor.seq
            )));
        }
        g.insert(anchor.ref_id, anchor.clone());
        let mut n = self.pushes.lock().unwrap_or_else(|e| e.into_inner());
        *n += 1;
        Ok(Some(AnchorReceipt {
            txid: format!("memory-{}-{}", hex::encode(&anchor.ref_id[..4]), anchor.seq),
            backend: AnchorBackend::Settlement,
            slot: None,
        }))
    }

    fn resolve(&self, ref_id: &Hash32) -> Result<Option<StrataAnchor>, AnchorError> {
        Ok(self
            .anchored
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(ref_id)
            .cloned())
    }

    /// Lô = một "tx" giả: mọi anchor được ghi, nhưng **đếm đúng MỘT lần đẩy**. Đếm
    /// N lần thì test đường lô không phân biệt được "một tx cho N anchor" với "N
    /// tx" — mà đó chính là tính chất đường này sinh ra để giữ.
    fn publish_many(
        &self,
        anchors: &[StrataAnchor],
        priority: AnchorPriority,
    ) -> Result<Option<AnchorReceipt>, AnchorError> {
        if matches!(priority, AnchorPriority::NoAnchor) || anchors.is_empty() {
            return Ok(None);
        }
        let mut g = self.anchored.lock().unwrap_or_else(|e| e.into_inner());
        // Kiểm TOÀN LÔ trước khi ghi bất cứ gì — một anchor rollback làm hỏng cả lô,
        // không để lại lô nửa-ghi (đúng ngữ nghĩa `publish_batch` của SettlementSink).
        for a in anchors {
            if let Some(prev) = g.get(&a.ref_id)
                && a.seq <= prev.seq
            {
                return Err(AnchorError::Rejected(format!(
                    "rollback: đã neo seq={}, thử neo seq={}",
                    prev.seq, a.seq
                )));
            }
        }
        for a in anchors {
            g.insert(a.ref_id, a.clone());
        }
        let mut n = self.pushes.lock().unwrap_or_else(|e| e.into_inner());
        *n += 1;
        Ok(Some(AnchorReceipt {
            txid: format!("memory-batch-{}-{}", anchors.len(), *n),
            backend: AnchorBackend::Settlement,
            slot: None,
        }))
    }
}

/// Sink luôn hỏng mạng — test đường "backend chết thì ref KHÔNG bị cháy".
#[derive(Debug, Default, Clone, Copy)]
pub struct FailingSink;

impl AnchorSink for FailingSink {
    fn publish(
        &self,
        _anchor: &StrataAnchor,
        _priority: AnchorPriority,
    ) -> Result<Option<AnchorReceipt>, AnchorError> {
        Err(AnchorError::Network("backend không phản hồi".into()))
    }

    fn resolve(&self, _ref_id: &Hash32) -> Result<Option<StrataAnchor>, AnchorError> {
        Err(AnchorError::Network("backend không phản hồi".into()))
    }
}
