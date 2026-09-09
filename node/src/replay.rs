//! Dựng lại daemon từ nhật ký — bằng cách **chạy lại đường ghi**, không nạp trạng thái.
//!
//! Mọi bản ghi đi qua đúng hàm mà cửa HTTP gọi (`create_inner` · `append_inner` ·
//! `audit_inner` · `publish_anchor`). Hệ quả: nhật ký **không thể** mang vào một lịch sử
//! mà cửa sẽ từ chối — sửa một byte thì chữ ký đỏ hoặc hash-link đứt, và daemon **không
//! khởi động**. Xem [`crate::journal`] cho lý do đầy đủ.
//!
//! Replay **không** đụng mạng: nó không gọi `AnchorSink`. Bản ghi `Anchor` chỉ chốt lại
//! `last_anchor_seq` + gương `anchored` — thứ vốn đã là *hệ quả* của một tx đã lên chuỗi
//! từ trước. Gọi lại sink ở đây là **neo lại toàn bộ lịch sử mỗi lần khởi động lại**.

use crate::dto::{AppendReq, AuditEventReq, CreateReq};
use crate::hexs;
use crate::journal::{JournalRecord, ReplayError};
use crate::registry::KeyRegistry;
use crate::routes::{append_inner, audit_inner, create_inner};
use crate::store::{AnchorState, ChainStore, StoreError, lock};
use lampnet_strata::version::Hash32;

/// Số đo của một lượt replay — in ra để **đo được** thay vì tin.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReplayStats {
    pub records: usize,
    pub refs: usize,
    pub versions: usize,
    pub audits: usize,
    pub anchors: usize,
}

fn parse_ref_hex(r: &str, line_no: usize) -> Result<Hash32, ReplayError> {
    hexs::decode_fixed::<32>(r).map_err(|e| ReplayError::Corrupt {
        line_no,
        why: format!("ref_id: {e}"),
    })
}

/// Chạy lại toàn bộ bản ghi vào `store`.
///
/// `store` phải **rỗng** khi vào; nó nhận nhật ký của chính mình sau đó.
pub fn replay_into(
    store: &ChainStore,
    registry: &dyn KeyRegistry,
    recs: &[JournalRecord],
) -> Result<ReplayStats, ReplayError> {
    let mut st = ReplayStats::default();
    for (i, rec) in recs.iter().enumerate() {
        let line_no = i + 1;
        let rejected = |e: crate::error::ApiError| ReplayError::Rejected {
            line_no,
            why: e.to_string(),
        };
        match rec {
            JournalRecord::Header { .. } => {}
            JournalRecord::Create { r, req } => {
                apply_create(store, registry, r, req, line_no, &rejected)?;
                st.refs += 1;
                st.versions += 1;
            }
            JournalRecord::Append { r, req } => {
                apply_append(store, r, req, line_no, &rejected)?;
                st.versions += 1;
            }
            JournalRecord::Audit { r, req } => {
                apply_audit(store, registry, r, req, line_no, &rejected)?;
                st.audits += 1;
            }
            JournalRecord::Anchor {
                r,
                seq,
                txid,
                backend,
            } => {
                apply_anchor(store, r, *seq, txid, backend, line_no, &rejected)?;
                st.anchors += 1;
            }
        }
        st.records += 1;
    }
    Ok(st)
}

fn apply_create(
    store: &ChainStore,
    registry: &dyn KeyRegistry,
    r: &str,
    req: &CreateReq,
    line_no: usize,
    rejected: &dyn Fn(crate::error::ApiError) -> ReplayError,
) -> Result<(), ReplayError> {
    let want = parse_ref_hex(r, line_no)?;
    let (ref_id, entry, _) = create_inner(registry, req).map_err(rejected)?;
    // `ref_id` là hàm của `(author_did, genesis_nonce)`. Lệch ⇒ bản ghi mô tả một ref khác
    // ref nó tự khai ⇒ nhật ký không nói về daemon này.
    if ref_id != want {
        return Err(ReplayError::Corrupt {
            line_no,
            why: format!(
                "ref_id khai {} nhưng dẫn ra {} từ (author_did, genesis_nonce)",
                hex::encode(want),
                hex::encode(ref_id)
            ),
        });
    }
    // `None` = KHÔNG ghi lại vào nhật ký: bản ghi này đến TỪ nhật ký. Ghi lại là nhân đôi
    // tệp mỗi lần khởi động, và lần khởi động thứ hai sẽ `RefExists` chính mình.
    store
        .insert_journaled(ref_id, entry, None)
        .map_err(|e| match e {
            StoreError::RefExists => ReplayError::Corrupt {
                line_no,
                why: format!("create lần hai cho ref {}", hex::encode(ref_id)),
            },
            StoreError::Journal(j) => ReplayError::Corrupt {
                line_no,
                why: j.to_string(),
            },
        })
}

fn apply_append(
    store: &ChainStore,
    r: &str,
    req: &AppendReq,
    line_no: usize,
    rejected: &dyn Fn(crate::error::ApiError) -> ReplayError,
) -> Result<(), ReplayError> {
    let ref_id = parse_ref_hex(r, line_no)?;
    let entry = store.get(&ref_id).ok_or_else(|| ReplayError::Corrupt {
        line_no,
        why: format!("append cho ref chưa create: {}", hex::encode(ref_id)),
    })?;
    let mut g = lock(&entry);
    append_inner(&mut g, req).map(|_| ()).map_err(rejected)
}

fn apply_audit(
    store: &ChainStore,
    registry: &dyn KeyRegistry,
    r: &str,
    req: &AuditEventReq,
    line_no: usize,
    rejected: &dyn Fn(crate::error::ApiError) -> ReplayError,
) -> Result<(), ReplayError> {
    let ref_id = parse_ref_hex(r, line_no)?;
    let entry = store.get(&ref_id).ok_or_else(|| ReplayError::Corrupt {
        line_no,
        why: format!("audit cho ref chưa create: {}", hex::encode(ref_id)),
    })?;
    let mut g = lock(&entry);
    audit_inner(registry, &mut g, req)
        .map(|_| ())
        .map_err(rejected)
}

fn apply_anchor(
    store: &ChainStore,
    r: &str,
    seq: u64,
    txid: &Option<String>,
    backend: &Option<String>,
    line_no: usize,
    rejected: &dyn Fn(crate::error::ApiError) -> ReplayError,
) -> Result<(), ReplayError> {
    let ref_id = parse_ref_hex(r, line_no)?;
    let entry = store.get(&ref_id).ok_or_else(|| ReplayError::Corrupt {
        line_no,
        why: format!("anchor cho ref chưa create: {}", hex::encode(ref_id)),
    })?;
    let mut g = lock(&entry);
    let committed = g
        .chain
        .publish_anchor()
        .map_err(|e| rejected(crate::error::ApiError::Core(e)))?;
    // ĐỐI CHỨNG, không phải nạp lại. `seq` trong bản ghi được tính lại từ lịch sử đã
    // replay; lệch nghĩa là chuỗi dựng lại KHÁC chuỗi lúc ghi — và khi ấy mọi proof
    // daemon sắp phục vụ đều nói về một lịch sử khác lịch sử đã neo lên chuỗi.
    if committed.seq != seq {
        return Err(ReplayError::Corrupt {
            line_no,
            why: format!(
                "anchor khai seq {seq} nhưng lịch sử dựng lại cho seq {}",
                committed.seq
            ),
        });
    }
    g.anchored = Some(AnchorState {
        seq: committed.seq,
        txid: txid.clone(),
        backend: backend.clone(),
    });
    Ok(())
}
