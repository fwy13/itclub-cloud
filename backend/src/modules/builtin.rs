use super::*;
pub struct DirectHttp;
pub struct TelegramSource;
pub struct TorrentSource;
pub struct YtDlpSource;
pub struct HlsSource;

#[async_trait]
impl DownloadModule for DirectHttp {
    fn info(&self)->ModuleInfo {ModuleInfo{id:"url",name:"HTTP / HTTPS",description:"Tải file trực tiếp từ URL công khai",admin_only:false,options_schema:no_options()}}
    fn matches(&self,u:&url::Url)->bool {matches!(u.scheme(),"http"|"https")}
    async fn resolve(&self,_:&ModuleContext,url:&str,_:&Value)->Result<DownloadPlan>{
        let (parsed,_)=crate::remote::validate_url(url).await?;
        let filename=parsed.path_segments().and_then(|mut s|s.next_back()).filter(|s|!s.is_empty()).map(|s|percent_encoding::percent_decode_str(s).decode_utf8_lossy().to_string()).filter(|s|crate::db::valid_name(s).is_ok());
        Ok(DownloadPlan::Http{url:url.into(),filename,headers:Headers::new()})
    }
}
#[async_trait]
impl DownloadModule for TelegramSource {
    fn info(&self)->ModuleInfo {ModuleInfo{id:"telegram",name:"Telegram",description:"Nhập file từ tin nhắn mà tài khoản đã đăng nhập có quyền đọc",admin_only:true,options_schema:no_options()}}
    fn matches(&self,u:&url::Url)->bool {matches!(u.host_str(),Some("t.me"|"telegram.me"))}
    async fn resolve(&self,_:&ModuleContext,url:&str,_:&Value)->Result<DownloadPlan>{Ok(DownloadPlan::Telegram{url:url.into()})}
}
#[async_trait]
impl DownloadModule for TorrentSource {
    fn info(&self)->ModuleInfo {ModuleInfo{id:"torrent",name:"Torrent / Magnet",description:"Tải bằng aria2c; dành cho quản trị viên",admin_only:true,options_schema:no_options()}}
    fn matches(&self,u:&url::Url)->bool {u.scheme()=="magnet"||u.path().ends_with(".torrent")}
    async fn resolve(&self,_:&ModuleContext,url:&str,_:&Value)->Result<DownloadPlan>{if !url.starts_with("magnet:?"){crate::remote::validate_url(url).await?;}Ok(DownloadPlan::Tool{tool:"torrent",url:url.into()})}
}
#[async_trait]
impl DownloadModule for YtDlpSource {
    fn info(&self)->ModuleInfo {ModuleInfo{id:"ytdlp",name:"yt-dlp",description:"Video từ các website yt-dlp hỗ trợ; dành cho quản trị viên",admin_only:true,options_schema:no_options()}}
    fn matches(&self,u:&url::Url)->bool {let h=u.host_str().unwrap_or("");["youtube.com","youtu.be","vimeo.com","tiktok.com","facebook.com"].iter().any(|d|h==*d||h.ends_with(&format!(".{d}")))}
    async fn resolve(&self,_:&ModuleContext,url:&str,_:&Value)->Result<DownloadPlan>{crate::remote::validate_url(url).await?;Ok(DownloadPlan::Tool{tool:"ytdlp",url:url.into()})}
}
#[async_trait]
impl DownloadModule for HlsSource {
    fn info(&self)->ModuleInfo {ModuleInfo{id:"hls",name:"HLS → MP4",description:"Playlist VOD không mã hóa, tải segment bằng Rust rồi mux bằng FFmpeg",admin_only:false,options_schema:no_options()}}
    fn matches(&self,u:&url::Url)->bool {u.path().to_lowercase().ends_with(".m3u8")}
    async fn resolve(&self,_:&ModuleContext,url:&str,_:&Value)->Result<DownloadPlan>{Ok(DownloadPlan::Hls{url:url.into(),filename:"video.mp4".into(),headers:Headers::new()})}
}
