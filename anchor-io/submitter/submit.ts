/**
 * submit.ts — build + sign + submit tx label 1234 (S1 AnchorSink, backend Settlement)
 * lên Cardano Preview qua Blockfrost + Lucid Evolution.
 *
 * Giao thức: stdin nhận MỘT JSON request, stdout in MỘT dòng JSON response.
 *   req submit : { label:number, records:[Rec], beacon?:boolean }
 *   req op     : { op:"policy_id", address?:string }   // suy policyId beacon, KHÔNG submit
 *   Rec        : { t:1, ref_id:hex64, head_version_hash:hex64, mmr_root:hex64, seq:number }
 *              | { t:2, payload:hex }
 *   resp submit: { ok:true, txid, address, policy_id? }
 *   resp op    : { ok:true, policy_id, address }
 *              | { ok:false, error_kind:"NotConfigured"|"Network"|"InsufficientAda"
 *                            |"DatumTooLarge"|"Rejected", error }
 *
 * Metadata layout (KHỚP payload.rs — CBOR raw bytes, KHÔNG JSON-hex):
 *   metadatum = [ { "t":int, "a":[...] } ];  t=1: a=[b32 ref_id, b32 hvh, b32 mmr_root, int seq]
 *   bytes > 64B → mảng chunk canonical (mọi chunk trừ cuối đúng 64B).
 *
 * BEACON mode (beacon:true, issue #14): mỗi anchor t=1 gắn một beacon NFT
 *   unit = policyId ‖ ref_id  (assetName = ref_id 32B; policy = native `sig(publisher)`).
 *   Chưa có beacon → MINT; đã có → SPEND UTxO giữ beacon + gửi beacon sang UTxO mới mang
 *   metadata anchro. `resolve` off-chain tra asset-index → miễn nhiễm flood-eviction.
 *
 * SECRET: VEDATA_WALLET_MNEMONIC + BLOCKFROST_TOKEN_GREENSUN chỉ đọc từ env.
 * TUYỆT ĐỐI không in ra stdout/stderr — mọi message lỗi đi qua redact() trước.
 */

import {
  Lucid,
  Blockfrost,
  scriptFromNative,
  mintingPolicyToId,
  paymentCredentialOf,
  type Script,
} from "@lucid-evolution/lucid";

type Rec =
  | { t: 1; ref_id: string; head_version_hash: string; mmr_root: string; seq: number }
  | { t: 2; payload: string };

type Req = { label: number; records: Rec[]; beacon?: boolean; op?: string; address?: string };

// ---------------------------------------------------------------------------
// Secret hygiene
// ---------------------------------------------------------------------------

const SECRETS: string[] = [];
for (const k of ["VEDATA_WALLET_MNEMONIC", "BLOCKFROST_TOKEN_GREENSUN"]) {
  const v = process.env[k];
  if (v && v.trim()) SECRETS.push(v.trim());
}

/** Xoá mọi vết secret khỏi chuỗi trước khi in. */
function redact(s: string): string {
  let out = s;
  for (const sec of SECRETS) {
    while (out.includes(sec)) out = out.replaceAll(sec, "[REDACTED]");
    // Mnemonic có thể bị wrap/đổi whitespace trong error → so bản chuẩn hoá.
    const norm = sec.replace(/\s+/g, " ");
    if (norm !== sec) while (out.includes(norm)) out = out.replaceAll(norm, "[REDACTED]");
  }
  return out;
}

function ok(obj: Record<string, unknown>): never {
  process.stdout.write(JSON.stringify({ ok: true, ...obj }) + "\n");
  process.exit(0);
}

