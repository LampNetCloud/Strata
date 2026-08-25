//! `lampnet-strata-node` — **lớp daemon** của Strata (`Strata-API.md` §0).
//!
//! ```text
//! Platform (ProofChat/OriLife/AladinWork)
//!    │  HTTP JSON  (§3)          ← crate NÀY
//!    ▼
//! daemon: lưu · phân giải Did→pk · neo qua AnchorSink
//!    │  Rust API thuần (§2)
//!    ▼
//! lampnet-strata core (no I/O)   ← mọi hash/proof/invariant nằm ở đây
//! ```
//!
//! Crate này CHỈ làm ba việc §0 giao cho daemon: **lưu** ([`store`]), **phân giải khoá**
//! ([`registry`]), **đẩy neo** ([`anchor`]) — cộng lớp cửa HTTP ([`routes`] + [`dto`] +
//! [`error`]). Không có một dòng hash/merkle nào ở đây: nếu thấy mình sắp băm cái gì trong
//! crate này thì đó là dấu hiệu code đang lấn xuống core (§8.4).
//!
//! **Bền vững:** [`journal`] ghi lại mọi lượt ghi ĐÃ ĐƯỢC NHẬN, [`replay`] dựng lại daemon
//! bằng cách chạy lại chúng qua đúng đường ghi. Blob theo Mirage vẫn là milestone sau —
//! hôm nay `state_fields` nằm trong nhật ký cùng request đã ký.

pub mod anchor;
pub mod dto;
pub mod error;
pub mod hexs;
pub mod journal;
pub mod registry;
pub mod replay;
pub mod routes;
pub mod sink_config;
pub mod store;

pub use anchor::{DisabledSink, FailingSink, MemorySink};
pub use error::{ApiError, ApiResult};
pub use journal::{Journal, JournalError, JournalRecord, ReplayError, read_records};
pub use registry::{InMemoryRegistry, KeyRegistry};
pub use replay::{ReplayStats, replay_into};
pub use routes::{AppState, router};
pub use sink_config::{SinkChoice, build_sink};
pub use store::{ChainEntry, ChainStore, StoreError};
