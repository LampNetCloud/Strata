#!/usr/bin/env python3
"""Kiểm field-proof Strata ĐỘC LẬP — không dùng mã của crate Strata, không gọi daemon.

Đi trọn từ một giá trị trường tới thứ đã neo trên chuỗi:

    proof (value, salt, siblings)
      → fvh → leaf → state_root                     == proof.state_root
      → canonical_core(version, state_root TỪ PROOF)
      → version_hash                                == head_version_hash ĐÃ NEO (label 1234)

Dừng ở `state_root` là tin `state_root` do daemon khai; `version_hash` mà daemon trả kèm
version cũng là lời khai của daemon, nên script này KHÔNG dùng nó — nó tự tính lại rồi so với
giá trị bạn đọc từ chuỗi.

Luật băm (Strata-Math §3.1, §6; Strata#71):
    H_dom(tag, x)  = BLAKE3(UTF-8(tag) ‖ 0x00 ‖ x)
    fvh            = H_dom("LN/STRATA/state/fval/v1", value)                              salt rỗng
                   = H_dom("LN/STRATA/state/fval/salted/v1", u32_be(|salt|) ‖ salt ‖ value) salt khác rỗng
    leaf           = H_dom("LN/STRATA/state/leaf/v1", u32_be(|key|) ‖ key ‖ fvh)
    node           = H_dom("LN/STRATA/state/node/v1", left ‖ right)
    sibling        = [hash, sibling_is_right]; true ⇒ node(acc, hash), false ⇒ node(hash, acc)
    canonical_core = seq u64 ‖ prev_hash ‖ u32(|cid|) ‖ cid ‖ state_root ‖ author_did ‖ policy_hash ‖ ts u64
    version_hash   = H_dom("LN/STRATA/ver/v1", canonical_core)

Dùng:

    # 1. tự kiểm trên test-vector chung (CI chạy đúng lệnh này)
    python3 scripts/verify_field_proof.py --fixture apis/field-proof-vectors.json

    # 2. kiểm một proof thật
    curl -s $STRATA/v1/strata/$REF/proof/field/$KEY          > proof.json
    curl -s "$STRATA/v1/strata/$REF/version?at=$TS"          > version.json   # ts của version đó
    python3 scripts/verify_field_proof.py proof.json version.json \\
        --expect-version-hash <head_version_hash đọc từ record t=1 label 1234 của tx neo>

`version.json` nhận cả dạng `{"version": {...}}` (thân trả về của route) lẫn object version trần.
Không có giá trị neo thì phải khai `--no-chain`; khi đó kết quả chỉ nói proof nhất quán với
version đưa vào, KHÔNG nói gì về chuỗi.

Phụ thuộc: stdlib + `blake3` (`pip install blake3`). Thoát 0 = đạt, 1 = không đạt, 2 = dùng sai.
"""
from __future__ import annotations

import argparse
import json
import sys

TAG_FVAL = "LN/STRATA/state/fval/v1"
TAG_FVAL_SALTED = "LN/STRATA/state/fval/salted/v1"
TAG_LEAF = "LN/STRATA/state/leaf/v1"
TAG_NODE = "LN/STRATA/state/node/v1"
TAG_VER = "LN/STRATA/ver/v1"


def _blake3():
    try:
        import blake3  # noqa: PLC0415 — lười, để --help chạy được khi chưa cài
    except ImportError:
        sys.exit("thiếu thư viện `blake3` (stdlib Python không có BLAKE3): pip install blake3")
    return blake3.blake3


def h_dom(tag: str, x: bytes) -> bytes:
    return _blake3()(tag.encode("utf-8") + b"\x00" + x).digest()


def u32(n: int) -> bytes:
    return n.to_bytes(4, "big")


def u64(n: int) -> bytes:
    return n.to_bytes(8, "big")


def h32(s: str, what: str) -> bytes:
    b = bytes.fromhex(s)
    if len(b) != 32:
        raise ValueError(f"{what} phải đúng 32 byte, nhận {len(b)}")
    return b


def fvh_of(salt: bytes, value: bytes) -> bytes:
    """`salt` CHỌN MIỀN băm, không phải một đầu vào phụ (Strata#71)."""
    if not salt:
        return h_dom(TAG_FVAL, value)
    return h_dom(TAG_FVAL_SALTED, u32(len(salt)) + salt + value)


def root_from_proof(proof: dict) -> tuple[bytes, bytes]:
    """Trả (fvh tính lại, state_root dựng từ lá lên)."""
    key = proof["key"].encode("utf-8")   # dây: key là chuỗi UTF-8; value/salt/fvh/sibling là hex
    value = bytes.fromhex(proof["value"])
    salt = bytes.fromhex(proof.get("salt", ""))
    fvh = fvh_of(salt, value)
    acc = h_dom(TAG_LEAF, u32(len(key)) + key + fvh)
    for sib_hex, sib_is_right in proof["siblings"]:
        if not isinstance(sib_is_right, bool):
            raise ValueError("cờ chiều sibling phải là bool")
        sib = h32(sib_hex, "sibling")
        acc = h_dom(TAG_NODE, acc + sib) if sib_is_right else h_dom(TAG_NODE, sib + acc)
    return fvh, acc


def version_hash(version: dict, state_root: bytes) -> bytes:
    cid = bytes.fromhex(version["content_cid"])
    core = (u64(int(version["seq"])) + h32(version["prev_hash"], "prev_hash") + u32(len(cid)) + cid
            + state_root + h32(version["author_did"], "author_did")
            + h32(version["policy_hash"], "policy_hash") + u64(int(version["ts"])))
    return h_dom(TAG_VER, core)


