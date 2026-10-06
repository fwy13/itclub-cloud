<div align="center">

<img src="frontend/public/brand/itclub-mark.svg" alt="ITClub Cloud" width="96" />

# ITClub Cloud

**Lưu điều hay. Chia sẻ điều mới.**

Không gian lưu trữ cá nhân, kết nối qua Telegram.

**Rust · React · TypeScript · Tailwind CSS**

</div>

---

## Giới thiệu

**ITClub Cloud** giúp bạn lưu trữ, quản lý và chia sẻ file qua Telegram bằng giao diện web. File lớn được chia thành nhiều phần khi lưu lên Telegram; bạn quản lý chúng như những file thông thường trong thư viện.

Giao diện sử dụng nhận diện IT Club với màu navy, cyan, mint và font Open Sans. Logo và favicon có nền trong suốt.

## Tính năng

- 📁 Quản lý thư mục, tìm kiếm, đổi tên, di chuyển, sao chép và thùng rác.
- ☁️ Upload file hoặc cả thư mục; tiếp tục upload khi chọn lại cùng file.
- 🎬 Xem trước ảnh, video, âm thanh, PDF, EPUB và CBZ.
- 🔗 Chia sẻ file/thư mục bằng liên kết, có mật khẩu và thời hạn.
- 📊 Theo dõi tiến trình upload, hủy hoặc thử lại tác vụ.
- 🔐 Đăng nhập web bằng mật khẩu hoặc passkey; kết nối Telegram bằng QR.
- 👥 Quản lý thành viên, quota và bot lưu trữ trong nhóm/kênh.

## Chuẩn bị

