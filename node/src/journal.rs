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

/// Khoá advisory ĐỘC QUYỀN, KHÔNG CHẶN, trên chính tệp nhật ký (issue #73).
///
/// Vì sao `Mutex<File>` không đủ: nó nối tiếp hoá các lượt ghi của **chính tiến trình
/// này**. Hai daemon cùng trỏ vào một tệp thì mỗi bên giữ một `Mutex` riêng và không bên
/// nào biết bên kia tồn tại — hai luồng `append` xen kẽ nhau, còn mỗi dòng vẫn là JSON
/// hợp lệ. Replay sau đó thấy một lịch sử **phân nhánh**: hai `Create` cho cùng một ref,
/// hoặc hai `Append` cùng `seq`. Không có gì kêu lên ở lúc ghi, và đúng thứ nhật ký sinh
/// ra để bảo vệ — *"chỉ chứa được lịch sử mà cửa sẽ nhận lần nữa"* — mất hiệu lực.
///
/// KHÔNG chặn (`LOCK_NB`): daemon thứ hai phải **chết ngay và nói vì sao**, không được
/// treo im chờ một khoá có thể không bao giờ nhả. Một tiến trình treo lúc khởi động trông
/// giống hệt một tiến trình đang nạp nhật ký lớn.
///
/// **CỐ Ý CHƯA CÓ CỜ BỎ QUA KHOÁ** (kiểu `FORCE`/`--no-lock`). Có ca vận hành thật cần nó
/// (tệp còn khoá thừa sau một lần máy chết cứng), nhưng một cờ mở khoá là cờ sẽ bị dán vào
/// script khởi động rồi ở đó vĩnh viễn, và lúc ấy lá chắn này thành trang trí. Mặc định an
/// toàn là không có cờ; việc có mở một cờ như vậy không hay là quyết định của chủ nhân,
/// không phải của bản vá này.
///
/// Trên **hệ tệp cục bộ**, khoá bám vào **open-file-description** của `file`, nên nó sống
/// đúng bằng đời `File` trong [`Journal`]: đóng tệp (drop `Journal`, hoặc tiến trình chết
/// bằng bất cứ cách nào, kể cả `SIGKILL`) là nhả khoá. Không có tệp `.lock` mồ côi phải dọn.
///
/// **Ràng buộc đó KHÔNG phổ quát, và chỗ nó mất hiệu lực thì im lặng.** Trên NFS (và SMB),
/// Linux mô phỏng `flock` bằng khoá POSIX toàn tệp trừ khi mount `-o local_lock`. Khoá POSIX
/// gắn theo **TIẾN TRÌNH**, không theo open-file-description, nên nó bị nhả khi tiến trình
/// đóng **bất kỳ** mô tả tệp nào trỏ tới cùng tệp đó — kể cả một mô tả do đoạn mã khác mở.
/// `read_records` mở tệp thêm hai lượt rồi đóng, và ở `strata_node.rs` nó chạy ngay sau
/// `Journal::open`; dưới ngữ nghĩa mô phỏng, hai lượt đóng ấy nhả đúng cái khoá vừa giành.
///
/// Nghĩa là bảo đảm của issue #73 phủ ca hay xảy ra nhất (hai tiến trình, một máy, đĩa cục
/// bộ) và **KHÔNG** phủ ca hai bản chạy trên một volume dùng chung. Ca đó phải chặn ở tầng
/// khác — ràng buộc triển khai một-daemon-một-volume, hoặc bầu chủ ở tầng điều phối. Bài
/// kiểm `second_open_of_same_journal_is_refused` chạy trên `std::env::temp_dir()` nên nó
/// **không đo được** vế này; đừng đọc màu xanh của nó thành đã phủ.
#[cfg(unix)]
fn lock_exclusive(file: &File, path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::io::AsRawFd;

    // `sys/file.h`: hai hằng này cùng giá trị trên Linux và macOS/BSD.
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;

    // Khai tại chỗ thay vì kéo thêm một crate: đây là ĐÚNG MỘT lời gọi hệ thống, chữ ký
    // ổn định trên cả hai nền daemon này chạy. Thêm một phụ thuộc cho một dòng là thêm
    // một bề mặt phải nuôi và phải kiểm.
    unsafe extern "C" {
        fn flock(fd: i32, operation: i32) -> i32;
    }

    // SAFETY: `file` còn sống suốt lời gọi này (mượn `&File`), nên `as_raw_fd()` trả một
    // file descriptor hợp lệ, đang mở, thuộc sở hữu của `file`. `flock` không nhận và
    // không trả con trỏ nào — chỉ hai `int` vào, một `int` ra — nên KHÔNG có bộ nhớ Rust
    // nào đi qua biên FFI, và không có bất biến về vòng đời hay aliasing nào bị đặt cược.
    // Ta cũng không giữ lại `fd`: quyền sở hữu descriptor vẫn nằm ở `file`, nên không có
    // sở hữu kép và không có double-close.
    let rc = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if rc == 0 {
        return Ok(());
    }
    let cause = std::io::Error::last_os_error();
    Err(std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        format!(
            "nhật ký `{}` ĐANG BỊ GIỮ bởi một tiến trình khác ({cause}). Gần như luôn là \
             một daemon strata-node khác đang chạy trên cùng tệp này — kiểm tiến trình \
             đang sống trước khi khởi động lại. Hai daemon cùng ghi một nhật ký làm lịch \
             sử phân nhánh mà không lượt ghi nào báo lỗi, nên daemon này dừng ở đây thay \
             vì lên xanh",
            path.display()
        ),
    ))
}

