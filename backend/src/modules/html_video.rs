//! A functioning, deliberately small example extractor to extend for your anime site.
//! It handles HTML <video>, <source>, and OpenGraph video URLs, without executing JS.
use super::*;
use futures_util::StreamExt;
use scraper::{Html,Selector};

pub struct HtmlVideo;
#[async_trait]
impl DownloadModule for HtmlVideo {
    fn info(&self)->ModuleInfo {
        ModuleInfo {id:"html-video",name:"Video trong trang HTML",description:"Module mẫu: đọc video/source/og:video; có thể mở rộng thành bộ tải anime",admin_only:false,
            options_schema:json!({"type":"object","properties":{"selector":{"type":"string","default":"video[src], source[src], meta[property='og:video'], meta[property='og:video:url']","description":"CSS selector trỏ tới URL video"}},"additionalProperties":false})}
    }
    fn matches(&self,u:&url::Url)->bool {matches!(u.scheme(),"http"|"https")&&(u.path().contains("/watch/")||u.path().contains("/episode/")||u.path().ends_with(".html"))}
    async fn resolve(&self,ctx:&ModuleContext,url:&str,options:&Value)->Result<DownloadPlan> {
        let response=ctx.get(url,&Headers::new()).await?;let base=response.url().clone();
        let mut body=response.bytes_stream();let mut bytes=Vec::new();
        while let Some(chunk)=body.next().await{ctx.check_cancelled()?;let chunk=chunk?;if bytes.len()+chunk.len()>4*1024*1024{anyhow::bail!("Trang HTML vượt 4 MiB");}bytes.extend_from_slice(&chunk);}
        let (title,media)={
            let document=Html::parse_document(&String::from_utf8_lossy(&bytes));
            let selector=options["selector"].as_str().unwrap_or("video[src], source[src], meta[property='og:video'], meta[property='og:video:url']");
            let selector=Selector::parse(selector).map_err(|e|anyhow::anyhow!("CSS selector không hợp lệ: {e}"))?;
            let source=document.select(&selector).find_map(|e|e.value().attr("src").or_else(||e.value().attr("content"))).context("Không tìm thấy video trong HTML. Trang dùng JavaScript hoặc API riêng cần module riêng")?.to_owned();
            let title_selector=Selector::parse("title").unwrap();
            let title=document.select(&title_selector).next().map(|e|e.text().collect::<String>()).unwrap_or_else(||"video".into());
            (title,base.join(&source)?.to_string())
        }; // Html is dropped before returning a Send future across any further await.
        let title:String=title.chars().filter(|c|!c.is_control()&&!"/\\:*?\"<>|".contains(*c)).take(80).collect();
        let mut title=title;while title.len()>220{title.pop();}
        let title=if title.trim().is_empty(){"video".to_string()}else{title.trim().to_string()};
        let mut headers=Headers::new();headers.insert("Referer".into(),base.to_string());
        let parsed=url::Url::parse(&media)?;
        if parsed.path().to_lowercase().ends_with(".m3u8") {Ok(DownloadPlan::Hls{url:media,filename:format!("{title}.mp4"),headers})}
        else {let ext=std::path::Path::new(parsed.path()).extension().and_then(|s|s.to_str()).filter(|s|s.len()<8).unwrap_or("mp4");Ok(DownloadPlan::Http{url:media,filename:Some(format!("{title}.{ext}")),headers})}
    }
}