- Rust và Cargo.
- Node.js 22 và npm.
- TDLib: thư viện `libtdjson.so` có hỗ trợ kiểu `inputDocument`.
- FFmpeg để xử lý media.
- Telegram API ID và API hash, lấy tại [my.telegram.org/apps](https://my.telegram.org/apps).

## Cấu hình

Tạo file `.env` trong thư mục `backend`, dựa trên file mẫu của dự án. Các thông số chính:

```dotenv
BIND=0.0.0.0:8091
PUBLIC_URL=http://localhost:5173
DATA_DIR=./data
TDLIB_PATH=/duong/dan/libtdjson.so
SETUP_TOKEN=thay-bang-ma-thiet-lap-ngau-nhien
RUST_LOG=itclub_cloud=info
```

| Biến | Ý nghĩa |
| --- | --- |
| `BIND` | Địa chỉ và cổng backend lắng nghe. |
| `PUBLIC_URL` | Địa chỉ truy cập giao diện, gồm giao thức và cổng. |
| `DATA_DIR` | Nơi lưu metadata, khóa mã hóa, phiên Telegram và dữ liệu tạm. |
| `TDLIB_PATH` | Đường dẫn thực tế đến `libtdjson.so`. |
| `SETUP_TOKEN` | Mã dùng để tạo tài khoản quản trị lần đầu. |

Nếu truy cập từ thiết bị khác, thay `localhost` trong `PUBLIC_URL` bằng IP hoặc tên miền của máy chủ. Khi nâng cấp, tiếp tục dùng đúng `DATA_DIR` cũ.

## Khởi động

Mở terminal thứ nhất để chạy backend:

```bash
cd backend
cargo run --release
```

Mở terminal thứ hai để chạy frontend:

```bash
cd frontend
npm install
npm run dev
```

Truy cập **http://localhost:5173**.

Nếu chạy trên VPS, truy cập `http://IP-VPS:5173` và cho phép kết nối đến cổng này trong firewall. Frontend chuyển tiếp yêu cầu API đến backend ở cổng `8091`.

> Cách chạy trên sử dụng Vite dev server, phù hợp để phát triển và dùng thử. Passkey trên địa chỉ IP qua HTTP không hoạt động; tính năng này cần HTTPS hoặc localhost.

## Thiết lập lần đầu

1. Mở giao diện và nhập mã `SETUP_TOKEN`.
2. Tạo tài khoản quản trị.
3. Vào **Cài đặt → Kết nối Telegram**.
4. Nhập **API ID** và **API hash**, sau đó chọn **Lưu và khởi tạo**.
5. Chọn **Hiện mã QR đăng nhập**.
6. Trên Telegram điện thoại, mở **Cài đặt → Thiết bị → Liên kết thiết bị máy tính** và quét mã.
7. Nhập mật khẩu xác minh hai bước nếu được yêu cầu.

QR dùng để kết nối Telegram làm nơi lưu trữ. Đăng nhập giao diện web bằng tài khoản ITClub Cloud hoặc passkey đã đăng ký.

## Chọn nơi lưu trữ

Trong **Cài đặt → Nơi lưu dữ liệu**, chọn:

- **`me`**: lưu vào Saved Messages của tài khoản chính.
- **Chat ID hoặc `@username`**: lưu vào nhóm/kênh mà tài khoản có quyền gửi tài liệu.

Nhấn **Lấy danh sách chat** nếu muốn chọn từ các cuộc trò chuyện hiện có, sau đó nhấn **Lưu**.

### Sử dụng bot upload

1. Tạo bot qua **@BotFather** và lấy token.
2. Thêm bot cùng tài khoản chính vào nhóm/kênh lưu trữ.
3. Cấp quyền gửi tài liệu; với kênh, cấp quyền đăng bài phù hợp.
4. Thêm tên và token trong **Cài đặt → Pool upload Telegram**.
5. Chọn nhóm/kênh đó làm nơi lưu dữ liệu và đợi bot báo đã đăng nhập.

Bot không upload vào Saved Messages. Nhiều bot có thể hỗ trợ tổng thông lượng khi có nhiều file chạy đồng thời; tốc độ thực tế phụ thuộc mạng và giới hạn Telegram.

## Sử dụng hằng ngày

### Tải file lên

Trong **Tệp của tôi**, chọn **Tải file lên**, dùng nút tải cả thư mục hoặc kéo thả file vào giao diện. File được đưa vào thư mục đang mở.

Giữ tab mở trong lúc gửi file lên server. Sau khi server nhận đủ file, theo dõi quá trình lưu lên Telegram trong **Tác vụ**.

Nếu upload bị gián đoạn, chọn lại cùng file trong cùng thư mục để tiếp tục.

### Quản lý và xem file

Nhấn vào file để xem trước. Mở menu **⋯** để tải xuống, đổi tên, di chuyển, sao chép, chia sẻ hoặc xóa.

File đã xóa được chuyển vào **Thùng rác**. Bạn có thể khôi phục hoặc xóa vĩnh viễn tại đây.

### Chia sẻ

1. Mở menu **⋯** của file hoặc thư mục.
2. Chọn **Chia sẻ**.
3. Đặt mật khẩu và thời hạn nếu cần.
4. Sao chép liên kết để gửi cho người nhận.

Thu hồi liên kết tại mục **Đã chia sẻ**.

### Tài khoản và bảo mật

- Quản trị viên quản lý tài khoản, khóa tài khoản và đặt quota tại **Thành viên**.
- Người dùng đổi mật khẩu hoặc thêm passkey trong **Cài đặt**.
- Giữ nguyên `PUBLIC_URL` khi nâng cấp để tiếp tục sử dụng passkey theo địa chỉ đã đăng ký.

### Sao lưu

Tải bản sao lưu metadata tại **Cài đặt → Sao lưu metadata** hoặc bật gửi bản sao lưu hằng ngày lên Telegram.

Giữ thêm bản sao an toàn của **`master.key`** và thư mục phiên **TDLib** trong `DATA_DIR`. Bản ZIP metadata không thay thế các dữ liệu này.

## Module

Giao diện Module và tải từ liên kết hiện đang được ẩn. Backend module vẫn được giữ để phát triển lại sau.

---

<div align="center">

**ITClub Cloud**  
Lưu trữ · Kết nối · Chia sẻ

</div>