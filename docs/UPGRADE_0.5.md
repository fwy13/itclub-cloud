# ITClub Cloud 0.5.0

## Nâng cấp từ 0.4.0

Gói này chỉ có mã nguồn. Chưa cài dependency, build, chạy, typecheck hay test theo yêu cầu.

1. Dừng backend và sao lưu toàn bộ `DATA_DIR`, đặc biệt database, `master.key` và phiên TDLib. Giữ nguyên `.env` và cấu hình Vercel/Vite đang dùng.
2. Thay source bằng ZIP mới, hoặc chạy `git apply /đường/dẫn/itclub-cloud-0.5.0.patch` tại thư mục gốc source **0.4.0**. Patch có thể xung đột nếu bạn đã tự sửa cùng file; không áp dụng ép lên bản khác.
3. Bản này thêm thư viện preview và bỏ dependency passkey. Chạy `npm install` rồi `npm run build` trong `frontend`; chạy `cargo build --release` trong `backend`. Đây là lệnh dành cho bạn chạy, chưa được thực thi khi bàn giao.
4. Thêm các biến SSD bên dưới vào `.env` backend. Chép binary `backend/target/release/itclub-cloud` vào vị trí `ExecStart` hiện tại và deploy frontend mới cùng phiên bản.
5. Khởi động lại service. Migration `0003_upload_statistics.sql` tự chạy. Không sửa/xóa migration 0001 hoặc 0002 đã áp dụng.

Đăng nhập web hiện chỉ dùng mật khẩu. Nếu trước đây chỉ dùng passkey, đặt mật khẩu trước khi nâng cấp. Mã API, UI và dependency WebAuthn đã bỏ; bảng passkey cũ giữ nguyên để không thay checksum migration đã phát hành, không còn endpoint sử dụng bảng đó. Feature `vendored-openssl` giữ dưới dạng không làm gì để lệnh build cũ vẫn hợp lệ. TDLib vẫn là thư viện native với dependency riêng.

## Giới hạn SSD

```dotenv
SSD_TEMP_LIMIT_GIB=20
SSD_MIN_FREE_GIB=2
PART_SIZE_MIB=256
MAX_PARALLEL_UPLOADS=2
TDLIB_CACHE_MIB=1024
```

- `SSD_TEMP_LIMIT_GIB`: tổng dung lượng logic của `DATA_DIR/temp` và `DATA_DIR/archives`, gồm file upload, part Telegram, multipart S3, cache truyện, backup và thư mục tải module. Mặc định 20 GiB; `0` bỏ trần dung lượng tạm.
- `SSD_MIN_FREE_GIB`: dung lượng còn trống tối thiểu cần giữ trên filesystem chứa `DATA_DIR`. Mặc định 2 GiB; kiểm tra cả khi backend chạy bằng root. `0` tắt phần dự phòng này.
- Khi nhận file mới, dành khoảng trống cho `PART_SIZE_MIB × MAX_PARALLEL_UPLOADS`. Với mặc định là 512 MiB. Vì thế không thể nhận đủ một file 20 GiB khi hạn mức tạm là 20 GiB.
- File web biết trước kích thước được cấp phát block trên SSD ngay khi bắt đầu, trước khi nhận chunk. Hai lượt upload không cùng hứa sử dụng phần trống đó. Filesystem phải hỗ trợ cấp phát qua `fs2::FileExt::allocate`; nếu không, request bị từ chối thay vì âm thầm dùng file sparse.
- Stream WebDAV/S3/HTTP/HLS được kiểm tra theo từng lần ghi; part Telegram, file ghép S3 và cache archive kiểm tra trước khi cấp phát. Vượt ngưỡng trả lỗi **HTTP 507** hoặc lỗi tương ứng trong Tác vụ.
- File lỗi/tạm dừng còn được giữ để tiếp tục, vẫn chiếm hạn mức. Hủy tác vụ không cần thiết để dọn file tạm. Không xóa thủ công file đang upload. Archive cache cũ cũng tính vào hạn mức; có thể dọn nội dung `DATA_DIR/archives` khi đã dừng backend.
- Không tự xóa file trên Telegram khi SSD đầy. Giới hạn này khác quota file đã lưu của từng thành viên.

