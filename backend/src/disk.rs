//! Application admission control for temporary files, separate from member quota.
//! DATA_DIR must be on one filesystem; filesystem quotas are still needed for
//! a hard ceiling covering TDLib, SQLite and external downloader processes.
use crate::{error::{Error, Result}, state::App};
use axum::http::StatusCode;
use fs2::FileExt;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

#[derive(Serialize)]
pub struct Usage {
    pub temporary_bytes: u64,
    pub temporary_limit_bytes: u64,
    pub available_bytes: u64,
    pub minimum_free_bytes: u64,
    pub upload_headroom_bytes: u64,
    pub tdlib_cache_bytes_per_account: u64,
}

fn tree_size(root: &Path) -> std::io::Result<u64> {
    let mut size = 0u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path)? { pending.push(entry?.path()); }
        } else if metadata.is_file() { size = size.saturating_add(metadata.len()); }
        // Never follow symlinks outside DATA_DIR.
    }
    Ok(size)
}

pub async fn usage(app: &App) -> Result<Usage> {
    let cfg = app.cfg.clone();
    tokio::task::spawn_blocking(move || -> Result<Usage> {
        Ok(Usage {
            temporary_bytes: tree_size(&cfg.data.join("temp"))?.saturating_add(tree_size(&cfg.data.join("archives"))?),
            temporary_limit_bytes: cfg.ssd_temp_limit,
            available_bytes: fs2::available_space(&cfg.data)?,
            minimum_free_bytes: cfg.ssd_min_free,
            upload_headroom_bytes: cfg.part_size.saturating_mul(cfg.max_parallel as u64),
            tdlib_cache_bytes_per_account: cfg.td_cache_bytes,
        })
    }).await.map_err(anyhow::Error::from)?
}

fn ensure(usage: &Usage, growth: u64, headroom: u64) -> Result<()> {
    let required = growth.saturating_add(headroom);
    if usage.temporary_limit_bytes > 0 && usage.temporary_bytes.saturating_add(required) > usage.temporary_limit_bytes {
        return Err(Error(StatusCode::INSUFFICIENT_STORAGE,
            "Đã chạm hạn mức SSD tạm. Đợi upload hoàn tất hoặc hủy tác vụ không cần thiết, rồi thử lại.".into()));
    }
    if usage.available_bytes < usage.minimum_free_bytes.saturating_add(required) {
        return Err(Error(StatusCode::INSUFFICIENT_STORAGE,
            "SSD không còn đủ dung lượng an toàn. Hãy giải phóng ổ đĩa hoặc giảm kích thước file.".into()));
    }
    Ok(())
}

pub async fn check(app: &App) -> Result<()> { ensure(&usage(app).await?, 0, 0) }

/// TDLib owns its cache writes. Check free space before requesting more data;
/// this is admission only, not a filesystem reservation for native writers.
pub async fn require_free(app: &App, bytes: u64) -> Result<()> {
    let data=app.cfg.data.clone();let minimum=app.cfg.ssd_min_free;
    let available=tokio::task::spawn_blocking(move || fs2::available_space(data)).await.map_err(anyhow::Error::from)??;
    if available < minimum.saturating_add(bytes) {
        return Err(Error(StatusCode::INSUFFICIENT_STORAGE,"SSD không đủ chỗ trống để tải thêm dữ liệu từ Telegram.".into()));
    }
    Ok(())
}

pub async fn guard(app: &App, growth: u64) -> Result<tokio::sync::OwnedMutexGuard<()>> {
    let guard = app.lock("disk-budget").lock_owned().await;
    ensure(&usage(app).await?, growth, 0)?;
    Ok(guard)
}

/// Allocate real disk blocks before accepting a known-size upload. Unlike
/// set_len, this does not let sparse files overcommit the available disk.
pub async fn create_sized(app: &App, path: &Path, size: u64, incoming: bool) -> Result<tokio::fs::File> {
    let guard = app.lock("disk-budget").lock_owned().await;
    match tokio::fs::remove_file(path).await {
        Ok(()) => {}, Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}, Err(e) => return Err(e.into()),
    }
    let current = usage(app).await?;
    ensure(&current, size, if incoming { current.upload_headroom_bytes } else { 0 })?;
    let path: PathBuf = path.into();
    let file = tokio::task::spawn_blocking(move || -> Result<std::fs::File> {
        let _guard = guard;
        let file = std::fs::OpenOptions::new().create_new(true).read(true).write(true).open(&path)?;
        if let Err(e) = if size > 0 { file.allocate(size) } else { Ok(()) } {
            drop(file); let _ = std::fs::remove_file(&path);
            return Err(Error(StatusCode::INSUFFICIENT_STORAGE, format!("Không cấp phát được file tạm trên SSD: {e}")));
        }
        Ok(file)
    }).await.map_err(anyhow::Error::from)??;
    Ok(tokio::fs::File::from_std(file))
}

/// Serialize admission + writes for streams whose final size is unknown.
pub async fn write(app: &App, file: &mut tokio::fs::File, bytes: &[u8], incoming: bool) -> Result<()> {
    let _guard = app.lock("disk-budget").lock_owned().await;
    let current = usage(app).await?;
    ensure(&current, bytes.len() as u64, if incoming { current.upload_headroom_bytes } else { 0 })?;
    file.write_all(bytes).await?;
    file.flush().await?; // Make the new size visible before releasing admission.
    Ok(())
}

pub async fn copy(app: &App, source: &Path, destination: &Path, incoming: bool) -> Result<()> {
    let size = tokio::fs::metadata(source).await?.len();
    let mut output = create_sized(app, destination, size, incoming).await?;
    let mut input = tokio::fs::File::open(source).await?;
    let copied = tokio::io::copy(&mut input.take(size), &mut output).await?;
    output.sync_all().await?;
    if copied != size { return Err(Error::bad("File nguồn thay đổi trong khi sao chép")); }
    Ok(())
}
use tokio::io::AsyncReadExt;