/// Nền không phải Unix: biên dịch được, nhưng **KHÔNG có khoá**.
///
/// Nói thẳng ra đây thay vì để người đọc suy: daemon chỉ chạy thật trên macOS + Linux,
/// nên nhánh này tồn tại để `cargo check` các nền khác không gãy, chứ không phải để hứa
/// một sự bảo vệ nó không có. Cần chạy thật trên Windows thì phải cắm
/// `LockFileEx(LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY)` vào đúng chỗ này.
#[cfg(not(unix))]
fn lock_exclusive(_file: &File, _path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

impl Journal {
    /// Mở (tạo nếu chưa có) nhật ký tại `path` và **giành khoá độc quyền** trên nó; khoá
    /// giữ suốt đời [`Journal`]. Tệp mới ⇒ ghi header.
    ///
    /// Tệp đang bị một tiến trình khác giữ ⇒ `Err` với
    /// [`ErrorKind::WouldBlock`](std::io::ErrorKind::WouldBlock) — xem [`lock_exclusive`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let path = path.as_ref().to_path_buf();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&path)?;
        // Giành khoá TRƯỚC lượt ghi đầu tiên: header cũng là một lượt ghi, và hai tiến
        // trình cùng thấy tệp rỗng sẽ cùng ghi hai header vào một tệp.
        lock_exclusive(&file, &path)?;
        // ĐO `fresh` SAU KHI ĐÃ CÓ KHOÁ, và đo bằng ĐỘ DÀI chứ không bằng `Path::exists()`.
        // Hai lý do, cả hai đều đã dựng lại được:
        //   1. Đo trước khoá thì khoá không loại được ca nó sinh ra để loại — hai tiến trình
        //      cùng chạy qua phép đo lúc tệp chưa có, rồi lần lượt vào vùng khoá, và cả hai
        //      vẫn mang `fresh == true` ⇒ header thứ hai nối vào cuối một tệp đã có nội dung.
        //      Khoá chỉ nối tiếp hoá hai lượt ghi sai, không ngăn được lượt nào.
        //   2. `Path::exists()` trả `false` cho MỌI lỗi `stat` (mất quyền trên thư mục cha,
        //      handle mạng ôi, EIO) ⇒ một lượt `stat` hỏng thoáng qua trên nhật ký ĐANG SỐNG
        //      cũng cho `fresh == true`. Không cần đua, không cần kẻ tấn công.
        // Độ dài 0 là đúng điều kiện cần hỏi: tệp rỗng thì thiếu header, tệp có byte thì không.
        // Không phép kiểm nào bắt được header thừa nếu lọt — `replay.rs` bỏ qua `Header` ở mọi
        // vị trí, và `read_records` chỉ soi bản ghi ĐẦU.
        let fresh = file.metadata()?.len() == 0;
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