**Phạm vi:** đây là kiểm soát ghi của ứng dụng, không phải quota cứng cho toàn bộ ổ. Database, thumbnail và cache TDLib nằm ngoài trần thư mục tạm nhưng làm giảm dung lượng trống. `TDLIB_CACHE_MIB` là mục tiêu dọn cache **mỗi tài khoản**, không phải trần tức thời. Downloader bên ngoài/FFmpeg và tải TDLib được theo dõi định kỳ nên có thể vượt ngưỡng giữa hai lần kiểm tra; module tự viết phải dùng helper `disk` khi ghi dữ liệu. Các thư mục con của `DATA_DIR` cần cùng filesystem, không thay bằng symlink hoặc mount riêng. Muốn giới hạn tuyệt đối toàn bộ ITClub Cloud, đặt `DATA_DIR` trên filesystem/volume có quota riêng. Chỉ chạy một backend trên cùng `DATA_DIR`.

Trong **Cài đặt → Dung lượng SSD**, admin xem được dung lượng tạm, hạn mức, dung lượng còn trống và phần giữ lại. Nút cập nhật đọc lại số liệu; thay cấu hình bằng `.env` rồi restart backend.

## Progress và thống kê

- Hai chặng riêng: thiết bị → server và server → Telegram. Tiến trình Telegram lấy từ TDLib, không giả lập theo thời gian. 100% byte có thể vẫn đang chờ Telegram xác nhận gửi thành công.
- WebSocket được giữ ổn định khi chuyển thư mục. Polling tự đối soát khi thiếu/mất WebSocket. Với host không proxy WebSocket, đặt `VITE_DISABLE_WS=true` khi build frontend.
- Tổng MB của thành viên chỉ cộng file upload hoàn tất, không cộng upload lỗi, bản sao file hoặc tác vụ cha của danh sách. Upload ghi đè thành công tính thêm dung lượng đã gửi. Xóa file không giảm bộ đếm này.
- Số liệu lịch sử được khởi tạo từ tác vụ thành công còn trong database. Tác vụ lịch sử đã bị xóa không thể khôi phục để tính lại. `Đang lưu` là số liệu riêng, gồm cả thùng rác. MB dùng 1.000.000 byte; GiB cấu hình dùng 1.073.741.824 byte.

## Xem trước

| Loại | Cách hiển thị / giới hạn |
| --- | --- |
| Ảnh JPG, PNG, GIF, WebP, AVIF, SVG, BMP, ICO | Trình duyệt hiển thị, có thu phóng; tùy khả năng trình duyệt |
| Video, audio | Streaming/seek; codec phụ thuộc trình duyệt, không tự transcode |
| PDF | Trình xem PDF của trình duyệt |
| EPUB | Trình đọc sách, tắt scripted content |
| CBZ cá nhân | Trình đọc truyện dùng backend, archive tối đa 2 GiB, phụ thuộc SSD còn trống |
| ZIP, CBZ chia sẻ | Danh sách entry, xem ảnh/text và tải từng entry; tối đa 25 MiB file nén |
| DOCX | Nội dung, bảng và ảnh nhúng; bố cục có thể khác Word |
| XLSX | Giá trị lưu sẵn; tối đa 30 sheet, 1.000 dòng và 50 cột mỗi sheet; không chạy macro/tính công thức |
| PPTX, ODT, ODP | Văn bản; không dựng lại hiệu ứng, hình hoặc bố cục slide |
| Markdown | Bản đọc và mã nguồn; không chạy HTML hoặc tự tải ảnh ngoài |
| CSV, TSV | Bảng tối đa 1.000 dòng/50 cột và bản text |
| JSON, source code, log, XML, HTML, phụ đề… | Bản text; JSON được thụt dòng nếu hợp lệ; không thực thi mã |
| TTF, OTF, WOFF, WOFF2 | Thử font với văn bản tùy chỉnh, tối đa 10 MiB |

Text tối đa 2 MiB; tài liệu Office/OpenDocument tối đa 25 MiB. ZIP dùng cho preview có tối đa 5.000 entry, 32 MiB/entry và 64 MiB tổng dữ liệu giải nén. Giải nén trong worker có timeout 15 giây. File vượt giới hạn hoặc định dạng chưa hỗ trợ vẫn có thể tải xuống. Không gửi tài liệu sang dịch vụ xem Office bên ngoài.

## Share có mật khẩu

Trang mở khóa không yêu cầu tài khoản web. Nhập sai trả thông báo **“Mật khẩu chia sẻ không đúng. Vui lòng thử lại.”** và giữ form để nhập lại. Vẫn giữ giới hạn thử mật khẩu, cookie mở khóa và thời hạn link như trước.
