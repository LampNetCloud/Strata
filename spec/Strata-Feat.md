# Strata — Tính năng (bản phổ thông)

**Module**: Strata (Evolving Content Record — "Hồ sơ Tiến hóa") — một primitive metadata cho dữ liệu **tiến hóa được nhưng không sửa được** trên LampNet

> Bản phổ thông cho mọi người đọc (tech + nontech). Toán chi tiết (cây băm, MMR, proof) ở **Strata-Math.md**; cài đặt thành code ở **Strata-Tech.md**.

---

## Tóm tắt

Hệ thống lưu trữ LampNet hiện chỉ phục vụ tốt **dữ liệu tĩnh**: một tệp được tải lên, lấy một định danh, và không bao giờ đổi nữa — video, ảnh, PDF, bản phát hành. Đó là một phần nhỏ của thế giới thật. Phần lớn dữ liệu **thay đổi theo thời gian**: chuỗi commit của một dự án mã nguồn, dòng nhật ký thiết bị IoT, giá tài sản, số dư ví, học bạ, sổ bệnh. Với những thứ này, hệ thống hiện tại không có cách nào để **thêm tiếp** (append), **đăng một giá trị mới** (register), hay **cập nhật từng phần một hồ sơ có cấu trúc** mà vẫn giữ được lịch sử kiểm chứng được.

Strata là một primitive duy nhất lấp khoảng trống đó. Nó cho mỗi đối tượng một **định danh ổn định** (`ref_id`) không bao giờ đổi qua các phiên bản, và sau định danh đó là một **chuỗi phiên bản** liên kết bằng băm (hash-linked). Thêm một phiên bản chỉ **mở rộng** lịch sử, không bao giờ ghi đè hay xóa cái cũ. Toàn bộ lịch sử được tóm vào một **cam kết lịch sử** (`mmr_root`) chỉ 32 byte; cả `anchor` neo lên Cardano gồm 4 trường (`ref_id` + `head_version_hash` + `mmr_root` + `seq`) tổng 104 byte, nên không ai tua lùi hay sửa lén được. Triết lý gốc: **tiến hóa được nhưng không sửa được** — nội dung có thể lớn lên theo thời gian, nhưng mỗi trạng thái quá khứ vẫn bất biến và chứng minh được.

Một primitive Strata phục vụ **cả bốn loại** dữ liệu (Tĩnh / Chuỗi-thêm / Thanh-ghi / Hồ sơ cấu trúc) bằng cùng một bộ máy, thay vì bốn cơ chế rời rạc. Strata là **tầng metadata nằm TRÊN Mirage**: nó không tự lưu blob, không tự mã hóa — việc lưu nội dung, bảo mật và tái phân tán giao hết cho Mirage; Strata chỉ giữ định danh, chuỗi phiên bản, cam kết băm và neo on-chain.

---

## §0.5 Phạm vi module

Strata là module đồng cấp với Mirage / Splash / Reward / Join trong tầng LampNet, nhưng đứng ở **lớp metadata phía trên lưu trữ**. Strata không tự đặt byte xuống đĩa, không tự mã hóa, không tự sửa lỗi dữ liệu — nó **mô tả và cam kết** mối quan hệ giữa một định danh ổn định và chuỗi nội dung của nó qua thời gian, rồi giao việc lưu thật cho Mirage và việc neo finality cho Cardano.

