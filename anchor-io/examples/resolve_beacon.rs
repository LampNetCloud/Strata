//! Smoke read-side beacon (#14) trên Preview thật: dựng `SettlementSink` beacon mode +
//! `BlockfrostQuery`, gọi `resolve(ref_id)` → in anchor đọc được từ asset-index.
//!
//! Chạy:
//!   BLOCKFROST_TOKEN_GREENSUN=<token> \
//!   STRATA_BEACON_POLICY=<policyId hex 56> \
//!   STRATA_REF_ID=<ref_id hex 64> \
//!   STRATA_PUBLISHER=<addr publisher> \
//!   cargo run -p lampnet-anchor-io --example resolve_beacon
//!
//! KHÔNG in secret. Token chỉ đọc từ env, dùng để khởi tạo `BlockfrostQuery`.

use lampnet_anchor_io::BlockfrostQuery;
use lampnet_strata::settlement::{SettlementRecord, SettlementSink, SinkConfig, Submitter};
use lampnet_strata::{AnchorError, AnchorSink, SubmitOutcome};

/// Submitter giả — example chỉ đọc (`resolve`), không submit.
struct NoSubmit;
impl Submitter for NoSubmit {
    fn submit(&self, _r: &[SettlementRecord]) -> Result<SubmitOutcome, AnchorError> {
        Err(AnchorError::Network("resolve_beacon: không submit".into()))
    }
}

fn hex32(s: &str, name: &str) -> [u8; 32] {
    let v = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect::<Vec<_>>();
    assert_eq!(v.len(), 32, "{name} cần 32 byte");
    let mut a = [0u8; 32];
    a.copy_from_slice(&v);
    a
}

fn main() {
    let token =
        std::env::var("BLOCKFROST_TOKEN_GREENSUN").expect("thiếu BLOCKFROST_TOKEN_GREENSUN");
    let policy = std::env::var("STRATA_BEACON_POLICY").expect("thiếu STRATA_BEACON_POLICY");
    let ref_hex = std::env::var("STRATA_REF_ID").expect("thiếu STRATA_REF_ID");
    let publisher = std::env::var("STRATA_PUBLISHER").expect("thiếu STRATA_PUBLISHER");
    let ref_id = hex32(&ref_hex, "STRATA_REF_ID");

    let cfg = SinkConfig {
        publisher_address: publisher,
        beacon_policy: Some(policy),
        ..Default::default()
    };
    let sink = SettlementSink::new(cfg, BlockfrostQuery::preview(token), NoSubmit);

    match sink.resolve(&ref_id) {
        Ok(Some(a)) => {
            println!(
                "resolve OK: seq={} ref_id={} hvh={} mmr_root={}",
                a.seq,
                hex_lower(&a.ref_id),
                hex_lower(&a.head_version_hash),
                hex_lower(&a.mmr_root),
            );
        }
        Ok(None) => println!("resolve = None (beacon chưa index / chưa neo)"),
        Err(e) => println!("resolve LỖI: {e:?}"),
    }
}

fn hex_lower(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