function fail(kind: string, msg: string, extra: Record<string, unknown> = {}): never {
  process.stdout.write(
    JSON.stringify({ ok: false, error_kind: kind, error: redact(msg), ...extra }) + "\n"
  );
  process.exit(1);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function hexToBytes(hex: string, name: string, expectLen?: number): Uint8Array {
  if (!/^[0-9a-fA-F]*$/.test(hex) || hex.length % 2 !== 0) {
    fail("Rejected", `${name}: hex không hợp lệ`);
  }
  const b = new Uint8Array(hex.length / 2);
  for (let i = 0; i < b.length; i++) b[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  if (expectLen !== undefined && b.length !== expectLen) {
    fail("Rejected", `${name}: cần ${expectLen} byte, có ${b.length}`);
  }
  return b;
}

/** Chunk canonical 64B — khớp payload.rs::encode_bytes_chunked. */
function chunk64(b: Uint8Array): Uint8Array | Uint8Array[] {
  if (b.length <= 64) return b;
  const out: Uint8Array[] = [];
  for (let i = 0; i < b.length; i += 64) out.push(b.slice(i, i + 64));
  return out;
}

/** Native minting policy `sig(pkh)` cho beacon — publisher là người ký duy nhất. */
function beaconPolicy(pkh: string): Script {
  return scriptFromNative({ type: "sig", keyHash: pkh });
}

async function readStdin(): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const c of process.stdin) chunks.push(c as Buffer);
  return Buffer.concat(chunks).toString("utf-8");
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

async function main(): Promise<void> {
  let req: Req;
  try {
    req = JSON.parse(await readStdin()) as Req;
  } catch (e) {
    fail("Rejected", `request JSON hỏng: ${String(e)}`);
  }

  // --- op: policy_id (suy policyId beacon; offline nếu có `address`) ---------
  if (req.op === "policy_id") {
    if (req.address && req.address.trim()) {
      // Suy pkh từ địa chỉ công khai — KHÔNG cần ví/provider/secret.
      let pkh: string;
      try {
        pkh = paymentCredentialOf(req.address.trim()).hash;
      } catch (e) {
        fail("Rejected", `address không hợp lệ: ${String(e)}`);
      }
      ok({ policy_id: mintingPolicyToId(beaconPolicy(pkh)), address: req.address.trim() });
    }
    // Không có address → suy từ ví (cần mnemonic + provider).
    const { lucid, address } = await initWallet();
    const pkh = paymentCredentialOf(address).hash;
    ok({ policy_id: mintingPolicyToId(beaconPolicy(pkh)), address });
    void lucid;
  }

  // --- submit -----------------------------------------------------------------
  if (!Number.isInteger(req.label) || req.label < 0) {
    fail("Rejected", "label phải là số nguyên không âm");
  }
  if (!Array.isArray(req.records) || req.records.length === 0) {
    fail("Rejected", "records rỗng");
  }

  // Dựng metadatum: [ { t, a } ] — thứ tự key cố định "t" rồi "a" (deterministic,
  // khớp decoder Rust; lucid-evolution giữ insertion order khi build CBOR map).
  const metadata = req.records.map((r) => {
    if (r.t === 1) {
      if (!Number.isSafeInteger(r.seq) || r.seq < 0) {
        fail("Rejected", `seq ${r.seq} không phải số nguyên an toàn không âm`);
      }
      return {
        t: 1,
        a: [
          chunk64(hexToBytes(r.ref_id, "ref_id", 32)),
          chunk64(hexToBytes(r.head_version_hash, "head_version_hash", 32)),
          chunk64(hexToBytes(r.mmr_root, "mmr_root", 32)),
          r.seq,
        ],
      };
    }
    if (r.t === 2) {
      return { t: 2, a: [chunk64(hexToBytes(r.payload, "payload"))] };
    }
    fail("Rejected", `record t không hỗ trợ: ${JSON.stringify(r)}`);
  });

  const { lucid, address } = await initWallet();

  try {
    let tx = lucid.newTx();

    if (req.beacon) {
      // BEACON-WALK: mỗi anchor t=1 → mint/di chuyển beacon unit=policyId‖ref_id.
      const pkh = paymentCredentialOf(address).hash;
      const policy = beaconPolicy(pkh);
      const policyId = mintingPolicyToId(policy);
      const toMint: Record<string, bigint> = {};
      let needPolicy = false;

      for (const r of req.records) {
        if (r.t !== 1) continue; // chỉ anchor có beacon (t=2 reserved)
        const unit = policyId + r.ref_id.toLowerCase(); // assetName = ref_id hex (32B)
        const held = await lucid.utxosAtWithUnit(address, unit);
        if (held.length === 0) {
          toMint[unit] = 1n; // beacon chưa tồn tại → mint
          needPolicy = true;
        } else {
          tx = tx.collectFrom([held[0]]); // đã tồn tại → tiêu UTxO giữ beacon
        }
        // Gửi beacon (mới mint hoặc vừa tiêu) sang UTxO mới — 1 beacon / 1 output.
        tx = tx.pay.ToAddress(address, { lovelace: 2_000_000n, [unit]: 1n });
      }
      if (needPolicy) {
        tx = tx.mintAssets(toMint).attach.MintingPolicy(policy);
        // Native `sig` policy → tx phải có chữ ký của pkh (ví ký, khai báo tường minh).
        tx = tx.addSignerKey(pkh);
      }
      tx = tx.attachMetadata(req.label, metadata as never);

      const completed = await tx.complete();
      const signed = await completed.sign.withWallet().complete();
      const txid = await signed.submit();
      ok({ txid, address, policy_id: policyId });
    }

    // Không beacon: tx metadata tối thiểu (1 output tự-trả, Lucid tự lo input+change).
    const completed = await tx
      .pay.ToAddress(address, { lovelace: 2_000_000n })
      .attachMetadata(req.label, metadata as never)
      .complete();
    const signed = await completed.sign.withWallet().complete();
    const txid = await signed.submit();
    ok({ txid, address });
  } catch (e) {
    const msg = e instanceof Error ? (e.stack ?? e.message) : String(e);
    if (/insufficient|not enough|exhausted|InputsExhausted|MissingInput/i.test(msg)) {
      fail("InsufficientAda", msg, { need: 0, have: 0 });
    }
    if (/maximum transaction size|MaxTxSize|too large|exceeds/i.test(msg)) {
      fail("DatumTooLarge", msg, { bytes: 0 });
    }
    if (/fetch|network|timeout|ECONN|ENOTFOUND|50\d|429/i.test(msg)) {
      fail("Network", msg);
    }
    fail("Rejected", msg);
  }
}

/** Khởi tạo Lucid (Preview) + chọn ví từ seed; trả địa chỉ base (CIP-1852 acct0/idx0). */
async function initWallet(): Promise<{ lucid: Awaited<ReturnType<typeof Lucid>>; address: string }> {
  const mnemonic = process.env.VEDATA_WALLET_MNEMONIC?.trim().replace(/\s+/g, " ");
  const bfToken = process.env.BLOCKFROST_TOKEN_GREENSUN?.trim();
  if (!mnemonic) fail("NotConfigured", "thiếu env VEDATA_WALLET_MNEMONIC");
  if (!bfToken) fail("NotConfigured", "thiếu env BLOCKFROST_TOKEN_GREENSUN");
  let lucid;
  try {
    lucid = await Lucid(
      new Blockfrost("https://cardano-preview.blockfrost.io/api/v0", bfToken),
      "Preview"
    );
  } catch (e) {
    fail("Network", `khởi tạo Lucid/Blockfrost: ${String(e)}`);
  }
  lucid.selectWallet.fromSeed(mnemonic!);
  const address = await lucid.wallet().address();
  return { lucid, address };
}

main().catch((e) => {
  const msg = e instanceof Error ? (e.stack ?? e.message) : String(e);
  fail("Rejected", `lỗi không bắt được: ${msg}`);
});