| Lằn ranh | Nội dung |
|---|---|
| **Làm** | Cấp **định danh ổn định** `ref_id` (không đổi qua phiên bản); dựng **chuỗi phiên bản** hash-linked (`version`, `version_hash`); duy trì **history accumulator** (`mmr_root` trên dãy `version_hash`); neo **on-chain commitment** (`anchor` = 4 trường); sinh **field-level proof** từ `state_root` (chứng minh một trường mà giấu trường khác) |
| **Kế thừa** | Khung 4 loại dữ liệu MECE và bộ invariant `INV-E1..INV-E9` (xem `_CONTRACT.md`); chuẩn băm dùng chung BLAKE3 + domain-separation + RFC 6962 + dup-leaf guard |
| **Dùng** | **Mirage** — lưu `content_cid` của mỗi phiên bản theo lớp bảo mật, bảo mật (mã hóa) + tái phân tán + repair; **PhoenixKey** — `author_did` của người tạo/người sửa + chữ ký Ed25519; **Cardano** — nơi neo `anchor` để có finality kinh tế và chống rollback |
| **Giao việc** | Lưu blob nội dung → Mirage; mã hóa nội dung nhạy cảm + tái phân tán → Mirage; danh tính + chữ ký → PhoenixKey; finality / chống tua lùi → Cardano. Strata **KHÔNG** tự lưu blob, **KHÔNG** tự mã hóa. |

Strata khác Reward ở chỗ Reward tính tiền từ bằng chứng đã xác minh; Strata thì **mô tả vòng đời của một đối tượng dữ liệu**. Cả hai cùng dựa nguyên tắc: chỉ cam kết những gì kiểm chứng được, và đẩy phần nặng (lưu, đo) cho module chuyên trách.

---

## §1. Bài toán & bốn loại dữ liệu (khung MECE)

Dữ liệu trên một mạng lưu trữ phân biệt nhau ở **quan hệ giữa định danh và nội dung qua thời gian**: một định danh trỏ tới một nội dung cố định, hay tới một thứ đang lớn lên, hay tới giá trị mới nhất, hay tới một hồ sơ nhiều mảnh? Bốn câu trả lời đó chia hết và không chồng lấn (MECE) thành bốn loại. Hệ thống hiện tại chỉ phục vụ loại thứ nhất.

| Loại | Ví dụ thật | Ngữ nghĩa đọc | Strata phục vụ thế nào |
|---|---|---|---|
| **1. Tĩnh** (write-once) | Video OriLife, ảnh, PDF, bản phát hành phần mềm | "Cho tôi đúng nội dung đó" — một định danh ↔ một nội dung cố định | Strata là **một version duy nhất** (`seq = 0`). Đọc `content_cid` của version đó. |
| **2. Chuỗi-thêm** (append-only) | Chuỗi commit Gitlamp, dòng nhật ký IoT, bình luận, sự kiện like/share | "Cho tôi toàn bộ chuỗi, theo thứ tự, không thiếu mục" — chỉ thêm cuối, mục cũ bất biến | **MMR chính là cái log**: mỗi mục là một leaf `version_hash`; thêm mục = mở rộng cây, các proof cũ vẫn đúng (INV-E3). |
| **3. Thanh-ghi** (mutable register) | Giá tài sản, số dư ví, nhiệt độ hiện tại | "Cho tôi **giá trị mới nhất**" — ghi đè; lịch sử chỉ để audit | Đọc **head** (version `seq` lớn nhất). Lịch sử ghi đè vẫn nằm trong MMR để kiểm tra sau. |
| **4. Hồ sơ cấu trúc** (structured evolving) | Học bạ, sổ bệnh, hồ sơ DID | "Cho tôi **một trường nhất định**, đúng quyền" — nhiều trường, cập nhật từng phần | `state_root` cam kết từng trường riêng; cập nhật một trường sinh một version mới; **field-level proof** chứng minh một trường mà không lộ trường khác (INV-E6). |

