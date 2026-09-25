//! Đối chiếu chuỗi lúc khởi động (#119) — sau replay, trước khi nhận request ghi.
//!
//! Replay chỉ đọc nhật ký cục bộ và tin nó. Một node phục hồi từ bản chép cũ (hoặc bản chép có
//! đuôi rách mà ở máy gốc đã trả `200` và đã vào một lô neo) lên xanh với lịch sử **tụt sau**
//! chuỗi. Lượt neo sau đó chốt một `version_hash` khác cho một `seq` đã có trên L1 ⇒ một
//! `ref_id` hai nhánh. INV-E7 không cứu được ca này: nó so với gương cục bộ, và gương sai thì
//! phép gác vẫn chạy, vẫn trả `Ok`, và vẫn sai.
//!
//! # Định nghĩa "khớp" — dùng lại đúng gác của đường neo
//!
//! Một anchor trên chuỗi khớp lịch sử local ⟺ `AnchoredTable::record_anchor` dựng được dòng
//! neo cho `seq` đó từ chuỗi local **và** `verify_resolved` qua (cùng `head_version_hash` +
//! inclusion dưới `mmr_root` on-chain). Đó là cặp gác mà `verify_on_chain_against_local` dùng
//! lúc neo — một vị ngữ, một định nghĩa.
//!
//! # Gương tụt nhưng lịch sử khớp thì KHÔNG chặn
//!
//! Issue đề nghị chặn khi *"chuỗi đi trước gương"*. Có một ca hợp lệ mang đúng hình dạng đó:
//! daemon chết **sau** khi tx neo lên chuỗi và **trước** khi ghi bản ghi `Anchor` vào nhật ký.
//! Lúc đó gương tụt, nhưng mọi version mà anchor trên chuỗi cam kết đều có trong lịch sử local
//! và khớp byte. Lượt neo kế tiếp đi qua `verify_on_chain_against_local` như thường. Nên thứ bị
//! chặn là *lịch sử* không chứa hoặc không khớp anchor trên chuỗi — đúng chỗ các version nằm
//! giữa bị mất — chứ không phải con số gương. Ca gương tụt được đếm và in ra.
//!
//! # Không tự "đuổi" theo chuỗi
//!
//! Thứ tụt lại không chỉ là `seq`, mà là các version nằm giữa, và bản chép này không có chúng.
//! Đường về là khôi phục phần đuôi nhật ký từ bản mới hơn, không phải sửa gương.

use crate::store::{ChainStore, lock};
use lampnet_strata::version::Hash32;
use lampnet_strata::{AnchorError, AnchorSink, AnchoredTable, verify_resolved};

/// Một ref mà lịch sử local không chứa hoặc không khớp anchor trên chuỗi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    pub ref_id: Hash32,
    pub on_chain_seq: u64,
    pub local_head_seq: u64,
    /// Gương `anchored.seq` trong daemon (`None` = chưa ghi nhận lượt neo nào).
    pub mirror_seq: Option<u64>,
    pub why: String,
}

/// Kết quả khi mọi ref đều khớp.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChainCheckReport {
    /// Số ref đã hỏi chuỗi.
    pub checked: usize,
    /// Số ref có anchor trên chuỗi.
    pub anchored_on_chain: usize,
    /// Số ref có gương tụt sau chuỗi nhưng lịch sử khớp (ca chết giữa submit và ghi nhật ký).
    pub mirror_lagging: usize,
}

#[derive(Debug)]
pub enum ChainCheckError {
    /// Backend neo tắt: không hỏi được chuỗi — và cũng không neo được, nên không có lượt neo
    /// nào đè được. Bên gọi quyết định in gì; đây KHÔNG phải "đã đối chiếu xong".
    BackendDisabled,
    /// Không hỏi được chuỗi (mạng, thượng nguồn). Fail-closed: không biết thì không lên.
    Upstream(AnchorError),
    /// Có ref mà lịch sử local không chứa/không khớp anchor trên chuỗi.
    Diverged(Vec<Divergence>),
}

impl std::fmt::Display for ChainCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChainCheckError::BackendDisabled => write!(f, "backend neo tắt — không hỏi được chuỗi"),
            ChainCheckError::Upstream(e) => write!(f, "không hỏi được chuỗi: {e:?}"),
            ChainCheckError::Diverged(v) => {
                write!(
                    f,
                    "{} ref có lịch sử local không chứa hoặc không khớp anchor trên chuỗi — neo \
                     tiếp sẽ đẩy một version_hash khác vào một seq đã có trên L1. Khôi phục \
                     phần đuôi nhật ký từ bản mới hơn; KHÔNG sửa gương:",
                    v.len()
                )?;
                for d in v.iter().take(10) {
                    write!(
                        f,
                        "\n  ref {} · seq trên chuỗi {} · head local {} · gương {} · {}",
                        hex::encode(d.ref_id),
                        d.on_chain_seq,
                        d.local_head_seq,
                        d.mirror_seq
                            .map_or_else(|| "chưa có".to_string(), |s| s.to_string()),
                        d.why
                    )?;
                }
                if v.len() > 10 {
                    write!(f, "\n  … và {} ref nữa", v.len() - 10)?;
                }
                Ok(())
            }
        }
    }
}

/// Hỏi chuỗi cho mọi ref trong `store` rồi đối chiếu với lịch sử local.
///
/// Gọi ở ngữ cảnh **đồng bộ** (sink có thể dùng `reqwest::blocking`). Một lượt `resolve_many`
/// cho cả tập — `SettlementSink` quét địa chỉ một lần cho cả lô.
pub fn check_against_chain(
    store: &ChainStore,
    sink: &(dyn AnchorSink + Send + Sync),
) -> Result<ChainCheckReport, ChainCheckError> {
    let all = store.all();
    let mut report = ChainCheckReport {
        checked: all.len(),
        ..Default::default()
    };
    if all.is_empty() {
        return Ok(report);
    }
    let ids: Vec<Hash32> = all.iter().map(|(id, _)| *id).collect();
    let on_chain = match sink.resolve_many(&ids) {
        Ok(v) => v,
        Err(AnchorError::NotConfigured) => return Err(ChainCheckError::BackendDisabled),
        Err(e) => return Err(ChainCheckError::Upstream(e)),
    };

    let mut diverged = Vec::new();
    for a in &on_chain {
        let Some(entry) = store.get(&a.ref_id) else {
            continue; // sink trả ref ngoài tập đã hỏi — không thuộc node này
        };
        report.anchored_on_chain += 1;
        let g = lock(&entry);
        let local_head_seq = g.chain.head().seq;
        let mirror_seq = g.anchored.as_ref().map(|s| s.seq);
        let mut table = AnchoredTable::new();
        let why = match table.record_anchor(&g.chain, a) {
            Err(e) => Some(format!("lịch sử local không có seq đó ({e:?})")),
            Ok(()) => verify_resolved(&g.chain, a, &table)
                .err()
                .map(|e| format!("khác nội dung ({e:?})")),
        };
        match why {
            Some(why) => diverged.push(Divergence {
                ref_id: a.ref_id,
                on_chain_seq: a.seq,
                local_head_seq,
                mirror_seq,
                why,
            }),
            None if mirror_seq.is_none_or(|m| m < a.seq) => report.mirror_lagging += 1,
            None => {}
        }
    }
    if diverged.is_empty() {
        Ok(report)
    } else {
        diverged.sort_by_key(|d| d.ref_id);
        Err(ChainCheckError::Diverged(diverged))
    }
}
