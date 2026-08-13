/**
 * Đối chiếu TS ↔ Rust trên test-vector CHUNG `apis/settlement-metadata.json`.
 *
 * File vector do Rust sinh (`cargo run --example dump_settlement_fixture`) vì Rust giữ
 * **decoder** — bên phải khớp thì không nên tự ra đề. Bên Rust có
 * `tests/settlement_fixture.rs` đọc đúng file này.
 *
 * Bắt được gì: luật chunk 64B lệch (ca 64B cấm chunk / 65B phải 64+1), thứ tự trường
 * anchor, discriminator `t`, và kiểu của `seq`. Đây là những chỗ hai bản cài đặt trôi
 * khỏi nhau mà không lệnh build nào phát hiện.
 */

import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

import { buildMetadata } from "../submit.js";

/** `a[i]` = `string` (một bytestring trần) | `string[]` (mảng chunk) | `number` (seq). */
type Field = string | string[] | number;
type FixtureCase = {
  name: string;
  records: any[];
  expected_structure: { t: number; a: Field[] }[];
};

const here = dirname(fileURLToPath(import.meta.url));
const fixturePath = resolve(here, "../../../apis/settlement-metadata.json");
const fixture = JSON.parse(readFileSync(fixturePath, "utf-8")) as {
  label: number;
  cases: FixtureCase[];
  must_reject: { name: string; why: string; expect_error: string; cbor_hex: string }[];
  must_skip: { name: string; why: string; expect_records: number; cbor_hex: string }[];
};

const toHex = (b: Uint8Array): string =>
  Array.from(b, (x) => x.toString(16).padStart(2, "0")).join("");

/**
 * Chuẩn hoá metadatum TS về đúng hình dạng `expected_structure` của vector.
 *
 * QUAN TRỌNG — giữ nguyên phân biệt **bytestring trần** (`Uint8Array` → `string`) và
 * **mảng chunk** (`Uint8Array[]` → `string[]`). Bản đầu của test này gộp cả hai thành
 * `[hex]`, và vì thế nó **không bắt được** lỗi chunk sai: decoder Rust từ chối
 * "≤64B mà lại chunk" (chống malleability), nên đó đúng là lỗi phải bắt.
 */
function normalize(meta: unknown[]): { t: number; a: Field[] }[] {
  return meta.map((rec) => {
    const r = rec as { t: number; a: unknown[] };
    return {
      t: r.t,
      a: r.a.map((field): Field => {
        if (typeof field === "number") return field;
        if (field instanceof Uint8Array) return toHex(field);
        if (Array.isArray(field)) return field.map((c) => toHex(c as Uint8Array));
        throw new Error(`trường lạ trong metadatum: ${typeof field}`);
      }),
    };
  });
}

let failed = 0;
assert.equal(fixture.label, 1234, "label vector phải là 1234");
assert.ok(fixture.cases.length > 0, "vector rỗng thì test này vô nghĩa");

for (const c of fixture.cases) {
  try {
    assert.deepEqual(
      normalize(buildMetadata(c.records)),
      c.expected_structure,
      `case \`${c.name}\`: metadatum TS lệch vector chung`
    );
    console.log(`  ok  ${c.name}`);
  } catch (e) {
    failed++;
    console.error(`  FAIL ${c.name}\n${e instanceof Error ? e.message : String(e)}`);
  }
}

// Ca biên phải có mặt — nếu ai rút gọn vector thì kêu, giống test bên Rust.
for (const must of ["rotation_64B_boundary_single", "rotation_65B_chunked_64_1"]) {
  if (!fixture.cases.some((c) => c.name === must)) {
    failed++;
    console.error(`  FAIL vector thiếu ca biên \`${must}\``);
  }
}

// ── Nửa ÂM của fixture (PR #40 hướng (b)) ────────────────────────────────────
//
// `submit.ts` hiện là ENCODER-ONLY: nó dựng metadatum, không có đường decode CBOR nào.
// Nên khối `must_reject` KHÔNG cưỡng chế được ở phía TS hôm nay — nó đang được cưỡng chế
// bên Rust (`tests/settlement_fixture.rs::must_reject_thi_decoder_phai_tu_choi`).
//
// Vẫn kiểm ở đây hai điều, để cái gap đó KHÔNG vô hình:
//   1. khối phải tồn tại — ai rút nó khỏi fixture thì cả hai phía cùng đỏ, không phải
//      chỉ Rust đỏ còn TS im lặng đi tiếp;
//   2. in ra lời nhắc, để ngày `submit.ts` có đường đọc (ví dụ beacon-walk resolve đọc
//      metadatum thật) thì người viết thấy ngay là có sẵn bộ ca âm phải dùng.
const MIN_MUST_REJECT = 6;
if (!Array.isArray(fixture.must_reject) || fixture.must_reject.length < MIN_MUST_REJECT) {
  failed++;
  console.error(
    `  FAIL fixture thiếu khối must_reject (cần >= ${MIN_MUST_REJECT} ca âm) — ` +
      `không có nó thì vector chỉ là nguồn sự thật cho nửa dương`,
  );
}
if (!Array.isArray(fixture.must_skip) || fixture.must_skip.length < 1) {
  failed++;
  console.error("  FAIL fixture thiếu khối must_skip (`t` lạ phải bỏ qua, không ném lỗi)");
}

if (failed > 0) {
  console.error(`\n${failed} case lệch — TS và Rust đã trôi khỏi nhau.`);
  process.exit(1);
}
console.log(`\n${fixture.cases.length}/${fixture.cases.length} case khớp vector chung.`);
console.log(
  `${fixture.must_reject.length} ca âm + ${fixture.must_skip.length} ca bỏ-qua có mặt trong vector ` +
    `(cưỡng chế phía Rust — submit.ts chưa có đường đọc CBOR).`,
);
