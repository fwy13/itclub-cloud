//! OPTIONAL TEMPLATE: copy into backend/src/modules/anime_playlist.rs.
//! This demonstrates module composition; selectors and example domain must be
//! adapted to the actual website. It is not registered by default.
use super::{DownloadModule, DownloadPlan, ModuleContext, ModuleInfo};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct AnimePlaylist;

#[async_trait]
impl DownloadModule for AnimePlaylist {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            id: "my-anime-playlist",
            name: "My anime — danh sách tập",
            description: "Extractor danh sách cho website của bạn",
            admin_only: false,
            options_schema: json!({
                "type":"object",
                "properties":{
                    "start":{"type":"integer","minimum":1,"default":1},
                    "end":{"type":"integer","minimum":1,"default":12}
                },
                "additionalProperties":false
            }),
        }
    }
    fn matches(&self, url: &url::Url) -> bool {
        url.host_str() == Some("anime.example.com") && url.path().starts_with("/series/")
    }
    async fn resolve(&self, ctx: &ModuleContext, url: &str, options: &Value) -> Result<DownloadPlan> {
        if !self.matches(&url::Url::parse(url)?) { anyhow::bail!("Sai tên miền/loại URL"); }
        // Reuse the bounded HTML parser, range selection and Batch plan builder.
        // Register the AnimeApi sample as "anime-api" before using this template.
        let delegate = super::html_playlist::HtmlPlaylist;
        delegate.resolve(ctx, url, &json!({
            "selector": ".episode-list a[href]",
            "episode_module": "anime-api",
            "episode_options": {"quality":"1080p"},
            "start": options.get("start").cloned().unwrap_or(json!(1)),
            "end": options.get("end").cloned().unwrap_or(json!(12)),
            "reverse": false
        })).await
    }
}
