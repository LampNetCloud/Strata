#!/usr/bin/env python3
"""Dựng lại `mmr_root` Strata ĐỘC LẬP — không dùng mã của crate Strata/Anchor, không gọi daemon.

`mmr_root` là một trong ba trường cam kết của anchor (label 1234: `ref_id ‖ head_version_hash ‖
mmr_root ‖ seq`). So `head_version_hash` với chuỗi chỉ chứng minh ĐỈNH đúng; `mmr_root` cam kết
TOÀN BỘ lịch sử version 0..seq. Script này dựng lại nó từ danh sách `version_hash` bạn tự có
(vd tự tính bằng `verify_field_proof.py`, hoặc từ request thô) rồi so với giá trị đọc từ chuỗi.

Luật (crate `lampnet-merkle-anchor` `src/mmr.rs`; `src/chain.rs`: lá MMR = `version_hash`):
    H_dom(tag, x) = BLAKE3(UTF-8(tag) ‖ 0x00 ‖ x)
    lá            = H_dom("LN/STRATA/mmr/leaf/v1", 0x00 ‖ version_hash)        seq 0..n-1
    nút           = H_dom("LN/STRATA/mmr/node/v1", 0x01 ‖ trái ‖ phải)
    đỉnh          = gốc cây con hoàn hảo theo khai triển nhị phân của n, lớn→nhỏ (không nhân đôi lá lẻ)
    bag           = fold-right: bag = p_m; j = m-1..1: bag = nút(p_j, bag)
    root          = H_dom("LN/STRATA/mmr/root/v1", u64_be(n) ‖ bag)            n = seq + 1

Dùng:

    # 1. tự kiểm trên test-vector chung (CI chạy đúng lệnh này)
    python3 scripts/verify_mmr_root.py --fixture apis/mmr-root-vectors.json

    # 2. kiểm một anchor thật: version_hash seq 0..seq, mỗi dòng một hex (hoặc một mảng JSON)
    python3 scripts/verify_mmr_root.py version_hashes.txt \\
        --expect-mmr-root <mmr_root đọc từ record t=1 label 1234 của tx neo>

Ở chế độ 2, `n` = số dòng đưa vào; anchor ở `seq` thì phải đưa đúng `seq + 1` version_hash.

Phụ thuộc: stdlib + `blake3` (`pip install blake3`). Thoát 0 = đạt, 1 = không đạt, 2 = dùng sai.
"""
from __future__ import annotations

import argparse
import json
import re
import sys

TAG_LEAF = "LN/STRATA/mmr/leaf/v1"
TAG_NODE = "LN/STRATA/mmr/node/v1"
TAG_ROOT = "LN/STRATA/mmr/root/v1"
TAG_STATE_NODE = "LN/STRATA/state/node/v1"   # chỉ dùng cho ca âm "nhầm miền"
HEX64 = re.compile(r"^[0-9a-f]{64}$")


def _blake3():
    try:
        import blake3  # noqa: PLC0415 — lười, để --help chạy được khi chưa cài
    except ImportError:
        sys.exit("thiếu thư viện `blake3` (stdlib Python không có BLAKE3): pip install blake3")
    return blake3.blake3


def h_dom(tag: str, x: bytes) -> bytes:
    return _blake3()(tag.encode() + b"\x00" + x).digest()


def mmr_root(leaves: list[bytes], *, leaf_prefix=b"\x00", node_prefix=b"\x01", node_tag=TAG_NODE,
             dup_odd=False, peaks_small_first=False, bag_left=False, commit_n=True,
             n_le=False) -> bytes:
    """Luật đúng khi mọi tham số để mặc định; mỗi tham số khác mặc định đổi đúng MỘT vế luật."""
    lh = [h_dom(TAG_LEAF, leaf_prefix + v) for v in leaves]

    def node(a: bytes, b: bytes) -> bytes:
        return h_dom(node_tag, node_prefix + a + b)

    n = len(lh)
    if dup_odd:                      # MỘT cây, nhân đôi lá lẻ (kiểu CVE-2012-2459) — sai
        lv = lh[:]
        while len(lv) > 1:
            if len(lv) % 2:
                lv.append(lv[-1])
            lv = [node(lv[i], lv[i + 1]) for i in range(0, len(lv), 2)]
        peaks = lv
    else:
        peaks, off = [], 0
        for bit in range(63, -1, -1):
            s = 1 << bit
            if n & s:
                lv = lh[off:off + s]
                while len(lv) > 1:
                    lv = [node(lv[i], lv[i + 1]) for i in range(0, len(lv), 2)]
                peaks.append(lv[0])
                off += s
    if peaks_small_first:
        peaks = peaks[::-1]
    if bag_left:
        bag = peaks[0]
        for p in peaks[1:]:
            bag = node(bag, p)
    else:
        bag = peaks[-1]
        for p in reversed(peaks[:-1]):
            bag = node(p, bag)
    nb = n.to_bytes(8, "little" if n_le else "big")
    return h_dom(TAG_ROOT, (nb if commit_n else b"") + bag)


