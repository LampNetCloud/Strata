//! Nhật ký bền vững — biến `strata-node` từ *"mất sạch khi restart"* thành một daemon
//! dựng lại được chính mình.
//!
//! # Vì sao ghi REQUEST, không ghi TRẠNG THÁI
//!
//! Cách hiển nhiên là tuần tự hoá `ChainEntry` (versions + MMR + policy) rồi nạp lại lúc
//! khởi động. Cách đó **bỏ qua lõi**: mọi bất biến `INV-E1/E2/E4` + `verify_strict` chỉ
//! chạy ở đường ghi, nên một tệp bị sửa — hỏng đĩa, tay người, một bản vá sai — nạp vào
//! thành một `StrataChain` **chưa bao giờ qua cửa nào**. Sau đó nó phục vụ proof, và
//! những proof ấy verify đúng.
//!
//! Nhật ký này vì thế ghi **đúng cái client đã gửi và cửa đã nhận**, rồi replay bằng cách
//! **chạy lại chính hàm của đường ghi**. Hệ quả là một tính chất chứ không phải một lời
//! hứa: *nhật ký chỉ chứa được những lịch sử mà cửa sẽ nhận lần nữa.* Sửa một byte trong
//! đó thì hoặc chữ ký đỏ, hoặc hash-link đứt — daemon **không khởi động**, chứ không phục
//! vụ một lịch sử giả.
//!
//! Giá phải trả nói thẳng: replay là `O(n)` lượt `verify_strict`. Đó là giá của tính chất
//! trên, và nó đo được (`replay` in ra số record + thời gian).
//!
//! # `Did → pubkey` KHÔNG nằm trong nhật ký
//!
//! `Create` chỉ ghi **danh sách `Did`**, không ghi pubkey; replay phân giải khoá qua
//! **key-registry** như đường ghi thật (CHỐT-5). Ghi pubkey vào đây là dựng **nguồn sự
//! thật thứ hai** cho đúng thứ registry sinh ra để là nguồn duy nhất — và hai nguồn thì
//! lệch nhau vào ngày không ai nhìn.
//!
//! Hệ quả vận hành phải biết trước: **gỡ một khoá khỏi `STRATA_NODE_KEYS` rồi khởi động
//! lại ⇒ daemon từ chối lên**, kèm tên `ref_id` và `Did` không phân giải được. Đó là
//! fail-closed đúng chiều: phục vụ một lineage mà ta không còn xác minh được chủ của nó
//! thì tệ hơn là không phục vụ.
//!
//! # Thứ tự ghi — sau khi lõi nhận, và ghi hỏng thì ĐẦU ĐỘC daemon
//!
//! Ghi **trước** khi lõi kiểm thì nhật ký chứa cả request bị từ chối ⇒ replay đỏ. Nên ghi
//! **sau**. Nhưng khi ấy một lượt ghi đĩa hỏng để lại trạng thái RAM **đã tiến** quá
//! trạng thái bền vững, và mọi request sau đó xây tiếp lên một nền sẽ biến mất.
//!
//! ⇒ Ghi hỏng ⇒ [`Journal`] **tự đầu độc**: mọi lượt ghi sau trả lỗi, cửa trả `503`. Đọc
//! vẫn phục vụ (dữ liệu trong RAM vẫn đúng). *Một daemon nhận thêm việc sau khi mất khả
//! năng nhớ là một daemon nói dối về thứ nó đang hứa.*

use crate::dto::{AppendReq, AuditEventReq, CreateReq};
use lampnet_strata::version::Hash32;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Phiên bản định dạng. Dòng đầu tệp là header mang số này; lệch ⇒ **từ chối**, không
/// đoán. Một định dạng đọc nhầm còn tệ hơn một định dạng không đọc được.
pub const FORMAT_VERSION: u32 = 1;

