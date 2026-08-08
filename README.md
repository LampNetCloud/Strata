# Strata

**Tầng lưu trữ tiến hóa** của hệ MagicLamp — hồ sơ *"bất biến mà tiến hóa được"*: chuỗi phiên bản hash-link + MMR + `state_root` field-Merkle + anchor on-chain + audit-log. Module hạ tầng độc lập, dùng chung xuyên nền tảng (OriLife, ProofChat, AladinWork, VeData...) qua **Strata API**.

## Vì sao có Strata
Dữ liệu thật không chỉ là "file tĩnh". Hồ sơ KYC, timeline nông sản, tin nhắn sửa được, Jem việc làm, lớp phủ video... đều **tiến hóa theo thời gian** nhưng phải **chứng minh được lịch sử bất biến**. Strata cho phép: cập nhật = phiên bản mới (bản cũ giữ nguyên), chứng minh từng trường không lộ trường khác, neo bằng chứng on-chain, ghi nhật ký mọi truy cập.

## Bất biến (INV-E1..E9)
- **INV-E1** append-only, lịch sử bất biến.
- **INV-E5** CID không lộ loại nội dung (không mớm hacker).
- **INV-E6** field-proof từng trường (ghép thẳng ZK).
- **INV-E7** anchor chống rollback.
- `version_hash` KHÔNG gồm chữ ký; sig Ed25519 canonical low-S trên `version_hash`.

Chi tiết: [spec/_CONTRACT.md](spec/_CONTRACT.md), [spec/Strata-Feat.md](spec/Strata-Feat.md), [spec/Strata-Math.md](spec/Strata-Math.md), [spec/Strata-Tech.md](spec/Strata-Tech.md), [spec/Strata-API.md](spec/Strata-API.md).

## Tích hợp
- Phụ thuộc `lampnet-merkle-anchor` (primitive MMR + Merkle Sum Tree, dùng chung LampNet ∧ VeData) — hiện ở `lampnet-hivemind`, ghim qua git-rev.
- Tích hợp **in-process** (Rust link tĩnh, zero overhead) với Mirage (lưu byte → `content_cid`), Splash (compute), Carpet (truyền); chỉ ranh giới LampNet↔VeData mới qua API mạng.

## Test
```
cargo test
# 44 unit + 11 integration (gồm red-team E1/E4/E5/E6/E7)
```
\n## Ch?y CI & thi?t l?p m�i tru?ng (t�m t?t)\n\nN?u mu?n ch?y tuong t? CI ho?c gi�p reviewer k�ch ho?t workflow, c�c bu?c quan tr?ng:\n\n1) Pin toolchain: repo c� ust-toolchain.toml (1.96.0) � d�ng rustfmt/clippy tuong th�ch.\n2) N?u workspace tham chi?u crate git t? repo private LampNetCloud/Anchor, runner CI c?n quy?n d?c repo d�. Xem docs/CI-SETUP.md d? bi?t c�c phuong �n (secret ANCHOR_READ_TOKEN, deploy key, ho?c public repo).\n3) C�i Node.js & ch?y 
pm run test:fixture trong pis/ d? ki?m tra test-vector Rust?TS.\n\nV� d? l?nh local nhanh:\n\n`ash\nrustup toolchain install 1.96.0\ncargo fmt --all -- --check\ncargo clippy --all -- -D warnings\ncargo test --workspace\ncd apis && npm install && npm run test:fixture\n`\n\n
