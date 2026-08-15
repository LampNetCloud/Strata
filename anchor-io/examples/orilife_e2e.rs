//! Lượt chạy **đầu-cuối thật**: OriLife (giả lập) → Strata daemon → cửa Mosaic → Cardano.
//!
//! ```text
//! ví dụ này            daemon strata-node          cửa mosaic-anchor-door       Cardano
//!   ký Ed25519 THẬT  ──▶ create/version  ──▶
//!   quyết lô (vai      ──▶ _anchor_batch ──▶ publish_batch (INV-E7 + encode)
//!   BatchCoordinator)                    ──▶ POST strata-anchor-batch ──▶ dựng tx ──▶ txid
//! ```
//!
//! **Cái gì giả lập, cái gì thật.** Giả lập đúng **một** thứ: *danh tính* — `author_did`
//! là hằng suy từ seed thay vì DID thật do PhoenixKey phân giải. Mọi thứ còn lại là
//! thật: khoá Ed25519 thật, chữ ký thật (daemon `verify_strict` sẽ từ chối nếu sai),
//! `state_root`/`version_hash`/`mmr_root` do core tính, tx thật trên Preprod, phí thật.
//! Ghi rõ ranh giới này vì *"chạy được đầu-cuối"* mà mọi mắt xích đều mock thì chỉ
//! chứng minh các mock khớp nhau.
//!
//! # Dùng
//!
//! ```bash
//! # 1) In cấu hình key-registry cho daemon (daemon chỉ nạp KHOÁ CÔNG KHAI)
//! cargo run -p lampnet-anchor-io --example orilife_e2e -- --print-keys
//! # 2) Chạy lô
//! STRATA_URL=http://127.0.0.1:6690 SIM_TREES=3 SIM_VERSIONS=2 \
//!   cargo run -p lampnet-anchor-io --example orilife_e2e
//! ```
//!
//! ENV: `STRATA_URL` (mặc định `http://127.0.0.1:6690`) · `SIM_TREES` (mặc định 3) ·
//! `SIM_VERSIONS` số version nối thêm mỗi cây (mặc định 1) · `SIM_SEED_BASE` (mặc định
//! 200) · `SIM_ROUNDS` số vòng neo (mặc định 1; >1 để chạm nhánh **di chuyển** beacon).

use ed25519_dalek::{SigningKey, VerifyingKey};
use lampnet_strata::chain::Policy;
use lampnet_strata::state::build_state_root;
use lampnet_strata::version::{Hash32, StrataVersion};
use serde_json::{Value, json};

/// Khoá của "cây thứ i". Tất định để chạy lại được, và **công khai được** — đây là
/// ví mô phỏng, không phải khoá vận hành.
fn key_of(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

/// `author_did` giả lập: 32 byte suy từ seed. Đây **đúng là** chỗ PhoenixKey sẽ thay.
fn did_of(seed: u8) -> [u8; 32] {
    [seed ^ 0x5a; 32]
}

fn policy_of(seed: u8) -> Policy {
    let mut p = Policy::new();
    p.allow(did_of(seed), VerifyingKey::from(&key_of(seed)));
    p
}

/// Ký như client: dựng đúng version rồi lấy `sig`. Daemon KHÔNG ký hộ — nó chỉ kiểm.
#[allow(clippy::too_many_arguments)]
fn sign_version(
    seed: u8,
    seq: u64,
    prev_hash: Hash32,
    content_cid: &[u8],
    fields: &[(Vec<u8>, Vec<u8>)],
    policy_hash: Hash32,
    ts: u64,
) -> String {
    let mut v = StrataVersion::unsigned(
        seq,
        prev_hash,
        content_cid.to_vec(),
        build_state_root(fields),
        did_of(seed),
        policy_hash,
        ts,
    );
    v.sign(&key_of(seed));
    hex::encode(v.sig)
}

fn field(key: &str, value_hex: &str) -> (Vec<u8>, Vec<u8>) {
    (key.as_bytes().to_vec(), hex::decode(value_hex).unwrap())
}

fn post(client: &reqwest::blocking::Client, url: &str, body: &Value) -> Result<Value, String> {
    let resp = client
        .post(url)
        .json(body)
        .send()
        .map_err(|e| format!("POST {url}: {e}"))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        return Err(format!("POST {url} → HTTP {status}: {text}"));
    }
    Ok(v)
}

fn env_num(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(default)
}

