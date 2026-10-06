//! Extensible download sources. Register another Rust module in Registry::new().
//! Modules resolve a page into a plan; the common worker owns quotas, cancellation,
//! disk staging, Telegram upload, progress, and database commits.
pub mod builtin;
pub mod html_video;
pub mod hls;
pub mod html_playlist;

use crate::{state::App,db::Job};
use anyhow::{Context,Result};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json,Value};
use std::{collections::BTreeMap,path::PathBuf,sync::Arc};
use tokio_util::sync::CancellationToken;

pub type Headers=BTreeMap<String,String>;
#[derive(Serialize)]
pub struct ModuleInfo {
    pub id:&'static str,
    pub name:&'static str,
    pub description:&'static str,
    pub admin_only:bool,
    /// JSON Schema, displayed as editable JSON in the client.
    pub options_schema:Value,
}
#[derive(Clone)]
pub struct ModuleContext {
    pub app:App,
    pub job:Job,
    pub cancel:CancellationToken,
}
impl ModuleContext {
    pub async fn progress(&self,stage:&str,done:u64,total:u64) {
        self.app.progress(&self.job.id,stage,done,total,None).await;
    }
    pub fn check_cancelled(&self)->Result<()> {
        if self.cancel.is_cancelled(){anyhow::bail!("Tác vụ đã hủy");}Ok(())
    }
    pub async fn work_dir(&self)->Result<PathBuf> {
        let dir=self.app.cfg.data.join("temp").join(format!("module-{}",self.job.id));
        tokio::fs::create_dir_all(&dir).await?;Ok(dir)
    }
    pub async fn get(&self,url:&str,headers:&Headers)->Result<reqwest::Response> {
        self.check_cancelled()?;
        tokio::select! {
            _=self.cancel.cancelled()=>anyhow::bail!("Tác vụ đã hủy"),
            response=crate::remote::safe_get_headers(url,headers)=>Ok(response?),
        }
    }
}

/// Persistable input for an episode. Resolve episode URLs when their jobs run,
/// so short-lived CDN URLs can be refreshed on retry.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SourceItem {
    pub url: String,
    #[serde(default = "auto_module")]
    pub module: String,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default = "empty_options")]
    pub options: Value,
}
fn auto_module() -> String { "auto".into() }
fn empty_options() -> Value { json!({}) }

pub enum DownloadPlan {
    /// A finite list (1..=200) of episode sources. Nested batches are rejected.
    Batch { folder: String, entries: Vec<SourceItem> },
    Http {url:String,filename:Option<String>,headers:Headers},
    Hls {url:String,filename:String,headers:Headers},
    Telegram {url:String},
    Tool {tool:&'static str,url:String},
    /// For site-specific processing: write output within ctx.work_dir(), then return it.
    /// The worker checks containment and file size before accepting it.
    Local {path:PathBuf,filename:String,mime:String},
}
#[async_trait]
pub trait DownloadModule:Send+Sync {
    fn info(&self)->ModuleInfo;
    /// Higher-priority modules are checked first; Direct HTTP is always last.
    fn matches(&self,url:&url::Url)->bool;
    async fn resolve(&self,ctx:&ModuleContext,url:&str,options:&Value)->Result<DownloadPlan>;
}
pub struct Registry {modules:Vec<Arc<dyn DownloadModule>>}
impl Registry {
    pub fn new()->Self {
        Self { modules:vec![
            Arc::new(builtin::TelegramSource),
            Arc::new(builtin::TorrentSource),
            Arc::new(builtin::HlsSource),
            Arc::new(html_playlist::HtmlPlaylist),
            Arc::new(html_video::HtmlVideo),
            Arc::new(builtin::YtDlpSource),
            // Add your site-specific source ABOVE DirectHttp.
            Arc::new(builtin::DirectHttp),
        ] }
    }
    pub fn list(&self,admin:bool)->Vec<ModuleInfo> {
        self.modules.iter().map(|m|m.info()).filter(|m|admin||!m.admin_only).collect()
    }
    pub fn select(&self,id:&str,url:&str)->Result<Arc<dyn DownloadModule>> {
        if id!="auto" {
            return self.modules.iter().find(|m|m.info().id==id).cloned().context("Module không tồn tại");
        }
        let parsed=url::Url::parse(url)?;
        self.modules.iter().find(|m|m.matches(&parsed)).cloned().context("Không có module hỗ trợ URL này")
    }
}
pub fn no_options()->Value {json!({"type":"object","properties":{},"additionalProperties":false})}
