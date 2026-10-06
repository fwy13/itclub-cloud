//! OPTIONAL TEMPLATE. Copy into backend/src/modules/anime_api.rs and register it.
//! The domain/API below is an illustrative contract, not a supported real provider.
//! Expected JSON: {"title":"Episode 1","streams":[{"quality":"1080p","url":"..."}]}.
use super::{DownloadModule, DownloadPlan, Headers, ModuleContext, ModuleInfo};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};

pub struct AnimeApi;

#[async_trait]
impl DownloadModule for AnimeApi {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            id: "anime-api",
            name: "Anime API",
            description: "Extractor cho API của website bạn quản lý",
            admin_only: false,
            options_schema: json!({
                "type": "object",
                "properties": {"quality": {"type": "string", "enum": ["720p", "1080p"], "default": "1080p"}},
                "additionalProperties": false
            }),
        }
    }

    fn matches(&self, url: &url::Url) -> bool {
        url.host_str() == Some("anime.example.com") && url.path().starts_with("/episode/")
    }

    async fn resolve(&self, ctx: &ModuleContext, source: &str, options: &Value) -> Result<DownloadPlan> {
        let page = url::Url::parse(source)?;
        if !self.matches(&page) { bail!("URL không thuộc website của module"); }
        let quality = options.get("quality").map(|v| v.as_str().context("quality phải là string")).transpose()?.unwrap_or("1080p");
        if !matches!(quality, "720p" | "1080p") { bail!("quality phải là 720p hoặc 1080p"); }
        let episode = page.path().strip_prefix("/episode/").context("Thiếu episode ID")?;
        if episode.is_empty() || !episode.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            bail!("Episode ID không hợp lệ");
        }
        let endpoint = page.join(&format!("/api/episodes/{episode}"))?;
        let response = ctx.get(endpoint.as_str(), &Headers::new()).await?;
        let mut stream = response.bytes_stream();
        let mut payload = Vec::new();
        while let Some(chunk) = stream.next().await {
            ctx.check_cancelled()?;
            let chunk = chunk?;
            if payload.len() + chunk.len() > 2 * 1024 * 1024 { bail!("API response quá lớn"); }
            payload.extend_from_slice(&chunk);
        }
        let data: Value = serde_json::from_slice(&payload)?;
        let streams = data["streams"].as_array().context("API thiếu streams")?;
        let selected = streams.iter().find(|v| v["quality"].as_str() == Some(quality))
            .or_else(|| streams.first()).context("Episode chưa có stream")?;
        let media = endpoint.join(selected["url"].as_str().context("Stream thiếu URL")?)?;
        let filename = format!("episode-{episode}.mp4");
        let mut headers = Headers::new();
        headers.insert("Referer".into(), page.to_string());
        if media.path().ends_with(".m3u8") {
            Ok(DownloadPlan::Hls { url: media.to_string(), filename, headers })
        } else {
            Ok(DownloadPlan::Http { url: media.to_string(), filename: Some(filename), headers })
        }
    }
}
