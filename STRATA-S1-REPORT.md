# Strata S1 — Anchor adapter `Strata.anchor → Mosaic` (CIP-68) — Báo cáo

> **Issue:** [#1 [P0]](https://github.com/LampNetCloud/Strata/issues/1) · **Spec:** `Strata-API.md §4.1 + §8.1`, `_CONTRACT.md` (phương án A) · **Cập nhật:** 2026-07-04
> **Trạng thái:** Phase 1 (Strata Rust) CODE-COMPLETE + **Phase 2 ĐÃ NEO ON-CHAIN THẬT (L1 Preview)**. Repo **71 test pass**, clippy sạch.

---

## 1. Phạm vi + quyết định đã chốt (anh Đức, issue #1)

S1 = cầu nối **Strata neo on-chain qua Mosaic (VeData)**. Chốt kỹ thuật (comment issue #1):
- **`AnchorSink` = trait cắm được, mặc định theo platform** → VeData dùng `MosaicAnchorSink` (CIP-68). OriLife 1454/1455 KHÔNG hội tụ (thêm sink riêng nếu cần).
- **Byte-layout anchor = PHƯƠNG ÁN A** (`_CONTRACT.md`): `(ref_id, head_version_hash, mmr_root, seq)` = 104 byte. KHÔNG `state_root` (đã bắc-cầu qua `head_version_hash`). **Khớp struct `StrataAnchor` sẵn có → 0 rework byte-neo.**
- **Bảng daemon** `anchored: Vec<(seq, mmr_root, mmr_size)>` — state daemon, KHÔNG nhét vào core thuần.
- **Ranh giới:** Strata giữ logic chain + sinh datum; **Mosaic dựng+submit tx; KHÔNG dựng tx trong Strata.**

---

## 2. Phase 1 — Strata Rust (`src/anchor_sink.rs`)

| Thành phần | Vai trò |
|---|---|
| trait `AnchorSink` | `publish(anchor, priority) → Option<AnchorReceipt>` + `resolve(ref_id) → Option<StrataAnchor>` (§4.1 + §8.1c) |
| kiểu | `AnchorPriority` (Immediate/Milestone/BatchDaily/NoAnchor) · `AnchorReceipt` · `AnchorBackend` · `AnchorError` (6 biến thể §8.1b, `is_retryable` chỉ `Network`) |
| `PlutusData` + codec | `map_anchor_to_datum`/`parse_datum_to_anchor` (CIP-68 `Constr 0 [meta, version=1, extra]`, thứ tự `extra` = canonical anchor); `to_cbor`/`from_cbor` (ledger Cardano, tag 121, mảng indefinite); **`to_detailed_json`** (detailed-schema cardano-cli/Lucid nạp thẳng) |
| `MosaicBackend` (seam) | boundary ra Mosaic: `on_chain_seq` · `submit_anchor(datum)` · `read_anchor` — real impl gọi Mosaic SDK/Lucid (Phase 2); test dùng mock |
| `MosaicAnchorSink<B>` | impl `AnchorSink`: idempotency (query seq trước build → `Ok(None)` nếu `on_chain==anchor.seq`) + rollback (`on_chain>anchor.seq` → `RollbackAttempt`) |
| `AnchoredTable` + `verify_resolved` | bảng daemon lưu `AnchorRecord{seq, mmr_root, mmr_size, version_hash, proof}` (proof TẠI lúc neo — verify dưới root CŨ cần proof size-cũ); `verify_resolved` §8.1c |

**Ghi chú thiết kế:** bảng lưu thêm `proof` (ngoài `(seq,mmr_root,mmr_size)` spec nêu) vì proof MMR phụ thuộc size — proof sinh ở size mới KHÔNG verify dưới root đã neo. Đây là state daemon, không đụng core.

### 2.1 Test — 6 tiêu chí §8.1 (`tests/anchor_sink.rs`, mock backend)
| # | Test | Trạng thái |
|---|---|---|
| 1 | `map_anchor_to_datum` round-trip bit-exact (datum + CBOR + seq=u64::MAX) | ✅ |
| 2 | publish → resolve → datum khớp `mmr_root`/`head_version_hash` | ✅ (mock; **bản on-chain thật ở §3**) |
| 3 | INV-E7 rollback: neo lại seq cũ → `RollbackAttempt`, KHÔNG đẻ tx | ✅ |
| 4 | idempotent: publish cùng seq 2 lần → `Ok(None)`, tx=1; `NoAnchor`→`Ok(None)` | ✅ |
| 5 | resolve sau append: neo seq=1, append→seq=5, verify version seq=1 dưới root ĐÃ NEO (size cũ) PASS | ✅ |
| 6 | `DatumTooLarge`/`InsufficientAda`/`Network` → đúng biến thể, phân tầng retry, KHÔNG panic | ✅ |

---

## 3. Phase 2 — NEO ON-CHAIN THẬT (L1 Preview) ✅

Đóng DoD test #2 "Preview tx hash thật + datum khớp" trên **L1 Cardano Preview thật** (reuse rig node M6: cardano-node synced + ví/key alice + cardano-cli). Luồng đúng ranh giới issue:

1. **Strata (Rust)** sinh datum: `cargo run --example emit_anchor_datum` → anchor của 1 chain 2-version → `to_detailed_json()`.
2. **Mosaic/VeData (PoC cardano-cli, thay Lucid)** dựng tx L1: 1 output mang **inline-datum CIP-68** + submit → tx hash.
3. **resolve**: query UTxO on-chain → đọc `inlineDatum` → assert `mmr_root` + `seq` + `name` khớp.

### 3.1 Bảng giao dịch (L1 Preview — tra explorer)
Explorer: `https://preview.cardanoscan.io/transaction/<txid>`

| Giao dịch | TxId | Ý nghĩa |
|---|---|---|
| Neo anchor CIP-68 | `44bb3b91894e77eec3ffc41dcf5c23a1dffabdb1da10f50b338c9aafc46c89e8` | Output `#0` = UTxO inline-datum CIP-68 mang anchor `(ref_id, head_version_hash, mmr_root, seq=1)`; fee 0.177 tADA |

**Anchor đã neo (chain deterministic, seed cố định):**
- `ref_id` = `365e4a028012f42e95d85e3ae6842c1862e2c139850c6a6baaa78f7d929dc0fe`
- `head_version_hash` = `6b27fbb6be1faf9d4f1f661cc681b51ff34823f39fd0b8999b31d4bd6f55cfa7`
- `mmr_root` = `3d0b68c958f6e39d579d41117cb96a717ea78acc5f9422efcdfeeb872f0c22a7`
- `seq` = 1

**resolve on-chain khớp:** `mmr_root` ✓ · `seq=1` ✓ · `name="LN-STRATA-ANCHOR"` (hex `4c4e2d5354524154412d414e43484f52`) ✓.

---

## 4. Validator Aiken (test #3 INV-E7 on-chain) — ✅ CODE-COMPLETE + **ON-CHAIN THẬT** (VeData/Code)

Enforce INV-E7 on-chain đã hiện thực ở **VeData/Code** (đúng ranh giới "Mosaic giữ validator"):
- `mosaic/aiken/validators/strata_anchor.ak` (Plutus V3) + lib `mosaic/aiken/lib/strata/anchor.ak`.
- State-thread reference-UTxO spend-recreate: `seq' == seq+1` (T4) + `ref_id` bất biến + single-successor (T2) + value-preserved (T5) + datum CIP-68 hợp lệ (T3).
- **9 test validator + 12 test lib** (`aiken check`: toàn project **156 pass**), gồm **test #3**: `spend_rejects_rollback` (seq 1→0), `spend_rejects_replay_same_seq` (1→1), `spend_rejects_seq_skip` (1→3).
- `aiken build` → **script hash `1c7c7ca8ff353a2475697ab9219a3d961de6923cea2cf9b8b99f58ca`** (deployable).

### 4.1 Deploy Preview + test #3 SPEND-RECREATE ON-CHAIN THẬT (2026-07-06) ✅

Đóng DoD "test #3 on-chain thật": neo anchor tại **script-address** rồi thử advance/rollback trên **L1 Cardano Preview thật** (reuse rig node M6: cardano-node synced 100% + ví/key alice + `cardano-cli`, thay Lucid cho PoC). Anchor dùng đúng bộ giá trị của §3 (cùng "chain" Strata: `ref_id=365e4a…`, `mmr_root=3d0b68…`).

| Thứ | Giá trị |
|---|---|
| Script address (Preview) | `addr_test1wqw8cl9glu6n5fr4d9atjgv68ktpme5j8n4ze7dchx043jst02nh7` |
| Script hash | `1c7c7ca8ff353a2475697ab9219a3d961de6923cea2cf9b8b99f58ca` |
| Ví fund/collateral | alice `addr_test1vzj38jr34pe6xcnwgdx9svpg5gnapavwda3jk0gdfqtex7qkewhel` |

**Bảng giao dịch** — explorer `https://preview.cardanoscan.io/transaction/<txid>`:

| Case | seq | Kỳ vọng | Kết quả on-chain | TxId |
|---|---|---|---|---|
| Seed | → 1 | UTxO anchor tại script addr | ✅ confirmed, datum `seq=1` | `5309c8d88d4f9b90d488b4300f74cba9c680d74f6846c992ae978bbe53e829e7#0` |
| **Happy** | 1 → 2 | submit OK (INV-E7 pass) | ✅ confirmed, datum `seq=2`; UTxO `seq=1` **đã tiêu** | `c57f55ec9df32e5c770f5480b475a4f51b602615bccecbed6ed6dbb4fc96a754#0` |
| **Reject** | 1 → 0 | **node reject** (rollback) | ✅ **node từ chối** — không lên chain | `9446a86bda5a09f099fe3deed284d200034c11267470bc7521f1be731effe078` |

**Bằng chứng reject (rollback 1→0):** node trả `ValidationTagMismatch (IsValid True) (FailedUnexpectedly (PlutusFailure "The PlutusV3 script failed… CekError… 'error'"))` cho `ScriptHash 1c7c7ca8…`. TxInfo xác nhận input datum `seq=1` → output datum `seq=0`, redeemer `<0>` → validator `error` tại `anchor.seq_advances` (0 ≠ 1+1). Tx `isValid=True` (mô phỏng kẻ tấn công cố neo lùi) → node từ chối ở phase-2, UTxO `seq=1` **không bị tiêu**.

**Ex-units happy path (đo bằng `calculate-plutus-script-cost online`):** memory `164,782` / steps `60,272,198` (nhẹ, cách xa trần). Driver tái lập lưu tại `~/vedata-node/strata-test3/` + `~/vedata-node/test3-spend-recreate.sh` (ngoài repo — như `anchor-cip68.sh`).

*(Nhánh replay `seq 1→1` và skip `seq 1→3` cùng đi qua guard `seq_advances` như rollback — đã phủ ở `aiken check`; on-chain chứng minh 1 đại diện `1→0` là đủ cho DoD test #3.)*

## 5. Còn lại (để đạt DoD đầy đủ)

- ~~Deploy validator lên Preview + spend-recreate THẬT~~ → ✅ **XONG 2026-07-06** (§4.1: happy `1→2` OK / reject `1→0` node từ chối, tx hash Preview thật).
- **Tx-builder Mosaic "proper" (TS Lucid/CSL)** trong VeData/Code thay PoC cardano-cli; `MosaicBackend` (Rust) gọi sang.
- **Aladin chốt** backend mặc định Settlement vs Mosaic (KHÔNG chặn phần đã làm).

---

## 6. Phụ lục — script PoC phía Mosaic (`~/vedata-node/`)

> Lưu vết (giống `boot-m6live.sh`). File thật ở `~/vedata-node/` (ngoài repo — tham chiếu path local + `credentials/`), **không** đưa vào code tree. PoC dùng cardano-cli thay Lucid; bản proper là tx-builder TS trong VeData/Code.
>
> - `anchor-cip68.sh` — neo CIP-68 tại ví + resolve (test #2, §3).
> - `test3-spend-recreate.sh` — deploy strata_anchor + test #3 spend-recreate on-chain (§4.1): seed seq=1 → happy 1→2 (OK) → reject 1→0 (node từ chối). Artifact tạm ở `~/vedata-node/strata-test3/` (`script.addr`, `strata_anchor.plutus`, datum/redeemer JSON, `*.raw/*.signed`, `reject-submit.log`).

```bash
#!/usr/bin/env bash
# anchor-cip68.sh — neo THẬT 1 Strata anchor lên L1 Preview dạng UTxO inline-datum CIP-68 + resolve.
# Dùng: anchor-cip68.sh <datum.json> <expected_mmr_root_hex> <expected_seq>
set -euo pipefail
cd "$(dirname "$0")"
CLI=./bin/bin/cardano-cli; MAGIC=2
ALICE=$(cat credentials/alice/alice-node.addr); SKEY=credentials/alice/alice-node.skey
export CARDANO_NODE_SOCKET_PATH="$PWD/data/node.socket"
DATUM="$1"; EXP_ROOT="$2"; EXP_SEQ="$3"; OUT_ADA=2000000
# [1] UTxO fuel alice
UTXO=$($CLI query utxo --address "$ALICE" --testnet-magic $MAGIC --out-file /dev/stdout \
  | jq -r 'to_entries|max_by(.value.value.lovelace)|.key')
# [2] build tx L1: output inline-datum CIP-68
$CLI conway transaction build --tx-in "$UTXO" \
  --tx-out "$ALICE+$OUT_ADA" --tx-out-inline-datum-file "$DATUM" \
  --change-address "$ALICE" --testnet-magic $MAGIC --out-file /tmp/anchor.raw
$CLI conway transaction sign --tx-body-file /tmp/anchor.raw --signing-key-file "$SKEY" \
  --testnet-magic $MAGIC --out-file /tmp/anchor.signed
TXID=$($CLI conway transaction txid --tx-file /tmp/anchor.signed | jq -r '.txhash // .')
# [3] submit L1 + chờ confirm
$CLI conway transaction submit --tx-file /tmp/anchor.signed --testnet-magic $MAGIC
for i in $(seq 1 50); do
  D=$($CLI query utxo --tx-in "$TXID#0" --testnet-magic $MAGIC --out-file /dev/stdout 2>/dev/null \
      | jq -c --arg k "$TXID#0" '.[$k].inlineDatum // empty'); [ -n "$D" ] && break; sleep 6
done
# [4] resolve + assert
GOT_ROOT=$(echo "$D" | jq -r '.fields[2].fields[2].bytes')
GOT_SEQ=$(echo "$D" | jq -r '.fields[2].fields[3].int')
[ "$GOT_ROOT" = "$EXP_ROOT" ] && [ "$GOT_SEQ" = "$EXP_SEQ" ] || { echo "resolve KHÔNG khớp"; exit 2; }
echo "RECEIPT: backend=mosaic-cip68 anchor_utxo=$TXID#0 seq=$GOT_SEQ finalized=true (L1 Preview)"
```

Emitter datum (Strata): `examples/emit_anchor_datum.rs` → `cargo run --example emit_anchor_datum` in `DATUM_JSON` + `REF_ID`/`HVH`/`MMR_ROOT`/`SEQ`.
