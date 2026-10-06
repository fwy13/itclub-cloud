use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::RngCore;
use std::{env, path::PathBuf};

#[derive(Clone)]
pub struct Config {
    pub data: PathBuf,
    pub bind: String,
    pub origin: String,
    pub frontend: PathBuf,
    pub tdlib: String,
    pub api_id: i64,
    pub api_hash: String,
    pub master_key: [u8; 32],
    pub setup_token: String,
    pub part_size: u64,
    pub max_upload: u64,
    pub max_parallel: usize,
    pub remote_parallel: usize,
    pub cache_bytes: u64,
    pub td_cache_bytes: u64,
    pub secure_cookie: bool,
    pub ffmpeg: String,
    pub ytdlp: String,
    pub aria2: String,
}
impl Config {
    pub fn load() -> Result<Self> {
        dotenvy::dotenv().ok();
        let data = PathBuf::from(get("DATA_DIR", "./data"));
        for dir in ["", "temp", "tdlib", "thumbs", "archives"] { std::fs::create_dir_all(data.join(dir))?; }
        let data = std::fs::canonicalize(data)?;
        let key_path = data.join("master.key");
        let key_text = match env::var("MASTER_KEY") {
            Ok(v) if !v.is_empty() => v,
            _ if key_path.exists() => std::fs::read_to_string(&key_path)?,
            _ => {
                let mut key = [0u8; 32]; rand::thread_rng().fill_bytes(&mut key);
                let value = STANDARD.encode(key);
                std::fs::write(&key_path, &value)?;
                #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))?; }
                value
            }
        };
        let master_key: [u8; 32] = STANDARD.decode(key_text.trim()).context("MASTER_KEY phải là 32 byte dạng Base64")?.try_into().map_err(|_| anyhow::anyhow!("MASTER_KEY phải đủ 32 byte"))?;
        let origin = get("PUBLIC_URL", "http://localhost:8091").trim_end_matches('/').to_string();
        let setup_token = env::var("SETUP_TOKEN").ok().filter(|x| !x.is_empty()).unwrap_or_else(crate::crypto::random_token);
        Ok(Self {
            data, bind: get("BIND", "0.0.0.0:8091"), frontend: get("FRONTEND_DIR", "../frontend/dist").into(),
            tdlib: get("TDLIB_PATH", if cfg!(target_os="windows") { "tdjson.dll" } else if cfg!(target_os="macos") { "libtdjson.dylib" } else { "libtdjson.so" }),
            api_id: get("TELEGRAM_API_ID", "0").parse()?, api_hash: get("TELEGRAM_API_HASH", ""),
            secure_cookie: origin.starts_with("https://"), origin, master_key, setup_token,
            part_size: number("PART_SIZE_MIB",256).clamp(16,1900)*1024*1024,
            max_upload: number("MAX_UPLOAD_GIB",50).clamp(1,1024)*1024*1024*1024,
            max_parallel: number("MAX_PARALLEL_UPLOADS",2).clamp(1,16) as usize,
            remote_parallel: number("MAX_PARALLEL_DOWNLOADS",2).clamp(1,8) as usize,
            cache_bytes: number("MEMORY_CACHE_MIB",64).clamp(1,1024)*1024*1024,
            td_cache_bytes: number("TDLIB_CACHE_MIB",1024).max(64)*1024*1024,
            ffmpeg:get("FFMPEG_PATH","ffmpeg"), ytdlp:get("YTDLP_PATH","yt-dlp"), aria2:get("ARIA2_PATH","aria2c"),
        })
    }
    pub fn upload_path(&self,id:&str)->PathBuf { self.data.join("temp").join(format!("{id}.upload")) }
}
fn get(k:&str,d:&str)->String { env::var(k).unwrap_or_else(|_|d.into()) }
fn number(k:&str,d:u64)->u64 { env::var(k).ok().and_then(|s|s.parse().ok()).unwrap_or(d) }
