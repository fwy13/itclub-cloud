use crate::{db::{self,User},error::{Error,Result},state::App,archives};
use axum::{extract::{State,Request},Extension,Json,response::Response};
use serde_json::{json,Value};
use std::{path::PathBuf,io::{Write,Seek},time::Duration};
use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

pub async fn create(app:&App)->Result<PathBuf> {
    let lock=app.lock("backup");let _guard=lock.lock().await;
    let temp=app.cfg.data.join("temp").join(format!("snapshot-{}.db",db::id()));
    let _snapshot=archives::RemoveOnDrop(temp.clone());
    let pages:i64=sqlx::query_scalar("PRAGMA page_count").fetch_one(&app.db).await?;
    let page_size:i64=sqlx::query_scalar("PRAGMA page_size").fetch_one(&app.db).await?;
    {
        let _disk=crate::disk::guard(app,(pages as u64).saturating_mul(page_size as u64).saturating_add(1024*1024)).await?;
        sqlx::query(&format!("VACUUM INTO '{}'",temp.to_string_lossy().replace('\'',"''"))).execute(&app.db).await?;
    }
    let output=app.cfg.data.join("temp").join(format!("backup-{}.zip",db::id()));
    let cleanup=archives::RemoveOnDrop(output.clone());
    let mut thumbnails=Vec::new();let mut reserved=tokio::fs::metadata(&temp).await?.len().saturating_mul(2).saturating_add(65536);
    let mut entries=tokio::fs::read_dir(app.cfg.data.join("thumbs")).await?;
    while let Some(entry)=entries.next_entry().await? {
        if !entry.file_type().await?.is_file(){continue;}
        let name=entry.file_name().to_string_lossy().to_string();if !name.ends_with(".jpg"){continue;}
        reserved=reserved.saturating_add(entry.metadata().await?.len().saturating_mul(2)).saturating_add(4096);
        thumbnails.push((name,entry.path()));
    }
    let out=crate::disk::create_sized(app,&output,reserved,false).await?.into_std().await;
    tokio::task::spawn_blocking(move||->anyhow::Result<()>{
        let _snapshot=_snapshot;let mut zip=zip::ZipWriter::new(out);let options=zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("itclub-cloud.db",options)?;std::io::copy(&mut std::fs::File::open(temp)?,&mut zip)?;
        zip.start_file("README.txt",options)?;zip.write_all(b"ITClub Cloud metadata backup. Keep master.key separately. TDLib sessions are not included.\n")?;
        for (name,path) in thumbnails {zip.start_file(format!("thumbs/{name}"),options)?;std::io::copy(&mut std::fs::File::open(path)?,&mut zip)?;}
        let mut file=zip.finish()?;let length=file.stream_position()?;file.set_len(length)?;Ok(())
    }).await.map_err(anyhow::Error::from)??;
    std::mem::forget(cleanup);Ok(output)
}

pub async fn download(State(app):State<App>,Extension(u):Extension<User>)->Result<Response>{u.admin()?;let path=create(&app).await?;archives::download_temp(path,&format!("itclub-cloud-backup-{}.zip",chrono::Utc::now().format("%Y%m%d-%H%M%S"))).await}
pub async fn telegram(State(app):State<App>,Extension(u):Extension<User>)->Result<Json<Value>>{u.admin()?;send(&app).await?;db::audit(&app.db,&u.id,"backup_telegram","").await;Ok(Json(json!({"ok":true})))}
pub async fn send(app:&App)->Result<()> {
    let path=create(app).await?;let _cleanup=archives::RemoveOnDrop(path.clone());let chat=app.tg.chat().await?;let account=app.tg.account("main").await?;
    if tokio::fs::metadata(&path).await?.len()>app.cfg.part_size{return Err(Error::bad("Backup quá lớn để gửi một file; hãy tải backup về máy"));}
    app.tg.send_document(&account,chat,&path,"ITClub Cloud · Metadata backup",&CancellationToken::new(),|_|async {}).await?;
    db::set(&app.db,"last_backup",&db::now().to_string()).await?;Ok(())
}
pub async fn restore(State(app):State<App>,Extension(u):Extension<User>,req:Request)->Result<Json<Value>> {
    u.admin()?;
    if req.headers().get("x-confirm-restore").and_then(|s|s.to_str().ok())!=Some("REPLACE"){return Err(Error::bad("Cần xác nhận thay thế dữ liệu"));}
    if !app.cancel.lock().await.is_empty(){return Err(Error::bad("Đợi hoặc hủy các tác vụ trước khi khôi phục"));}
    let input=app.cfg.data.join("temp").join(format!("restore-{}.zip",db::id()));let _cleanup=archives::RemoveOnDrop(input.clone());let mut f=tokio::fs::File::create(&input).await?;
    let mut stream=req.into_body().into_data_stream();let mut size=0usize;while let Some(b)=stream.next().await{let b=b.map_err(|e|Error::bad(e.to_string()))?;size+=b.len();if size>512*1024*1024{return Err(Error::bad("Backup upload giới hạn 512 MiB"));}crate::disk::write(&app,&mut f,&b,false).await?;}f.flush().await?;drop(f);
    let pending=app.cfg.data.join("restore.pending.db");let stage=app.cfg.data.join("temp").join(format!("restore-db-{}",db::id()));let cleanup=archives::RemoveOnDrop(stage.clone());
    let mut zip=tokio::task::spawn_blocking(move||->anyhow::Result<_>{Ok(zip::ZipArchive::new(std::fs::File::open(input)?)?)}).await.map_err(anyhow::Error::from)??;
    let database_name=if zip.file_names().any(|name|name=="itclub-cloud.db"){"itclub-cloud.db"}else{"telecloud.db"};
    let length=zip.by_name(database_name).map_err(anyhow::Error::from)?.size();
    if length>2*1024*1024*1024{return Err(Error::bad("Database backup quá lớn"));}
    let mut out=crate::disk::create_sized(&app,&stage,length,false).await?.into_std().await;
    tokio::task::spawn_blocking(move||->anyhow::Result<()>{let mut file=zip.by_name(database_name)?;let copied=std::io::copy(&mut file,&mut out)?;if copied!=length{anyhow::bail!("Database backup không đủ dữ liệu");}out.sync_all()?;Ok(())}).await.map_err(anyhow::Error::from)??;
    let connection=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect_with(sqlx::sqlite::SqliteConnectOptions::new().filename(&stage).read_only(true)).await?;
    let integrity:String=sqlx::query_scalar("PRAGMA integrity_check").fetch_one(&connection).await?;if integrity!="ok"{connection.close().await;return Err(Error::bad("Database backup không hợp lệ"));}
    let _:i64=sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE role='admin'").fetch_one(&connection).await?;
    let _:i64=sqlx::query_scalar("SELECT COUNT(*) FROM parts").fetch_one(&connection).await?;connection.close().await;
    tokio::fs::rename(stage,pending).await?;drop(cleanup);
    tokio::spawn(async move{tokio::time::sleep(Duration::from_secs(1)).await;app.shutdown.notify_one();});
    Ok(Json(json!({"ok":true,"message":"Database sẽ được thay thế khi tiến trình khởi động lại. Docker sẽ tự khởi động lại; chạy thủ công cần tự khởi động lại."})))
}