fn main() -> Result<(), String> {
    let seed_base = env_num("SIM_SEED_BASE", 200) as u8;
    let trees = env_num("SIM_TREES", 3) as u8;
    let versions = env_num("SIM_VERSIONS", 1);
    // Số VÒNG neo — mỗi vòng là một lô. Vòng >1 chạm nhánh "beacon đã tồn tại".
    let rounds = env_num("SIM_ROUNDS", 1).max(1);

    // --print-keys: daemon cần `did_hex:pubkey_hex` để phân giải chữ ký. In riêng vì
    // daemon phải khởi động TRƯỚC khi ví dụ này chạy.
    if std::env::args().any(|a| a == "--print-keys") {
        let spec: Vec<String> = (0..trees)
            .map(|i| {
                let seed = seed_base + i;
                format!(
                    "{}:{}",
                    hex::encode(did_of(seed)),
                    hex::encode(VerifyingKey::from(&key_of(seed)).to_bytes())
                )
            })
            .collect();
        println!("{}", spec.join(","));
        return Ok(());
    }

    let base = std::env::var("STRATA_URL").unwrap_or_else(|_| "http://127.0.0.1:6690".into());
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .map_err(|e| e.to_string())?;
    // Đồng hồ thật: daemon có gác `ts` (chặn mili-giây, chặn tương lai xa).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();

    let mut refs: Vec<String> = Vec::new();
    // (seed, policy_hash, head_seq, head_version_hash) — để nối version ở vòng sau.
    let mut heads: Vec<(u8, Hash32, u64, Hash32)> = Vec::new();
    for i in 0..trees {
        let seed = seed_base + i;
        let policy = policy_of(seed);
        let ph = policy.policy_hash();
        let value = format!("{:02x}{}", seed, "00".repeat(31));
        let fields = vec![field("tree_state", &value)];

        // ── genesis ───────────────────────────────────────────────────────────
        let body = json!({
            "author_did": hex::encode(did_of(seed)),
            "genesis_nonce": hex::encode([seed ^ 0x11; 32]),
            "content_cid": "cafe",
            "state_fields": [{ "key": "tree_state", "value": value }],
            "policy_hash": hex::encode(ph),
            "ts": now,
            "sig": sign_version(seed, 0, [0u8; 32], b"\xca\xfe", &fields, ph, now),
        });
        let r = post(&client, &format!("{base}/v1/strata/create"), &body)?;
        let ref_id = r["ref_id"]
            .as_str()
            .ok_or("create: thiếu ref_id")?
            .to_string();
        let mut prev_hash: Hash32 = hex::decode(r["head_version_hash"].as_str().unwrap())
            .map_err(|e| e.to_string())?
            .try_into()
            .map_err(|_| "head_version_hash không phải 32 byte")?;
        let mut prev_seq = r["head_seq"].as_u64().unwrap_or(0);

        // ── các version tiếp theo ─────────────────────────────────────────────
        for k in 0..versions {
            let v_hex = format!("{:02x}{:02x}{}", seed, k as u8, "00".repeat(30));
            let f2 = vec![field("tree_state", &v_hex)];
            let ts = now + k + 1;
            let body = json!({
                "prev_seq": prev_seq,
                "content_cid": "beef",
                "state_fields": [{ "key": "tree_state", "value": v_hex }],
                "author_did": hex::encode(did_of(seed)),
                "policy_hash": hex::encode(ph),
                "ts": ts,
                "sig": sign_version(seed, prev_seq + 1, prev_hash, b"\xbe\xef", &f2, ph, ts),
            });
            let r = post(
                &client,
                &format!("{base}/v1/strata/{ref_id}/version"),
                &body,
            )?;
            prev_seq = r["seq"].as_u64().ok_or("append: thiếu seq")?;
            prev_hash = hex::decode(r["version_hash"].as_str().unwrap())
                .map_err(|e| e.to_string())?
                .try_into()
                .map_err(|_| "version_hash không phải 32 byte")?;
        }
        println!("cây {i}: ref={ref_id} head_seq={prev_seq}");
        refs.push(ref_id);
        heads.push((seed, ph, prev_seq, prev_hash));
    }

    // ── neo LÔ: vai `BatchCoordinator` — quyết lô gồm những ref nào ────────────
    //
    // Nhiều VÒNG neo là để chạm nhánh thứ hai của beacon: vòng 1 **mint** beacon
    // (chưa tồn tại), vòng 2 trở đi **di chuyển** nó (tiêu UTxO đang giữ rồi phát
    // lại). Hai nhánh khác hẳn nhau ở phần cộng-trừ value, và nhánh di-chuyển là
    // nhánh KHÔNG chạy ở lượt đầu — tức nếu chỉ chạy một vòng thì nó vẫn chỉ TRÔNG
    // như tồn tại.
    for round in 1..=rounds {
        if round > 1 {
            // Mỗi vòng sau: mỗi cây thêm một version ⇒ `seq` tiến, lô mới hợp lệ.
            for (i, ref_id) in refs.iter().enumerate() {
                let (seed, ph, prev_seq, prev_hash) = heads[i];
                let v_hex = format!("{:02x}{:02x}{}", seed, 0xF0 + round as u8, "00".repeat(30));
                let f2 = vec![field("tree_state", &v_hex)];
                let ts = now + 100 * round;
                let body = json!({
                    "prev_seq": prev_seq,
                    "content_cid": "beef",
                    "state_fields": [{ "key": "tree_state", "value": v_hex }],
                    "author_did": hex::encode(did_of(seed)),
                    "policy_hash": hex::encode(ph),
                    "ts": ts,
                    "sig": sign_version(seed, prev_seq + 1, prev_hash, b"\xbe\xef", &f2, ph, ts),
                });
                let r = post(&client, &format!("{base}/v1/strata/{ref_id}/version"), &body)?;
                heads[i].2 = r["seq"].as_u64().ok_or("append: thiếu seq")?;
                heads[i].3 = hex::decode(r["version_hash"].as_str().unwrap())
                    .map_err(|e| e.to_string())?
                    .try_into()
                    .map_err(|_| "version_hash không phải 32 byte")?;
            }
        }

        println!(
            "\n[vòng {round}/{rounds}] bắn lô {} ref → /v1/strata/_anchor_batch …",
            refs.len()
        );
        let resp = post(
            &client,
            &format!("{base}/v1/strata/_anchor_batch"),
            &json!({ "refs": refs, "priority": "immediate" }),
        )?;
        match resp["anchor_txid"].as_str() {
            Some(txid) => println!("✅ vòng {round}: txid = {txid}"),
            None => println!("⚠️  vòng {round}: không có txid (lô idempotent hoặc no_anchor)"),
        }
    }
    Ok(())
}