# Mỗi ca âm: (tên, kwargs, điều kiện áp dụng theo n). Điều kiện là chỗ hai luật TRÙNG kết quả
# về mặt toán — ví dụ 2 đỉnh thì fold trái = fold phải — không phải chỗ bỏ qua cho tiện.
NEGATIVES = [
    ("lá thiếu tiền tố 0x00", dict(leaf_prefix=b""), lambda n: True),
    ("nút thiếu tiền tố 0x01", dict(node_prefix=b""), lambda n: n > 1),
    ("nút dùng tag state/node", dict(node_tag=TAG_STATE_NODE), lambda n: n > 1),
    ("root không commit n", dict(commit_n=False), lambda n: True),
    ("commit n little-endian", dict(n_le=True), lambda n: True),
    ("đỉnh nhỏ→lớn", dict(peaks_small_first=True), lambda n: bin(n).count("1") >= 2),
    ("một cây nhân đôi lá lẻ", dict(dup_odd=True), lambda n: bin(n).count("1") >= 2),
    ("bag fold-left", dict(bag_left=True), lambda n: bin(n).count("1") >= 3),
]


def _unhex32(s: str, where: str) -> bytes:
    s = s.strip().lower()
    if s.startswith("0x"):
        s = s[2:]
    if not HEX64.match(s):
        print(f"❌ {where}: không phải 64 hex: {s!r}", file=sys.stderr)
        sys.exit(2)
    return bytes.fromhex(s)


def run_fixture(path: str) -> int:
    fx = json.load(open(path, encoding="utf-8"))
    vecs = fx["vectors"]
    bad = 0
    bag_covered = False
    for v in vecs:
        leaves = [_unhex32(h, f"{v['name']}.version_hashes") for h in v["version_hashes"]]
        want = _unhex32(v["mmr_root"], f"{v['name']}.mmr_root")
        if len(leaves) != v["n"]:
            print(f"❌ {v['name']}: n={v['n']} nhưng {len(leaves)} version_hash")
            bad += 1
            continue
        got = mmr_root(leaves)
        ok = got == want
        bad += not ok
        line = [f"{'✅' if ok else '❌'} {v['name']:4s} n={v['n']:2d} đỉnh={bin(v['n']).count('1')}"]
        rej = 0
        for name, kw, applies in NEGATIVES:
            if not applies(v["n"]):
                continue
            if mmr_root(leaves, **kw) == want:
                bad += 1
                line.append(f"\n     ❌ ca âm KHÔNG lệch: {name}")
            else:
                rej += 1
                bag_covered |= name == "bag fold-left"
        if v["n"] > 1:
            if mmr_root(leaves[::-1]) == want:
                bad += 1
                line.append("\n     ❌ ca âm KHÔNG lệch: đảo thứ tự lá")
            else:
                rej += 1
        line.insert(1, f" · {rej} ca âm lệch")
        print("".join(line))
    if not bag_covered:
        # Không có vector nào ≥ 3 đỉnh ⇒ chiều bag chưa từng được kiểm, dù mọi dòng trên xanh.
        print("❌ fixture không có vector nào ≥ 3 đỉnh — chiều bag chưa được kiểm")
        bad += 1
    print(f"\n{'✅ ĐẠT' if bad == 0 else f'❌ {bad} chỗ sai'} — {len(vecs)} vector")
    return 1 if bad else 0


def run_one(path: str, expect: str) -> int:
    raw = open(path, encoding="utf-8").read().strip()
    items = json.loads(raw) if raw.startswith("[") else [l for l in raw.splitlines() if l.strip()]
    leaves = [_unhex32(h, f"dòng {i + 1}") for i, h in enumerate(items)]
    if not leaves:
        print("❌ không có version_hash nào", file=sys.stderr)
        return 2
    want = _unhex32(expect, "--expect-mmr-root")
    got = mmr_root(leaves)
    n = len(leaves)
    print(f"n = {n} (anchor seq {n - 1}) · {bin(n).count('1')} đỉnh")
    print(f"  dựng lại   {got.hex()}")
    print(f"  trên chuỗi {want.hex()}")
    print("✅ KHỚP" if got == want else "❌ LỆCH")
    return 0 if got == want else 1


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("version_hashes", nargs="?", help="tệp version_hash seq 0..seq")
    ap.add_argument("--fixture", help="apis/mmr-root-vectors.json")
    ap.add_argument("--expect-mmr-root", help="mmr_root đọc từ label 1234")
    a = ap.parse_args()
    if a.fixture:
        return run_fixture(a.fixture)
    if not a.version_hashes or not a.expect_mmr_root:
        ap.print_usage(sys.stderr)
        print("cần <version_hashes> và --expect-mmr-root (hoặc --fixture)", file=sys.stderr)
        return 2
    return run_one(a.version_hashes, a.expect_mmr_root)


if __name__ == "__main__":
    sys.exit(main())
