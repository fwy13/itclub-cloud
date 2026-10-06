use axum::{http::StatusCode, response::{IntoResponse,Response},Json};
#[derive(Debug)]
pub struct Error(pub StatusCode,pub String);
impl std::fmt::Display for Error { fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result { write!(f,"{}",self.1) } }
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T,Error>;
impl Error {
    pub fn bad(s:impl Into<String>)->Self {Self(StatusCode::BAD_REQUEST,s.into())}
    pub fn forbidden()->Self {Self(StatusCode::FORBIDDEN,"Không có quyền truy cập".into())}
    pub fn not_found()->Self {Self(StatusCode::NOT_FOUND,"Không tìm thấy dữ liệu".into())}
    pub fn unauthorized()->Self {Self(StatusCode::UNAUTHORIZED,"Vui lòng đăng nhập".into())}
}
impl IntoResponse for Error {
    fn into_response(self)->Response {(self.0,Json(serde_json::json!({"error":self.1}))).into_response()}
}
impl From<anyhow::Error> for Error {fn from(e:anyhow::Error)->Self {tracing::warn!(error=%e,"Request failed");Self(StatusCode::INTERNAL_SERVER_ERROR,e.to_string())}}
impl From<sqlx::Error> for Error {fn from(e:sqlx::Error)->Self {
    if e.as_database_error().map(|e|e.is_unique_violation()).unwrap_or(false) {return Self(StatusCode::CONFLICT,"Tên hoặc thông tin này đã tồn tại".into());}
    tracing::error!(error=%e,"Database error");Self(StatusCode::INTERNAL_SERVER_ERROR,"Lỗi cơ sở dữ liệu".into())
}}
impl From<std::io::Error> for Error {fn from(e:std::io::Error)->Self {Self::from(anyhow::Error::from(e))}}
impl From<serde_json::Error> for Error {fn from(e:serde_json::Error)->Self {Self::bad(e.to_string())}}
