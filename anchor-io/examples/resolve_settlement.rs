//! Đọc ngược lô vừa neo: `SettlementSink::resolve()` **thật** trên chuỗi thật.
//!
//! Vì sao cần một ví dụ riêng cho việc này. Sau khi neo, thứ dễ làm là mở
//! Blockfrost, nhìn thấy metadata label 1234, rồi kết luận "neo xong". Đó là đọc
//! bằng **mắt mình**, không phải bằng **đường đọc của hệ**. Hai thứ đó khác nhau ở
//! đúng chỗ đắt nhất: `resolve()` còn phải lọc theo **địa chỉ INPUT** (chỉ tin tx do
//! publisher CHI — chống đầu độc indexer), phải decode **khoan dung** đúng luật
//! chunk, và phải chọn `seq` cao nhất. Một lô lên chuỗi mà `resolve()` không thấy
//! thì với hệ thống nó **chưa hề được neo** — và nhìn bằng mắt sẽ không phát hiện ra.
//!
//! Chạy:
//! ```bash
//! BLOCKFROST_API_KEY=<token preprod> \
//! STRATA_ANCHOR_NETWORK=preprod \
//! STRATA_PUBLISHER_ADDRESS=<addr ví submit> \
//! STRATA_REF_IDS=<hex64>,<hex64>,… \
//!   cargo run -p lampnet-anchor-io --example resolve_settlement
//! ```
//! KHÔNG in secret.

use lampnet_anchor_io::BlockfrostQuery;
use lampnet_strata::settlement::{SettlementRecord, SettlementSink, SinkConfig, Submitter};
use lampnet_strata::{AnchorError, AnchorSink, SubmitOutcome};

/// Submitter giả — ví dụ này CHỈ đọc.
struct NoSubmit;
impl Submitter for NoSubmit {
    fn submit(&self, _r: &[SettlementRecord]) -> Result<SubmitOutcome, AnchorError> {
        Err(AnchorError::Network(
            "resolve_settlement: không submit".into(),
        ))
    }
}

fn hex32(s: &str) -> [u8; 32] {
    let v = hex::decode(s.trim()).expect("ref_id phải là hex");
    assert_eq!(v.len(), 32, "ref_id cần 32 byte, có {}", v.len());
    let mut a = [0u8; 32];
    a.copy_from_slice(&v);
    a
}

fn base_of(net: &str) -> &'static str {
    match net {
        "preview" => "https://cardano-preview.blockfrost.io/api/v0",
        "preprod" => "https://cardano-preprod.blockfrost.io/api/v0",
        "mainnet" => "https://cardano-mainnet.blockfrost.io/api/v0",
        other => panic!("mạng lạ: {other}"),
    }
}

fn main() {
    let net = std::env::var("STRATA_ANCHOR_NETWORK").unwrap_or_else(|_| "preprod".into());
    let token = ["BLOCKFROST_API_KEY", "BLOCKFROST_TOKEN_GREENSUN"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
        .expect("thiếu project-id Blockfrost");
    let publisher =
        std::env::var("STRATA_PUBLISHER_ADDRESS").expect("thiếu STRATA_PUBLISHER_ADDRESS");
    let refs = std::env::var("STRATA_REF_IDS").expect("thiếu STRATA_REF_IDS");

    let cfg = SinkConfig {
        publisher_address: publisher.trim().to_string(),
        beacon_policy: std::env::var("STRATA_BEACON_POLICY")
            .ok()
            .filter(|p| !p.trim().is_empty()),
        ..SinkConfig::default()
    };
    let sink = SettlementSink::new(
        cfg,
        BlockfrostQuery::new(base_of(&net).to_string(), token.trim().to_string()),
        NoSubmit,
    );

    let mut found = 0usize;
    let mut total = 0usize;
    for r in refs.split(',').filter(|s| !s.trim().is_empty()) {
        total += 1;
        match sink.resolve(&hex32(r)) {
            Ok(Some(a)) => {
                found += 1;
                println!(
                    "✅ {}\n   head_version_hash={}\n   mmr_root={}\n   seq={}",
                    hex::encode(a.ref_id),
                    hex::encode(a.head_version_hash),
                    hex::encode(a.mmr_root),
                    a.seq
                );
            }
            // "Chưa neo" và "neo rồi mà đọc không ra" trông giống hệt nhau ở đây —
            // nên phải nói thẳng là kết quả này KHÔNG chứng minh được gì tốt.
            Ok(None) => println!("❌ {}: resolve() KHÔNG thấy anchor nào", r.trim()),
            Err(e) => println!("⚠️  {}: resolve() lỗi: {e}", r.trim()),
        }
    }
    println!("\n{found}/{total} ref đọc lại được qua đường resolve() của Strata");
    if found != total {
        std::process::exit(1);
    }
}
