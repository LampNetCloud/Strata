#!/usr/bin/env python3
"""Bắt tay OriLife-Core ↔ strata-node: kiểm cặp did:pubkey, rồi so BYTE qua `_canonical`.

Chạy được bằng Python 3.9+ **stdlib thuần** — không cần cài gì. Đặt ở kho Strata vì
`_canonical` là route của daemon này; bên chạy nó là đội tích hợp.

    python3 orilife_handshake.py --did did:phoenix:… --pubkey <hex64> \
        [--url http://127.0.0.1:6690] [--canonical-core <hex bản mình dựng>]

Ba bước, dừng ở bước đầu tiên hỏng — vì bước sau không có nghĩa nếu bước trước sai:

  1. KHUÔN DID. Khuôn PhoenixKey cấp là `^did:phoenix:[a-z2-7]{13}:[0-9a-f]{64}$`. Trong
     khuôn đó, NFC/NFKC/NFD/NFKD và hạ-thường đều là NO-OP, còn `%` và `#` không lọt được
     — nên bốn câu chuẩn-hoá còn treo KHÔNG QUAN SÁT ĐƯỢC ở đây. Ngoài khuôn thì chúng
     quan sát được ngay, và `author_did` là hàm một chiều nên sai là không có đường lui.
     ⇒ Cổng khuôn thay cho một quyết định chuẩn-hoá chưa chốt.

  2. DẪN XUẤT `author_did = blake2b_256(UTF-8(did))` — không salt, không domain-tag.
     In ra để hai bên so. Đây là thứ đi vào `STRATA_NODE_KEYS`, không phải chuỗi DID.

  3. `POST /_canonical` — so BYTE `canonical_core` daemon dựng với bản bên gọi tự dựng.
     Đây là đường đối chiếu TRƯỚC KHI KÝ. Không có nó thì lệch một bit ⇒ `403 BadSignature`,
     một thông điệp không hề nhắc tới `state_root`, và bên tích hợp ngồi đoán.

     ĐÃ ĐO 2026-08-27: route này **không** tra key-registry — daemon với 0 khoá vẫn trả
     `200`. Nên bước 3 chạy được TRƯỚC khi bảng `did:pubkey` trao xong. Hai nửa của lượt
     bàn giao độc lập nhau; đừng xếp chúng nối đuôi.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import unicodedata
import urllib.error
import urllib.request

# Khuôn do PhoenixKey cấp (thư 2026-07-30, OriLife-Core#159).
DID_SHAPE = re.compile(r"^did:phoenix:[a-z2-7]{13}:[0-9a-f]{64}$")
NORM_FORMS = ("NFC", "NFKC", "NFD", "NFKD")
ZERO32 = "00" * 32


def step1_shape(did: str) -> list[str]:
    """Trả danh sách cảnh báo. Rỗng = đúng khuôn và mọi chuẩn-hoá là NO-OP."""
    warn: list[str] = []
    if not DID_SHAPE.match(did):
        warn.append(
            "DID KHÔNG khớp `^did:phoenix:[a-z2-7]{13}:[0-9a-f]{64}$`. Ngoài khuôn này thì "
            "hoa/thường · #fragment · %-encoding · NFC ĐỀU quan sát được, và cả bốn đang là "
            "câu hỏi CHƯA CHỐT với PhoenixKey. Đừng ghi lineage bằng DID này."
        )
    base = did.encode("utf-8")
    for f in NORM_FORMS:
        if unicodedata.normalize(f, did).encode("utf-8") != base:
            warn.append(f"chuẩn-hoá {f} ĐỔI byte của DID ⇒ author_did phụ thuộc dạng Unicode")
    if did.lower().encode("utf-8") != base:
        warn.append("DID có chữ HOA ⇒ hạ-thường đổi author_did; PhoenixKey chưa chốt case")
    return warn


def step2_author_did(did: str) -> str:
    return hashlib.blake2b(did.encode("utf-8"), digest_size=32).hexdigest()


def step3_canonical(url: str, author_did_hex: str, *, ts: int, nonce_hex: str) -> dict:
    body = {
        "seq": 0,
        "prev_hash": ZERO32,
        "content_cid": "",
        "state_fields": [],
        "author_did": author_did_hex,
        "policy_hash": ZERO32,
        "ts": ts,
        "genesis_nonce": nonce_hex,
    }
    req = urllib.request.Request(
        url.rstrip("/") + "/v1/strata/_canonical",
        data=json.dumps(body).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=10) as r:
        return json.loads(r.read().decode("utf-8"))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--did", required=True, help="chuỗi DID PhoenixKey, KHÔNG phải băm")
    ap.add_argument("--pubkey", required=True, help="Ed25519 public key, hex 64 ký tự")
    ap.add_argument("--url", default="http://127.0.0.1:6690")
    ap.add_argument("--ts", type=int, default=1_756_000_000, help="GIÂY, không phải mili giây")
    ap.add_argument("--nonce", default="11" * 32, help="genesis_nonce hex 32B")
    ap.add_argument("--canonical-core", default=None, help="hex bản bên gọi tự dựng, để so BYTE")
    ap.add_argument("--skip-daemon", action="store_true", help="chỉ chạy bước 1+2")
    a = ap.parse_args()

    print("── Bước 1: khuôn DID " + "─" * 47)
    warns = step1_shape(a.did)
    for w in warns:
        print("  ⚠️  " + w)
    if not warns:
        print("  ✅ đúng khuôn; NFC/NFKC/NFD/NFKD + hạ-thường đều NO-OP; '%' và '#' không lọt")

    print("\n── Bước 2: dẫn xuất author_did " + "─" * 38)
    pk = a.pubkey.strip().lower()
    if len(pk) != 64 or any(c not in "0123456789abcdef" for c in pk):
        print("  ❌ pubkey phải là hex ĐÚNG 64 ký tự (32 byte Ed25519)")
        return 2
    did_hex = step2_author_did(a.did)
    print(f"  did          : {a.did}")
    print(f"  author_did   : {did_hex}")
    print(f"  pubkey       : {pk}")
    print("\n  Dòng cho STRATA_NODE_KEYS phía VeData (did_hex32:pubkey_hex32):")
    print(f"    {did_hex}:{pk}")
    print("  ⚠️  Thêm khoá vào registry đòi KHỞI ĐỘNG LẠI daemon — registry nạp lúc lên.")

    if a.skip_daemon:
        return 1 if warns else 0

    print("\n── Bước 3: so BYTE qua _canonical " + "─" * 35)
    try:
        resp = step3_canonical(a.url, did_hex, ts=a.ts, nonce_hex=a.nonce)
    except urllib.error.HTTPError as e:
        detail = e.read().decode("utf-8", "replace")[:400]
        print(f"  ❌ HTTP {e.code}: {detail}")
        if e.code == 424:
            print("     424 UnknownAuthor = author_did chưa có trong registry.")
            print("     Mã này KHÔNG nói gì về khoá — nó chỉ nói 'chưa tra được chủ'.")
            print("     Bất thường ở ĐÂY: `_canonical` KHÔNG tra registry (đo 2026-08-27,")
            print("     daemon 0 khoá vẫn trả 200). Gặp 424 ở bước này ⇒ daemon đã đổi hành vi.")
        if e.code == 422:
            print("     422 — nghi can đầu tiên là `ts` gửi bằng MILI GIÂY. Phải là GIÂY.")
        return 2
    except urllib.error.URLError as e:
        print(f"  ❌ không nối được {a.url}: {e.reason}")
        return 2

    for k in ("canonical_core", "version_hash", "state_root", "ref_id"):
        print(f"  {k:<14}: {resp.get(k)}")

    if a.canonical_core is None:
        print("\n  (chưa truyền --canonical-core nên chưa so được byte nào —")
        print("   dựng canonical_core bằng encoder của bên mình rồi chạy lại kèm cờ đó)")
        return 1 if warns else 0

    mine = a.canonical_core.strip().lower().replace(" ", "")
    theirs = str(resp["canonical_core"]).lower()
    if mine == theirs:
        print("\n  ✅ canonical_core TRÙNG từng byte — đường ký đã khớp")
        return 1 if warns else 0

    print("\n  ❌ canonical_core LỆCH")
    print(f"     dài: bên gọi {len(mine)//2}B · daemon {len(theirs)//2}B")
    n = min(len(mine), len(theirs))
    i = next((j for j in range(n) if mine[j] != theirs[j]), n)
    print(f"     byte lệch ĐẦU TIÊN: offset {i//2}")
    lo, hi = max(0, i - 16), min(n, i + 16)
    print(f"     bên gọi …{mine[lo:hi]}…")
    print(f"     daemon  …{theirs[lo:hi]}…")
    print("     Layout TLV, KHÔNG phải CBOR. `sig` KHÔNG nằm trong canonical_core (CHỐT-1).")
    print("     Lệch ở giữa thân thường là `state_root` — so trường state_root ở trên trước.")
    return 2


if __name__ == "__main__":
    sys.exit(main())
