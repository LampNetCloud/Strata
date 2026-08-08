Contributing to Strata

Cảm ơn bạn đã quan tâm đóng góp cho Strata! Dưới đây là các bước và quy ước để làm việc hiệu quả trong repo này.

1) Thiết lập môi trường

- Cài toolchain theo `rust-toolchain.toml` (repo dùng 1.96.0). Cài rustup và chạy:

  rustup toolchain install 1.96.0

- Cài Node.js (phiên bản 18+ khuyến nghị) nếu bạn muốn chạy test fixture JS trong `apis/`.

2) Kiểm tra trước khi PR

- Chạy formatter và lint: `cargo fmt --all -- --check` và `cargo clippy --all -- -D warnings`.
- Chạy test: `cargo test --workspace --all-features` và (nếu bạn làm phần JS) `cd apis && npm install && npm run test:fixture`.
- Nếu thay đổi proto/API, cập nhật `spec/` tương ứng.

3) CI & dependency private (Anchor)

- Lưu ý: workspace hiện tham chiếu crate `lampnet-merkle-anchor` từ repo `LampNetCloud/Anchor` (git+rev). Nếu repo Anchor private, CI runner cần quyền đọc repo đó.
- Các phương án để CI có thể fetch Anchor:
  * Thêm secret `ANCHOR_READ_TOKEN` (fine-grained PAT với quyền đọc repo Anchor) trên repo Strata.
  * Hoặc: tạo deploy key read-only trên repo Anchor rồi thêm private key làm secret `ANCHOR_DEPLOY_KEY` trên repo Strata.
  * Hoặc (chỉ khi chính sách cho phép): chuyển repo Anchor về public.
- Xem `docs/CI-SETUP.md` và `.github/workflows/ci-anchor.yml` (mẫu) để biết cách tích hợp.

4) Pull request

- Tạo branch tên rõ ràng (ví dụ `feat/xxx` hoặc `fix/xxx` hoặc `docs/xxx`).
- Mô tả rõ ràng trong PR: mục tiêu, thay đổi, các bước kiểm thử, rủi ro bảo mật (nếu có).
- Nếu PR can thiệp byte-layout hoặc spec, tag reviewer chịu trách nhiệm spec (ví dụ: @thinh hoặc @anhduc tùy theo lịch sử của file). Những thay đổi byte-layout cần thảo luận rõ với nhóm.

5) Secrets & bảo mật

- Không commit secrets vào repo. Thêm secrets qua Settings → Secrets → Actions.
- Token/key chỉ cấp quyền tối thiểu và nên đặt tên rõ ràng.

6) Muốn giúp bật CI cho PR liên quan Anchor (issue #24)

- Nếu bạn có quyền Admin: thêm secret `ANCHOR_READ_TOKEN` hoặc `ANCHOR_DEPLOY_KEY` theo hướng dẫn ở `docs/CI-SETUP.md`.
- Nếu không có quyền Admin: ping người có quyền (như admin team) kèm link tới PR #31 và issue #24.

Cảm ơn đóng góp của bạn! Nếu cần mẫu workflow hoặc hỗ trợ tạo secret, comment trong PR này và mình sẽ giúp chỉnh snippet cụ thể.