/// Một bản ghi. `r` là `ref_id` hex32 — cùng dạng `AnchorResp.ref_id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum JournalRecord {
    /// Dòng đầu tệp.
    Header {
        format: u32,
    },
    Create {
        r: String,
        req: CreateReq,
    },
    Append {
        r: String,
        req: AppendReq,
    },
    Audit {
        r: String,
        req: AuditEventReq,
    },
    /// Neo đã **lên chuỗi**; ghi SAU khi backend trả biên nhận.
    ///
    /// `seq` không phải để nạp lại — replay tính ra nó bằng `publish_anchor()`. Nó ở đây
    /// để **đối chứng**: replay ra số khác ⇒ nhật ký không khớp lịch sử nó mô tả ⇒ từ chối.
    Anchor {
        r: String,
        seq: u64,
        txid: Option<String>,
        backend: Option<String>,
    },
}

/// Lỗi mức nhật ký.
#[derive(Debug)]
pub enum JournalError {
    Io(std::io::Error),
    /// Nhật ký đã bị đầu độc bởi một lượt ghi hỏng trước đó.
    Poisoned,
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JournalError::Io(e) => write!(f, "ghi nhật ký hỏng: {e}"),
            JournalError::Poisoned => write!(
                f,
                "nhật ký ĐÃ HỎNG ở một lượt ghi trước — daemon không còn nhớ được, mọi \
                 đường ghi đóng cho tới khi khởi động lại"
            ),
        }
    }
}

/// Nhật ký append-only trên đĩa.
#[derive(Debug)]
pub struct Journal {
    path: PathBuf,
    file: Mutex<File>,
    poisoned: AtomicBool,
}

impl Journal {
    /// Mở (tạo nếu chưa có) nhật ký tại `path`. Tệp mới ⇒ ghi header.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let path = path.as_ref().to_path_buf();
        let fresh = !path.exists();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&path)?;
        if fresh {
            let line = serde_json::to_string(&JournalRecord::Header {
                format: FORMAT_VERSION,
            })
            .expect("header luôn tuần tự hoá được");
            file.write_all(line.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_data()?;
        }
        Ok(Self {
            path,
            file: Mutex::new(file),
            poisoned: AtomicBool::new(false),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Ghi một bản ghi và **fsync**.
    ///
    /// `sync_data` không phải chi tiết thừa: thiếu nó thì "đã ghi" chỉ có nghĩa *"đã nằm
    /// trong cache của hệ điều hành"* — đúng thứ biến mất trong chính ca mà nhật ký sinh
    /// ra để sống sót.
    pub fn append(&self, rec: &JournalRecord) -> Result<(), JournalError> {
        self.append_many(std::slice::from_ref(rec))
    }

    /// Ghi nhiều bản ghi rồi **fsync một lần**. Dùng cho lô neo: N lượt fsync cho một tx
    /// đã lên chuỗi là trả giá cho một thứ đã tất định.
    pub fn append_many(&self, recs: &[JournalRecord]) -> Result<(), JournalError> {
        if self.poisoned.load(Ordering::SeqCst) {
            return Err(JournalError::Poisoned);
        }
        let mut buf = Vec::new();
        for rec in recs {
            let line = serde_json::to_string(rec).map_err(|e| {
                JournalError::Io(std::io::Error::other(format!("tuần tự hoá bản ghi: {e}")))
            })?;
            buf.extend_from_slice(line.as_bytes());
            buf.push(b'\n');
        }
        let mut f = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let r = f.write_all(&buf).and_then(|()| f.sync_data());
        if let Err(e) = r {
            // Đầu độc TRƯỚC khi trả lỗi: người gọi có thể nuốt lỗi, cờ này thì không.
            self.poisoned.store(true, Ordering::SeqCst);
            return Err(JournalError::Io(e));
        }
        Ok(())
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::SeqCst)
    }
}

/// Lỗi lúc replay — mọi biến thể đều là **từ chối khởi động**.
#[derive(Debug)]
pub enum ReplayError {
    Io(std::io::Error),
    /// Header vắng hoặc `format` lệch.
    BadHeader(String),
    /// Một dòng không phải JSON hợp lệ, hoặc không phải bản ghi ta biết.
    Corrupt {
        line_no: usize,
        why: String,
    },
    /// Bản ghi hợp lệ về cú pháp nhưng **lõi từ chối** khi chạy lại.
    Rejected {
        line_no: usize,
        why: String,
    },
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReplayError::Io(e) => write!(f, "đọc nhật ký hỏng: {e}"),
            ReplayError::BadHeader(m) => write!(f, "header nhật ký: {m}"),
            ReplayError::Corrupt { line_no, why } => {
                write!(f, "nhật ký hỏng ở dòng {line_no}: {why}")
            }
            ReplayError::Rejected { line_no, why } => write!(
                f,
                "dòng {line_no} bị LÕI TỪ CHỐI khi chạy lại: {why} — nhật ký mô tả một \
                 lịch sử mà cửa sẽ không nhận, nên nó không phải lịch sử của daemon này"
            ),
        }
    }
}