def verify(proof: dict, version: dict, expect_vh: str | None) -> tuple[str, str]:
    """Trả (tầng hỏng đầu tiên | "dat", mô tả). Ba tầng, trùng `must_fail_at` của fixture và
    `node/tests/field_proof_fixture.rs`: fvh · state_root · version_hash.

    `version.state_root` KHÔNG phải một tầng: `version_hash` đã được tính với state_root dựng từ
    proof, nên nếu hai root khác nhau thì tầng `version_hash` hỏng. Thêm tầng riêng thì tên tầng
    lệch hợp đồng chung, và một bên kiểm hỏng vì lý do khác vẫn khớp phía còn lại."""
    fvh, root = root_from_proof(proof)
    if fvh.hex() != proof["fvh"].lower():
        return "fvh", f"fvh tính lại {fvh.hex()[:16]}… ≠ proof {proof['fvh'][:16]}…"
    if root.hex() != proof["state_root"].lower():
        return "state_root", f"state_root dựng lại {root.hex()[:16]}… ≠ proof {proof['state_root'][:16]}…"
    vh = version_hash(version, root)
    if expect_vh is not None and vh.hex() != expect_vh.lower():
        hint = ""
        if version.get("state_root", root.hex()).lower() != root.hex():
            hint = " — version đưa vào có state_root khác proof (proof của version khác?)"
        return "version_hash", f"version_hash {vh.hex()[:16]}… ≠ giá trị neo {expect_vh[:16]}…{hint}"
    return "dat", vh.hex()


def run_fixture(path: str) -> int:
    fx = json.load(open(path, encoding="utf-8"))
    bad = 0
    for v in fx["vectors"]:
        stage, info = verify(v["proof"], v["version"], v["version_hash"])
        ok = stage == "dat"
        bad += not ok
        print(f"  {'✅' if ok else '❌'} {v['name']:34s} {'đạt' if ok else 'HỎNG ở ' + stage + ': ' + info}")
    for r in fx["must_reject"]:
        stage, info = verify(r["proof"], r["version"], r["version_hash"])
        ok = stage == r["must_fail_at"]
        bad += not ok
        note = f"từ chối ở {stage}" if ok else f"kỳ vọng hỏng ở {r['must_fail_at']}, thực tế: {stage}"
        print(f"  {'✅' if ok else '❌'} {r['name']:34s} {note}")
    if len(fx["vectors"]) < 5 or len(fx["must_reject"]) < 9:
        print("  ❌ fixture bị rút bớt ca (tối thiểu 5 vector · 9 must_reject)")
        bad += 1
    print("✅ ĐẠT" if bad == 0 else f"❌ {bad} ca sai")
    return 0 if bad == 0 else 1


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("proof", nargs="?", help="JSON từ GET /v1/strata/:ref/proof/field/:key")
    ap.add_argument("version", nargs="?", help="JSON từ GET /v1/strata/:ref/version?at=<ts>")
    ap.add_argument("--expect-version-hash", help="head_version_hash đọc từ chuỗi (label 1234)")
    ap.add_argument("--no-chain", action="store_true", help="bỏ vế so với chuỗi — phải khai tường minh")
    ap.add_argument("--fixture", help="chạy test-vector chung apis/field-proof-vectors.json")
    a = ap.parse_args()

    if a.fixture:
        return run_fixture(a.fixture)
    if not (a.proof and a.version):
        ap.print_usage(sys.stderr)
        return 2
    if a.expect_version_hash is not None:
        e = a.expect_version_hash.strip().lower()
        if len(e) != 64 or any(c not in "0123456789abcdef" for c in e):
            # Kiểm TRƯỚC khi băm: giá trị cụt (vd chép từ một dòng in `[:16]…`) vẫn bị từ chối ở
            # tầng version_hash, nhưng thông báo khi đó in hai tiền tố giống hệt nhau kèm "≠" và
            # dẫn người đọc đi tìm lỗi băm trong khi lỗi là đầu vào.
            print(f"--expect-version-hash phải đúng 64 ký tự hex, nhận {len(e)}", file=sys.stderr)
            return 2
        a.expect_version_hash = e
    if a.expect_version_hash is None and not a.no_chain:
        print("thiếu --expect-version-hash: không có nó thì chỉ kiểm được proof khớp version đưa vào, "
              "không kiểm được gì về chuỗi. Khai --no-chain nếu đó đúng là điều bạn muốn.", file=sys.stderr)
        return 2

    try:
        proof = json.load(open(a.proof, encoding="utf-8"))
        version = json.load(open(a.version, encoding="utf-8"))
        version = version.get("version", version)
        if "version_seq" in proof and int(proof["version_seq"]) != int(version["seq"]):
            print(f"❌ proof thuộc seq {proof['version_seq']}, version đưa vào là seq {version['seq']}")
            return 1
        stage, info = verify(proof, version, a.expect_version_hash)
    except (KeyError, ValueError, TypeError, AttributeError) as err:
        # Đầu vào sai khuôn (thường là thân lỗi của daemon, vd {"error": "NotFound"}) KHÔNG phải
        # "proof không đạt" — tách mã thoát để không lẫn hai chuyện.
        print(f"đầu vào không đúng khuôn proof/version của route: {type(err).__name__}: {err}",
              file=sys.stderr)
        return 2
    if stage != "dat":
        print(f"❌ HỎNG ở {stage}: {info}")
        return 1
    scope = "khớp giá trị neo trên chuỗi" if a.expect_version_hash else "CHƯA so với chuỗi (--no-chain)"
    print(f"✅ ĐẠT — version_hash {info} · {scope}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