**Ghi chú bắt buộc — đếm view/like không phải loại thứ năm.** Đếm số lượt xem hay lượt thích trông giống "một con số đang đổi", dễ lầm là một loại riêng. Thực ra đó là **register (#3) materialize ra từ một append-log (#2)**: mỗi lượt xem/thích là một mục thêm vào chuỗi (#2, bất biến, kiểm chứng được từng lượt), còn "tổng số" chỉ là giá trị head được tính lại (#3) từ chuỗi đó. Strata không cần cơ chế mới cho việc đếm — nó là tổ hợp của hai loại đã có.

**Một primitive, bốn loại.** Đây là điểm cốt lõi: Strata không phải bốn công cụ, mà **một** bộ máy (định danh ổn định + chuỗi version hash-linked + MMR + state_root + policy) phủ cả bốn. Loại #1 là Strata có đúng một version; #2 là đọc cả MMR như một log; #3 là đọc head; #4 là dùng `state_root` cam kết từng trường cộng `policy_hash` phân quyền sửa. Nhờ vậy hệ thống chỉ phải làm đúng **một** thứ thật chắc, thay vì bốn thứ na ná nhau mà cái nào cũng dễ sai ở chỗ băm và proof.

---

## §2. Người dùng làm được gì

Strata mở ra năm khả năng mà hệ thống dữ liệu tĩnh hiện nay không có:

- **Tạo một Strata.** Người dùng tạo một đối tượng tiến hóa và nhận về một `ref_id` ổn định (dạng `lnref1…`). Định danh này dùng để tham chiếu mãi mãi, kể cả khi nội dung sẽ thay đổi nhiều lần sau này. Nội dung phiên bản đầu được giao cho Mirage lưu; Strata chỉ giữ `content_cid` trỏ tới nó.

- **Cập nhật (thêm một phiên bản).** Khi nội dung tiến hóa — một commit mới, một giá mới, một trường học bạ được điền — người dùng (hoặc bên có quyền theo `policy_hash`) thêm một `version` mới vào chuỗi. Phiên bản mới liên kết vào phiên bản cũ bằng băm (`prev_hash`), `seq` tăng đúng +1 (INV-E2), và lịch sử cũ **không hề bị động tới**. Đây là nghĩa của "tiến hóa được nhưng không sửa được".

- **Chứng minh "giá trị tại thời điểm t".** Người dùng có thể chứng minh một cách kiểm chứng được rằng "tại thời điểm `t`, giá trị là X" — ví dụ "số dư ví lúc 0h ngày 1/1 là 5.000 LAMP" hay "giá tài sản tại thời điểm ký hợp đồng". Bằng chứng là một MMR proof tới version có dấu thời gian `ts ≤ t`, không cần tiết lộ toàn bộ lịch sử.

- **Chứng minh một trường mà giấu trường khác.** Với hồ sơ cấu trúc (học bạ, sổ bệnh), người dùng chứng minh đúng **một trường** — ví dụ "điểm Toán là 9", "nhóm máu là O" — mà **không để lộ** các trường còn lại (INV-E6). Một field-level proof từ `state_root` chỉ phơi bày trường được chọn; bên xác minh tin được trường đó mà vẫn không biết gì về phần riêng tư còn lại.

- **Kiểm tra lịch sử không bị sửa.** Bất kỳ ai cũng đối chiếu được chuỗi phiên bản với `anchor` đã neo trên Cardano để chắc rằng lịch sử **chưa bị tua lùi hay sửa lén**. Vì `anchor` đơn điệu theo `seq` (INV-E7), không ai neo lại được một phiên bản cũ để giả vờ trạng thái khác. Một bằng chứng inclusion cũ vẫn xác thực được sau khi lịch sử dài thêm — verify lại dưới tập peak hiện hành (INV-E3; chi tiết `Strata-Math §4.5`) — lịch sử chỉ dài thêm, không bao giờ viết lại.

---

## §3. Vì sao không dùng CIP-68 hay git nguyên bản

Hai công cụ gần nhất là **CIP-68** (chuẩn metadata cập nhật được trên Cardano) và **git** (lịch sử mã nguồn hash-linked). Cả hai đều giải một phần bài toán, nhưng đều thiếu mảnh quyết định nên Strata không thể dùng nguyên bản.

| Tiêu chí | CIP-68 | git | **Strata** |
|---|---|---|---|
| Lưu nội dung | On-chain, datum nhỏ, **đắt**, lộ hết | Off-chain (repo), không neo on-chain | **Off-chain qua Mirage**; on-chain chỉ cam kết lịch sử 32 byte (`mmr_root`), cả `anchor` 104 byte |
| Bằng chứng lịch sử | Không có proof gọn cho lịch sử | Có DAG bất biến, nhưng proof = **cả path** | **MMR proof `O(log n)`** — gọn cho chuỗi rất dài |
| Append-only nội tại | Không (ghi đè datum) | Có (DAG bất biến) | **Có** — thêm chỉ mở rộng MMR (INV-E3) |
| Neo on-chain / finality | Có (nhưng đắt và lộ) | **Không** có finality kinh tế | **Có** — `anchor` ký, neo Cardano (INV-E7) |
| Chống tua lùi mutable-ref | — | branch là ref đổi được, **không tamper-evident** | **head ký + anchor đơn điệu** theo `seq` |
| Riêng tư từng trường | Không | Không | **Field-level proof** từ `state_root` (INV-E6) |

Tóm gọn: **CIP-68** neo và cập nhật on-chain được nhưng datum nhỏ, đắt, lộ hết nội dung, và không có cách chứng minh lịch sử gọn. **git** có lịch sử bất biến đẹp nhưng bằng chứng inclusion là cả đường dẫn, không neo on-chain nên không có finality kinh tế, và branch (con trỏ đổi được) không chống sửa lén. Strata lấy phần mạnh của cả hai: nội dung off-chain (Mirage), cam kết lịch sử on-chain chỉ 32 byte (`mmr_root`, cả `anchor` 104 byte), MMR cho proof `O(log n)`, head được ký và neo, cộng riêng tư từng trường.

---

## §4. Bảo mật & quyền riêng tư (mức tính năng)

Strata đặt ba yêu cầu bảo mật ngay ở tầng tính năng, không để dồn xuống cài đặt.

- **Định danh không lộ loại (INV-E5).** Cả `ref_id` lẫn `content_cid` đều là **băm thuần** — không nhúng nhãn loại nội dung hay mức độ nhạy cảm. Đây là sửa trực tiếp một lỗi của hệ thống hiện tại, ở **cả hai đường runtime sinh CID**: đường legacy `gen_cid` lộ **tên loại dạng plaintext** ngay trong định danh (`ln1q_<hash>_<doc_type>`), nên nhìn CID là đọc thẳng được loại tài liệu; đường mới `gen_cid_v2` đỡ hơn nhưng vẫn lộ **một class byte** (Vault / Bulk), nên người ngoài vẫn đoán được "đây là dữ liệu nhạy cảm". Strata sửa **cả hai** bằng `ref_id` + `content_cid` là hash thuần: loại nằm **trong state đã cam kết** (sau `state_root`), không nằm trong định danh; nhìn `ref_id` không suy ra được nó là video công khai hay sổ bệnh.

- **Dữ liệu nhạy cảm: mã hóa cộng tái phân tán (INV-E9).** Nội dung và state nhạy cảm phải được **mã hóa** trước khi lưu, và chỉ cam kết băm (commitment) là công khai. Strata tách bạch hai việc thường bị gộp nhầm: "mã hóa" (giấu nội dung) và "tái phân tán" (đảm bảo dữ liệu còn tồn tại, không mất). Với dữ liệu nhạy cảm, Strata yêu cầu **cả hai** — mã hóa mà không tái phân tán thì dữ liệu an toàn nhưng dễ mất; tái phân tán mà không mã hóa thì còn nhưng hở. Việc mã hóa và tái phân tán thật giao cho Mirage; Strata chỉ ràng buộc rằng dữ liệu nhạy cảm không được công khai dạng thô.

- **Riêng tư từng trường (INV-E6).** Một proof cho một trường rút từ `state_root` **không tiết lộ giá trị** của bất kỳ trường nào khác. Đây là điều làm hồ sơ cấu trúc (học bạ, sổ bệnh) dùng được thật: chia sẻ đúng phần cần, giữ kín phần còn lại, mà bên nhận vẫn xác minh được phần được chia sẻ là thật và khớp với cam kết đã neo. Cần nói trung thực mức bảo vệ: đây là **"ZK-lite"**, không phải zero-knowledge đầy đủ — proof vẫn để lộ **số trường (xấp xỉ)** của hồ sơ và **các hash anh em** trên đường Merkle, nên một người quan sát có thể **so khớp một trường giữa hai phiên bản là đổi hay không đổi** (dù không đọc được giá trị). Với dữ liệu nhạy cảm (sổ bệnh), Strata yêu cầu bật **padding** (chèn trường giả để giấu số trường thật) và **blinding** (làm nhiễu giá trị trước khi băm để chặn so khớp giữa các version). Bảo vệ zero-knowledge đầy đủ là hướng nâng cấp về sau.

---

## §5. Trạng thái hiện tại & lộ trình

Cần nói thẳng để khỏi nhầm: **hôm nay hệ thống chỉ có dữ liệu tĩnh.** Lớp lưu trữ hiện tại định nghĩa `DataClass { Vault, Bulk }`, và **cả hai đều là tĩnh** — chỉ khác nhau ở mức bảo mật và cách phân tán, không khác nhau ở vòng đời. Ba loại còn lại trong khung MECE — Chuỗi-thêm (#2), Thanh-ghi (#3), Hồ sơ cấu trúc (#4) — **hiện chưa có gì** trong hệ thống. Không có append, không có register, không có hồ sơ cập nhật từng phần.

Strata là **một đề xuất mới**, chưa được cài đặt. Quan hệ với lớp hiện tại: Strata là **tầng nằm TRÊN** `DataClass {Vault, Bulk}`. Nội dung của mỗi phiên bản Strata vẫn được lưu qua Mirage theo đúng lớp bảo mật cũ (Vault cho nhạy cảm, Bulk cho khối lượng lớn). Điểm mới là **loại dữ liệu (vòng đời) nằm trong state đã cam kết, không nằm trong định danh** — nên thêm khả năng tiến hóa mà không phá lớp lưu trữ sẵn có, và đồng thời bịt lỗ lộ loại qua CID (INV-E5).

Lộ trình ở mức tính năng:

| Hạng mục | Trạng thái |
|---|---|
| Dữ liệu tĩnh (#1) | **Đã có** (qua `DataClass {Vault, Bulk}`, đều tĩnh) |
| Định danh ổn định `ref_id` không lộ loại | Đề xuất Strata (mới) |
| Chuỗi-thêm (#2) qua MMR | Đề xuất Strata (mới) |
| Thanh-ghi (#3) qua đọc head | Đề xuất Strata (mới) |
| Hồ sơ cấu trúc (#4) + field-level proof | Đề xuất Strata (mới) |
| Neo on-chain `anchor` chống rollback | Đề xuất Strata (mới) |

---

## §6. Strata ghép (Composite Strata) — đối tượng thật là rừng Strata

Bốn loại MECE (§1) là bốn **viên gạch nguyên thủy**. Đối tượng thật mà người dùng thấy hiếm khi là một viên gạch đơn lẻ — nó là một **rừng (forest) hoặc đồ thị các Strata nguyên thủy** ghép lại theo quan hệ cha–con. Đây là quyết định kiến trúc đã chốt qua phản biện: KHÔNG cố nhét một đối tượng phức tạp vào một loại MECE duy nhất, mà **tổ hợp** nhiều Strata nguyên thủy, mỗi cái đúng loại của nó.

Lý do nguyên lý gốc: mỗi loại MECE giải đúng một quan hệ định danh↔nội dung↔thời gian. Một nhóm chat vừa có "dòng tin chỉ thêm" (loại #2) vừa có "tên nhóm đổi được" (loại #3) — ép cả hai vào một cây sẽ phá field-privacy hoặc phá append-only. Tách thành nhiều Strata, mỗi cái một loại, rồi ghép bằng tham chiếu `ref_id`, giữ được tính chất của từng phần.

**Ví dụ tổ hợp:**

| Đối tượng thật | Tổ hợp Strata nguyên thủy |
|---|---|
| **Nhóm chat** | Một **rừng channel-log**: mỗi kênh là một Strata loại #2 (append-only); mỗi **tin nhắn** là một Strata loại #2 (hoặc #3 nếu cho sửa/thu hồi); metadata nhóm (tên, ảnh) là một Strata #3 (register). |
| **Bảng dữ liệu (tabular)** | Một **rừng row-Strata** (mỗi hàng = một Strata #4 hồ sơ cấu trúc) + một Strata index (#3, register trỏ tập `ref_id` hàng). |
| **Hồ sơ mạng xã hội** | `profile` (#4 hồ sơ cấu trúc) + `posts` (#1 tĩnh cho bài đã đăng, hoặc #2 cho dòng bài) + `counters` (#3 register: số follower/like, materialize từ append-log #2). |

**Cấu trúc `CompositeStrata`** (mức tính năng — toán chi tiết ở Strata-Math §12):
- Một tập **tham chiếu con** `children: [(ref_id, role)]` — mỗi con là một Strata nguyên thủy với vai trò đã đặt tên (ví dụ `"profile"`, `"posts"`, `"counters"`).
- Một **quan hệ cha–con**: Strata ghép tự nó cũng là một Strata (#4 thường) mà state chứa các `ref_id` con; cập nhật danh sách con = một version mới của Strata cha.
- Proof của một phần (ví dụ "bài đăng X thuộc profile Y") = field-proof trỏ tới `ref_id` con (từ state Strata cha) **cộng** inclusion/field-proof bên trong Strata con. Hai tầng proof, vẫn `O(log n)` mỗi tầng.

Điểm cốt lõi: bộ máy KHÔNG đổi — vẫn `ref_id` + chuỗi version + MMR + state_root + policy. Composite chỉ là **một Strata cha tham chiếu các Strata con**, đệ quy. Không cần primitive mới.

---

## §7. Hai trục thiết kế — cấu trúc tách rời mức phân tán

Một quyết định chốt qua phản biện: **hai câu hỏi thiết kế hoàn toàn độc lập**, đừng trộn.

- **Trục 1 — Loại MECE (#1–#4) quyết định CẤU TRÚC.** Đối tượng là tĩnh, chuỗi-thêm, register, hay hồ sơ cấu trúc? Trục này quyết hình dạng cây băm, kiểu proof, có hay không field-privacy. Nó KHÔNG nói gì về việc dữ liệu được lưu ở đâu hay phân tán bao xa.
- **Trục 2 — Mẫu tần-suất/giá-trị quyết định MỨC PHÂN TÁN & CAM KẾT.** Dữ liệu này cập nhật mỗi giây hay mỗi năm? Giá trị thấp (log debug) hay cao (hồ sơ pháp lý)? Trục này quyết: gộp lô hay không, phân tán tới bao nhiêu node, có neo on-chain hay không.

Hai trục trực giao: một register tần suất cao giá trị thấp (đo nhiệt độ phòng) và một register tần suất thấp giá trị cao (số dư ví lớn) **cùng loại #3** nhưng **mức phân tán/cam kết khác hẳn**.

**Bốn tầng lưu trữ** (chọn theo Trục 2, không theo Trục 1):

| Tầng | Tên | Khi nào dùng | Phân tán / anchor |
|---|---|---|---|
| **(a)** | **Nóng cục bộ** | Dữ liệu đang ghi dày, vừa tạo, giá trị thấp hoặc tạm | KHÔNG phân tán, KHÔNG anchor. Sống ở node tạo ra nó. |
| **(b)** | **Checkpoint gộp lô** | Tần suất cao cần giữ lịch sử nhưng không cần mỗi entry on-chain | Gộp một lô entry thành **sub-MMR theo epoch** (xem §8/§7-Tech), một checkpoint cam kết cả lô. Anchor theo checkpoint, không theo entry. |
| **(c)** | **Phân tán chọn lọc** | Dữ liệu cần bền (không mất) nhưng chưa cần finality on-chain | Tái phân tán qua Mirage tới nhiều node + repair. Vẫn off-chain. |
| **(d)** | **Anchor on-chain** | Giá trị cao, cần chống rollback + finality kinh tế | Neo `anchor` 104 byte lên Cardano (INV-E7). |

Nguyên tắc cứng: **chỉ phân tán (c) và anchor (d) theo tầng giá trị.** Không phải mọi Strata đều lên chuỗi — đẩy hết lên chuỗi là lãng phí và đắt vô ích. Dữ liệu giá trị thấp dừng ở (a)/(b); chỉ thứ thật sự cần bất biến kinh tế mới trả phí (d). Đây cũng là cách giữ chi phí user hợp lý: user trả theo tầng họ chọn.

---

## §8. Bảng / dữ liệu tabular — granularity per-row

Bảng (spreadsheet, database table) là một **rừng row-Strata** (xem §6). Quyết định chốt qua phản biện về **độ hạt (granularity)**:

- **Granularity = PER-ROW.** Mỗi **hàng** là một Strata loại #4 (hồ sơ cấu trúc): mỗi **cột** là một trường trong `state_root` (field-Merkle). Cập nhật một ô = một version mới của đúng hàng đó.
- **KHÔNG per-cell.** Một Strata cho mỗi ô sẽ đẻ ra số Strata khổng lồ (hàng × cột), proof phân mảnh, không có ranh giới tự nhiên cho quyền/đồng bộ.
- **KHÔNG per-table-snapshot.** Một Strata cho cả bảng (mỗi sửa đổi = version mới của toàn bảng) sẽ làm mọi cập nhật ô đụng vào cùng một chuỗi, phá song song và phình lịch sử.

Per-row là điểm cân bằng: hàng là đơn vị quyền tự nhiên (ai sửa được hàng nào), đơn vị đồng bộ tự nhiên (hai người sửa hai hàng khác nhau không xung đột), và cho field-privacy ở mức cột (chứng minh "lương của hàng này là X" mà không lộ cột khác — INV-E6).

**Tính tổng có proof = Merkle Sum Tree.** Khi cần "tổng cột lương = 5 tỷ" mà chứng minh được, dùng **Merkle Sum Tree**: mỗi node lưu thêm `sum` và `count` của cây con. Verifier kiểm tổng ở gốc khớp, và mỗi proof một hàng chứng minh hàng đó góp đúng phần vào tổng. (Toán ở Strata-Math §14.)

**Lọc / join = columnar engine derived.** Truy vấn kiểu "lọc lương > X" hay "join hai bảng" KHÔNG chạy trên cây băm — chạy trên một **columnar engine** dựng từ dữ liệu (derived, untrusted, xem nguyên tắc index ở Strata-Tech). Ngưỡng khi nào bật columnar engine quyết theo **đo thực** (số hàng, tần suất truy vấn), không định trước.

> **Ghi chú trùng VeData A22 MEASUREMENT_SERIES.** Cấu trúc per-row + Merkle Sum Tree trùng với chuỗi đo lường A22 của VeData (GreenSun). Hai bên **dùng chung sub-primitive** Merkle Sum Tree (qua `lampnet-merkle-anchor` hash-agnostic, xem Strata-Tech §0.5), không cài hai lần.

---

## §9. Đệm nhiễu (decoy/padding) chống suy luận qua kích thước (mở rộng INV-E9)

§4 đã nêu INV-E9 (mã hóa + tái phân tán) và padding số trường (INV-E6). Mục này mở rộng cho **kênh phụ kích thước/traffic** — một lỗ riêng tư mà mã hóa nội dung KHÔNG bịt được: kể cả khi nội dung mã hóa, **kích thước** bản mã và **mẫu lưu lượng** vẫn để lộ loại dữ liệu (một sổ bệnh dài khác một tin nhắn ngắn).

Quyết định: với dữ liệu nhạy cảm, Strata yêu cầu hai lớp đệm:
- **Bucket kích thước cố định**: bản mã được đệm lên một trong vài **bucket cố định** (ví dụ 4 KB / 64 KB / 1 MB). Mọi object trong cùng bucket nhìn giống nhau về kích thước → không suy ra được loại từ độ dài.
- **Shard nhiễu kích thước khác nhau**: chèn các shard giả (decoy) có kích thước khác nhau vào dòng phân tán, để phân tích lưu lượng (traffic-analysis) không tách được shard thật khỏi nhiễu.

Hai lớp này chống **suy luận loại qua kích thước** và **traffic-analysis** — phần mà INV-E5 (định danh không lộ loại) và mã hóa nội dung bỏ sót.

**Chi phí do user trả.** Đệm và shard nhiễu làm tăng dung lượng lưu + băng thông xử lý. Quyết định: **chi phí tăng này do user trả** — gắn vào pricing (user chọn mức riêng tư cao thì trả nhiều hơn). Không bắt mạng gánh chi phí riêng tư của một cá nhân. Đây là đánh đổi có chủ đích, minh bạch trong giá.

---

## §10. Audit-log bất biến cho object nhạy cảm

Quyết định chốt qua phản biện: mỗi object nhạy cảm gắn kèm **một Strata audit-log riêng** (loại #2, append-only) ghi lại mọi sự kiện vòng đời. Lý do nguyên lý gốc: với dữ liệu nhạy cảm (sổ bệnh, hồ sơ pháp lý), "ai đã xem cái gì, khi nào" tự nó là thông tin cần bất biến và kiểm chứng được — không thể để trong một log sửa được.

Audit-log là một Strata #2 độc lập, gắn với object nhạy cảm qua tham chiếu (như một con trong Composite §6). Mỗi **lần truy cập hoặc lần ký** sinh **một entry** append vào log đó — bất biến (INV-E3), chứng minh được (inclusion-proof), neo được on-chain nếu cần.

**Cấu trúc `AuditEntry`** (mức tính năng — mỗi entry ghi năm chiều):
- **Tạo khi nào** — dấu thời gian sự kiện.
- **Ai truy cập** — DID người thực hiện (`author_did` PhoenixKey).
- **Khi nào** — `ts` của lần truy cập/ký.
- **Ký cái gì** — `version_hash` hoặc `content_cid` của object được truy cập/ký.
- **Ở đâu** — vị trí/ngữ cảnh truy cập (qua Compass — định vị/ngữ cảnh trong hệ sinh thái).

Vì audit-log là Strata #2, mỗi entry kế thừa toàn bộ tính bất biến: không xóa được entry cũ (append-only), không sửa lén (hash-linked + có thể anchor). Một bên kiểm toán chứng minh được "object này đã bị DID Z truy cập lúc T" bằng inclusion-proof, mà không cần tin máy chủ.

---

## Liên kết

- **Strata-API.md** — API public cho platform (ProofChat/OriLife/AladinWork): 7 thao tác `create`/`append_version`/`append_event`/`read_head`/`read_at(t)`/`prove_version`/`prove_field` + `anchor`, request/response HTTP, bảng lỗi, adapter neo, chi tiết build Composite + 4 tầng + index derived.
- **Strata-Math.md** — toán: cấu trúc `version`/`version_hash`, cây MMR và history accumulator, RFC 6962 + dup-leaf guard, `state_root` và field-level proof, "giá trị tại thời điểm t", gộp lô tần suất cao (sub-MMR theo epoch, CRDT cho register), Composite Strata (§12), truy vấn lịch sử & số liệu (§13), Merkle Sum Tree cho tabular (§14).
- **Strata-Tech.md** — kỹ thuật: cài đặt thành code, lược đồ datum on-chain của `anchor`, giao tiếp với Mirage (lưu/mã hóa/tái phân tán) và PhoenixKey (DID + chữ ký), gộp lô và checkpoint.
- **_CONTRACT.md** — khế ước giao diện nội bộ: tên, ký hiệu, bộ invariant `INV-E1..INV-E9` dùng chung cho cả ba file.