/// Đọc mọi dòng của nhật ký thành bản ghi.
///
/// # Đuôi rách — bỏ ĐÚNG dòng cuối, và chỉ dòng cuối
///
/// Tiến trình chết giữa một lượt `write_all` để lại một dòng **không có `\n` kết thúc**.
/// Dòng đó là một lượt ghi chưa hoàn tất ⇒ thao tác đó **chưa từng thành công** với client
/// (cửa chỉ trả 200 sau khi `append` trả `Ok`), nên bỏ nó là khôi phục đúng sự thật.
///
/// Ngược lại, một dòng thiếu `\n` ở **giữa** tệp là hỏng thật (đĩa lỗi / sửa tay) — nhưng
/// nó bất khả với tệp append-only, nên chỉ có ca đuôi. Điều kiện đặt theo **byte cuối
/// tệp**, không theo "dòng cuối parse có được không": một dòng rách vẫn có thể tình cờ
/// parse được, và khi ấy phép thử theo nội dung sẽ nhận vào một bản ghi cụt.
pub fn read_records(path: impl AsRef<Path>) -> Result<Vec<JournalRecord>, ReplayError> {
    let f = File::open(path.as_ref()).map_err(ReplayError::Io)?;
    let mut lines: Vec<String> = Vec::new();
    let mut ends_with_newline = true;
    for line in BufReader::new(f).lines() {
        let line = line.map_err(ReplayError::Io)?;
        lines.push(line);
    }
    // `BufReader::lines` nuốt mất thông tin "byte cuối có phải `\n` không" ⇒ hỏi lại tệp.
    {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = File::open(path.as_ref()).map_err(ReplayError::Io)?;
        let len = f.metadata().map_err(ReplayError::Io)?.len();
        if len > 0 {
            f.seek(SeekFrom::End(-1)).map_err(ReplayError::Io)?;
            let mut b = [0u8; 1];
            f.read_exact(&mut b).map_err(ReplayError::Io)?;
            ends_with_newline = b[0] == b'\n';
        }
    }
    if !ends_with_newline {
        lines.pop(); // đuôi rách — lượt ghi chưa hoàn tất, chưa từng trả 200
    }

    let mut out = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let line_no = i + 1;
        if line.trim().is_empty() {
            continue;
        }
        let rec: JournalRecord = serde_json::from_str(line).map_err(|e| ReplayError::Corrupt {
            line_no,
            why: e.to_string(),
        })?;
        out.push(rec);
    }

    match out.first() {
        Some(JournalRecord::Header { format }) if *format == FORMAT_VERSION => {}
        Some(JournalRecord::Header { format }) => {
            return Err(ReplayError::BadHeader(format!(
                "định dạng {format}, daemon này biết {FORMAT_VERSION}"
            )));
        }
        Some(_) => {
            return Err(ReplayError::BadHeader(
                "dòng đầu không phải header".to_string(),
            ));
        }
        None => {
            return Err(ReplayError::BadHeader(
                "tệp rỗng — không có cả header".to_string(),
            ));
        }
    }
    Ok(out)
}

/// `ref_id` hex32 của một bản ghi (header không có).
pub fn record_ref(rec: &JournalRecord) -> Option<&str> {
    match rec {
        JournalRecord::Header { .. } => None,
        JournalRecord::Create { r, .. }
        | JournalRecord::Append { r, .. }
        | JournalRecord::Audit { r, .. }
        | JournalRecord::Anchor { r, .. } => Some(r),
    }
}

/// Hex32 của `ref_id` — dạng dùng trong nhật ký.
pub fn ref_hex(r: &Hash32) -> String {
    hex::encode(r)
}
