//! Generic episode-list extractor. No website-specific endpoints are assumed.
use super::*;
use futures_util::StreamExt;
use scraper::{Html, Selector};
use std::collections::HashSet;

pub struct HtmlPlaylist;

#[async_trait]
impl DownloadModule for HtmlPlaylist {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            id: "html-playlist",
            name: "Danh sách tập từ HTML",
            description: "Đọc liên kết các tập bằng CSS selector, tạo thư mục và tác vụ riêng cho từng tập",
            admin_only: false,
            options_schema: json!({
                "type": "object",
                "properties": {
                    "selector": {"type":"string", "default":".episodes a[href]", "description":"CSS selector cho liên kết tập"},
                    "episode_module": {"type":"string", "default":"html-video", "description":"Module xử lý từng trang tập"},
                    "episode_options": {"type":"object", "default":{}, "description":"Tùy chọn truyền cho module từng tập"},
                    "start": {"type":"integer", "minimum":1, "maximum":2000, "default":1},
                    "end": {"type":"integer", "minimum":1, "maximum":2000, "default":200},
                    "reverse": {"type":"boolean", "default":false},
                    "folder": {"type":"string", "description":"Tên thư mục, bỏ trống để dùng title trang"}
                },
                "additionalProperties": false
            }),
        }
    }

    // Explicit selection avoids mistaking a normal HTML page for a playlist.
    fn matches(&self, _: &url::Url) -> bool { false }

    async fn resolve(&self, ctx: &ModuleContext, url: &str, options: &Value) -> Result<DownloadPlan> {
        let object = options.as_object().context("Tùy chọn phải là object")?;
        let allowed = ["selector", "episode_module", "episode_options", "start", "end", "reverse", "folder"];
        if object.keys().any(|key| !allowed.contains(&key.as_str())) { anyhow::bail!("Tùy chọn html-playlist không được hỗ trợ"); }
        let string = |key: &str, default: &str| -> Result<String> {
            match options.get(key) {
                Some(v) => Ok(v.as_str().context(format!("{key} phải là string"))?.to_owned()),
                None => Ok(default.to_owned()),
            }
        };
        let selector = string("selector", ".episodes a[href]")?;
        let episode_module = string("episode_module", "html-video")?;
        if episode_module == "html-playlist" { anyhow::bail!("Không lồng playlist vào playlist"); }
        let episode_options = options.get("episode_options").cloned().unwrap_or_else(|| json!({}));
        if !episode_options.is_object() { anyhow::bail!("episode_options phải là object"); }
        let index = |key: &str, default: u64| -> Result<usize> {
            let value = options.get(key).map(|v| v.as_u64().context(format!("{key} phải là số nguyên dương"))).transpose()?.unwrap_or(default);
            if !(1..=2000).contains(&value) { anyhow::bail!("{key} phải trong khoảng 1..2000"); }
            Ok(value as usize)
        };
        let start = index("start", 1)?;
        let end = index("end", 200)?;
        if start > end { anyhow::bail!("Tập bắt đầu phải không lớn hơn tập kết thúc"); }
        if end - start + 1 > 200 { anyhow::bail!("Mỗi lần chọn tối đa 200 tập"); }
        let reverse = options.get("reverse").map(|v| v.as_bool().context("reverse phải là boolean")).transpose()?.unwrap_or(false);
        let response = ctx.get(url, &Headers::new()).await?;
        let base = response.url().clone();
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            ctx.check_cancelled()?;
            let chunk = chunk?;
            if bytes.len() + chunk.len() > 4 * 1024 * 1024 { anyhow::bail!("Trang danh sách vượt 4 MiB"); }
            bytes.extend_from_slice(&chunk);
        }
        let (title, mut links) = {
            let document = Html::parse_document(&String::from_utf8_lossy(&bytes));
            let selector = Selector::parse(&selector).map_err(|e| anyhow::anyhow!("CSS selector không hợp lệ: {e}"))?;
            let mut links = Vec::new();
            let mut seen = HashSet::new();
            for element in document.select(&selector) {
                let Some(href) = element.value().attr("href") else { continue; };
                let mut link = base.join(href)?;
                if !matches!(link.scheme(), "http" | "https") { continue; }
                link.set_fragment(None);
                if seen.insert(link.to_string()) { links.push(link.to_string()); }
                if links.len() > 2000 { anyhow::bail!("Selector trả quá nhiều link; hãy thu hẹp selector"); }
            }
            let title_selector = Selector::parse("title").unwrap();
            let title = document.select(&title_selector).next().map(|e| e.text().collect::<String>()).unwrap_or_else(|| "Episodes".into());
            (title, links)
        };
        if reverse { links.reverse(); }
        let selected = links.into_iter().enumerate().skip(start - 1).take(end - start + 1);
        let mut entries = Vec::new();
        for (_, url) in selected {
            entries.push(SourceItem {
                url,
                module: episode_module.clone(),
                // Use extractor filenames; the batch worker adds a stable episode prefix.
                filename: None,
                options: episode_options.clone(),
            });
        }
        if entries.is_empty() { anyhow::bail!("Không có tập trong khoảng đã chọn; kiểm tra CSS selector và thứ tự link"); }
        let requested = string("folder", "")?;
        let title = if requested.trim().is_empty() { title } else { requested };
        let mut folder: String = title.chars().filter(|c| !c.is_control() && !"/\\:*?\"<>|".contains(*c)).collect();
        folder = folder.trim().to_owned();
        while folder.len() > 180 { folder.pop(); }
        if folder.is_empty() { folder = "Episodes".into(); }
        Ok(DownloadPlan::Batch { folder, entries })
    }
}
