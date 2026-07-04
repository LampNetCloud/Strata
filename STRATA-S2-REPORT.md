# Strata S2 — DerivedIndex / columnar query engine — Báo cáo

> **Issue:** `LampNetCloud/Strata#2` [P1] · **Spec:** `Strata-API.md §5.4 + §8.2` · **Cập nhật:** 2026-07-04
> **Trạng thái:** CODE-COMPLETE. Module `derived_index` + 7 test (6 unit + 1 benchmark), toàn repo **71 test pass / 0 warning**.

---

## 1. Phạm vi

S2 materialize field-index từ version-chain (derived, untrusted, rebuild-able) và trả `(value, proof)` cho một field ở một version/latest. Hai ràng buộc cứng từ spec:

- **Không primitive mới** — chỉ GHÉP `state::prove_field` (tầng trong) với `chain::prove_version` (tầng ngoài).
- **Cấm ghi vòng (INV §7.5)** — index CHỈ đọc log, không có đường chạm `Mmr::append`.

Tiền đề: build blocker gỡ sau PR #4 (`lampnet-merkle-anchor` → repo `LampNetCloud/Anchor`), `cargo build/test` chạy local.

---

## 2. Kết quả — module `src/derived_index.rs`

| Thành phần | Vai trò |
|---|---|
| `trait VersionLog` | View chỉ-đọc chuỗi version: `len` / `version(seq)` / `mmr_root` / `field_value_at(seq,key)` / `field_keys_at(seq)` |
| `enum Query` | `FieldEquals` · `FieldLatest` · `SenderRange{sender,from_ts,to_ts}` (index nóng Math §13) |
| `trait DerivedIndex` + `ColumnarIndex` | `replay(log)` tất định → cột `key→[(value,seq)]` + `sender→[(ts,seq)]`; `lookup(query)→Vec<Seq>` |
| `fn brute_force(log,query)` | Oracle full-scan để đối chiếu index |
| `struct CompositeFieldProof` + `fn verify_composite(root, p)` | **Proof hai tầng** (GAP cốt lõi S2) |
| `struct InMemoryVersionLog` | Log daemon: version đã ký + `state_fields` off-chain + MMR tái dựng; ràng buộc `build_state_root(fields)==version.state_root` khi nạp |

### 2.1 Proof hai tầng (§8.2)

`verify_composite(root, p)` nối ba điều kiện, `root` = `mmr_root` đã neo (tin cậy):
1. `verify_field_proof(field_proof)` — `(key,value) ∈ field_proof.state_root`;
2. `field_proof.state_root == version.state_root` — root khớp state_root đã băm vào `version_hash`;
3. `version.seq == seq` ∧ `verify_version(root, version_hash(version), seq, mmr_size, inclusion)` — version thuộc lịch sử đã neo, đúng vị trí.

`CompositeFieldProof` mang FULL version core để verifier tự tính `version_hash` và bind `state_root`. INV-E6 (field-privacy) giữ nguyên: mỗi tầng độc lập, sibling chỉ là hash — không lộ trường khác.

### 2.2 Ranh giới off-chain (SSoT vẫn là chain)

`StrataVersion` on-chain chỉ giữ `state_root` (không giá trị thô — INV-E5/E6). `InMemoryVersionLog` lưu kèm `state_fields` off-chain và TỪ CHỐI (`LogError::StateRootMismatch`) nếu fields không dựng ra đúng `state_root` đã ký — off-chain buộc trung thực với phần đã ký.

---

## 3. Test (tiêu chí §8.2)

| # | Test | Nội dung | KQ |
|---|---|---|---|
| 1 | `s2_query_field_at_version` | Query field ở mọi version k → ghép proof → verify về `mmr_root` | ✅ |
| 2 | `s2_oracle_vs_bruteforce` | `lookup` KHỚP full-scan (FieldEquals/FieldLatest/SenderRange) | ✅ |
| 3 | `s2_index_replay_root_bit_exact` | `replay(log)` tất định (2 lần == byte); index-root == log-root; state_root replay khớp bit | ✅ |
| 4 | `s2_field_privacy_preserved` | Proof field X tại version k không chứa key/value trường khác (xuyên version) | ✅ |
| 5 | `s2_benchmark_query_scaling` | Bảng query-time theo `n ∈ {1e2,1e3,1e4,1e5}` (số thật, `#[ignore]`) | ✅ |
| + | `composite_rejects_wrong_root_and_tamper` | Root lạ / đổi value / đổi seq → verify fail | ✅ |
| + | `log_rejects_offchain_state_root_mismatch` | Off-chain fields lệch state_root đã ký → push fail | ✅ |

### 3.1 Benchmark (release, µs)

| n | build_index | lookup_equals | lookup_latest | lookup_sender |
|---|---|---|---|---|
| 100 | 50 | 9 | 0 | 0 |
| 1 000 | 524 | 2 | 0 | 1 |
| 10 000 | 5 128 | 92 | 0 | 22 |
| 100 000 | 52 589 | 787 | 1 | 197 |

`build_index` tuyến tính O(số trường). `lookup_latest` ~O(1) (`.last()` của cột). `FieldEquals`/`SenderRange` O(n) quét cột. Ngưỡng bật columnar vs full-scan KHÔNG hard-code (theo đo thực, spec §8.2).

Chạy lại: `cargo test --release s2_benchmark -- --ignored --nocapture`.

---

## 4. Invariant / ranh giới tuân thủ

- **Cấm ghi vòng (§7.5):** `DerivedIndex`/`VersionLog` không có method chạm MMR. `brute_force` + `lookup` chỉ đọc.
- **INV-E6:** field-privacy giữ xuyên version (test #4).
- **Không đụng byte-layout:** S2 thuần view + ghép proof — độc lập điểm byte-layout anchor đang chờ chốt (không block/không bị block bởi S1).

---

## 5. Việc còn lại

- Ráp `InMemoryVersionLog` vào daemon thực (feed từ `StrataChain` + kho `state_fields`) — hiện là view in-memory.
- HTTP surface cho query (Strata-API §3) khi dựng daemon.
- Backend/Tech Lead sign-off.

---

## 6. Toàn cảnh test repo

`cargo test`: **71 pass / 0 fail / 0 warning** (59 unit gồm 6 S2 + 12 integration; 1 benchmark `#[ignore]`). `cargo clippy --all-targets`: 0 warning.
