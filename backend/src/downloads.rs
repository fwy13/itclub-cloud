//! Persistent source requests and finite episode batches.
//! Source options are encrypted; only safe job metadata is returned to the UI.
use crate::{crypto, db::{self, Job, User}, error::{Error, Result}, modules::{DownloadPlan, ModuleContext, SourceItem}, remote, state::App, storage};
use axum::{extract::State, Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use tokio_util::sync::CancellationToken;

const MAX_BATCH: usize = 200;
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SavedSource {
    Single { entry: SourceItem },
    List { folder: String, entries: Vec<SourceItem> },
}
#[derive(Deserialize)]
pub struct BatchRequest {
    pub name: String,
    pub parent_id: Option<String>,
    pub entries: Vec<SourceItem>,
}

fn prepare_entry(app: &App, user: &User, mut entry: SourceItem) -> Result<SourceItem> {
    if entry.url.len() > 8192 { return Err(Error::bad("URL quá dài")); }
    let url = url::Url::parse(&entry.url).map_err(|_| Error::bad("URL không hợp lệ"))?;
    if !matches!(url.scheme(), "http" | "https" | "magnet") || !url.username().is_empty() || url.password().is_some() {
        return Err(Error::bad("Nguồn phải là HTTP/HTTPS công khai hoặc magnet, không kèm user/password"));
    }
    if !entry.options.is_object() || entry.options.to_string().len() > 32 * 1024 {
        return Err(Error::bad("Tùy chọn phải là JSON object không quá 32 KiB"));
    }
    if let Some(name) = &entry.filename { db::valid_name(name)?; }
    let module = app.modules.select(&entry.module, &entry.url)?;
    if module.info().admin_only { user.admin()?; }
    entry.module = module.info().id.to_owned();
    Ok(entry)
}
fn prepare_entries(app: &App, user: &User, entries: Vec<SourceItem>) -> Result<Vec<SourceItem>> {
    if entries.is_empty() || entries.len() > MAX_BATCH { return Err(Error::bad("Mỗi danh sách cần 1–200 tập")); }
    let mut seen = HashSet::new();
    let mut output = Vec::new();
    for entry in entries {
        let entry = prepare_entry(app, user, entry)?;
        let key = crypto::sha(serde_json::to_vec(&entry)?);
        if seen.insert(key) { output.push(entry); }
    }
    Ok(output)
}
fn source_label(url: &str) -> String {
    // Do not put signed query tokens into the public Job representation.
    match url::Url::parse(url) {
        Ok(mut value) => { value.set_query(None); value.set_fragment(None); value.to_string() },
        Err(_) => "Nguồn tải".into(),
    }
}
async fn save_source(app: &App, id: &str, source: &SavedSource) -> Result<()> {
    let encrypted = crypto::seal(&app.cfg.master_key, &serde_json::to_string(source)?)?;
    sqlx::query("INSERT INTO remote_sources(job_id,encrypted_config) VALUES(?,?)")
        .bind(id).bind(encrypted).execute(&app.db).await?;
    Ok(())
}
async fn load_source(app: &App, id: &str) -> Result<(SavedSource, bool)> {
    let (encrypted, ready): (String, i64) = sqlx::query_as("SELECT encrypted_config,staged_complete FROM remote_sources WHERE job_id=?")
        .bind(id).fetch_one(&app.db).await?;
    let text = crypto::open(&app.cfg.master_key, &encrypted)?;
    Ok((serde_json::from_str(&text)?, ready == 1))
}
pub async fn has_source(app: &App, id: &str) -> Result<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM remote_sources WHERE job_id=?)").bind(id).fetch_one(&app.db).await?)
}
pub async fn staged(app: &App, id: &str) -> Result<()> {
    sqlx::query("UPDATE remote_sources SET staged_complete=1 WHERE job_id=?").bind(id).execute(&app.db).await?;
    Ok(())
}
async fn job(app: &App, id: &str) -> Result<Job> {
    Ok(sqlx::query_as("SELECT * FROM jobs WHERE id=?").bind(id).fetch_one(&app.db).await?)
}
async fn owner(app: &App, id: &str) -> Result<User> {
    sqlx::query_as("SELECT * FROM users WHERE id=? AND disabled=0").bind(id).fetch_optional(&app.db).await?.ok_or_else(Error::forbidden)
}

pub async fn start(State(app): State<App>, Extension(user): Extension<User>, Json(request): Json<remote::Remote>) -> Result<Json<Value>> {
    let entry = prepare_entry(&app, &user, SourceItem {
        url: request.url,
        module: request.kind.unwrap_or_else(|| "auto".into()),
        filename: request.name.filter(|n| !n.trim().is_empty()),
        options: request.options.unwrap_or_else(|| json!({})),
    })?;
    let name = entry.filename.clone().unwrap_or_else(|| format!("{}-{}", entry.module, chrono::Utc::now().format("%Y%m%d-%H%M%S")));
    let new_job = storage::create_job(&app, &user, &name, request.parent_id, 0, "application/octet-stream", "queued", None).await?;
    save_source(&app, &new_job.id, &SavedSource::Single { entry: entry.clone() }).await?;
    sqlx::query("UPDATE jobs SET source=?,module_id=? WHERE id=?")
        .bind(source_label(&entry.url)).bind(&entry.module).bind(&new_job.id).execute(&app.db).await?;
    launch(app, new_job.id.clone()).await?;
    Ok(Json(json!({"id":new_job.id,"status":"queued"})))
}
pub async fn start_batch(State(app): State<App>, Extension(user): Extension<User>, Json(request): Json<BatchRequest>) -> Result<Json<Value>> {
    db::valid_name(&request.name)?;
    let entries = prepare_entries(&app, &user, request.entries)?;
    let new_job = storage::create_job(&app, &user, &request.name, request.parent_id, 0, "application/x-itclub-cloud-batch", "queued", None).await?;
    save_source(&app, &new_job.id, &SavedSource::List { folder: request.name, entries }).await?;
    sqlx::query("UPDATE jobs SET module_id='batch' WHERE id=?").bind(&new_job.id).execute(&app.db).await?;
    launch(app, new_job.id.clone()).await?;
    Ok(Json(json!({"id":new_job.id,"status":"queued"})))
}
pub async fn retry(app: &App, user: &User, id: &str) -> Result<()> {
    let task = storage::get_job(app, &user.id, id).await?;
    if !matches!(task.status.as_str(), "error" | "interrupted") { return Err(Error::bad("Tác vụ chưa cần thử lại")); }
    let children: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE parent_job_id=?").bind(id).fetch_one(&app.db).await?;
    if children > 0 && task.batch_total == 0 {
        return Err(Error::bad("Tác vụ công cụ ngoài đã nhập một phần nhiều file. Thử lại file con còn staging; không tự chạy lại cả nguồn để tránh tạo bản sao"));
    }
    launch(app.clone(), id.to_owned()).await
}
async fn launch(app: App, id: String) -> Result<()> {
    let task = job(&app, &id).await?;
    let child_ids: Vec<String> = sqlx::query_scalar("SELECT id FROM jobs WHERE parent_job_id=?").bind(&id).fetch_all(&app.db).await?;
    let token = CancellationToken::new();
    {
        let mut active = app.cancel.lock().await;
        if active.contains_key(&id) { return Err(Error::bad("Tác vụ đang chạy")); }
        if task.parent_job_id.as_ref().is_some_and(|p| active.contains_key(p)) || child_ids.iter().any(|id| active.contains_key(id)) {
            return Err(Error::bad("Đợi tác vụ cha/con kết thúc trước khi thử lại"));
        }
        active.insert(id.clone(), token.clone());
    }
    if let Err(error) = sqlx::query("UPDATE jobs SET status='queued',error=NULL,updated_at=? WHERE id=?").bind(db::now()).bind(&id).execute(&app.db).await {
        app.cancel.lock().await.remove(&id);
        return Err(error.into());
    }
    tokio::spawn(async move {
        let replay = matches!(task.status.as_str(), "error" | "interrupted");
        let result = run(&app, &id, &token, replay).await;
        finish(&app, &id, &token, result).await;
        app.cancel.lock().await.remove(&id);
        if let Some(parent) = task.parent_job_id { let _ = recount(&app, &parent, true).await; }
    });
    Ok(())
}
async fn finish(app: &App, id: &str, token: &CancellationToken, result: Result<()>) {
    if let Err(error) = result {
        let state = if app.stopping.is_cancelled() { "interrupted" } else if token.is_cancelled() { "cancelled" } else { "error" };
        app.progress(id, state, 0, 0, Some(&error.1)).await;
        if state == "cancelled" {
            if let Ok(parent)=job(app,id).await{let _=cancel_children(app,&parent).await;}
            let _ = storage::discard_staging(app, id).await;
        } else if state == "interrupted" {
            let _=sqlx::query("UPDATE jobs SET status='interrupted',updated_at=? WHERE parent_job_id=? AND status IN ('batch_pending','queued','resolving','downloading','segments','muxing','hashing','telegram','processing')").bind(db::now()).bind(id).execute(&app.db).await;
        }
    }
    let _ = tokio::fs::remove_dir_all(app.cfg.data.join("temp").join(format!("module-{id}"))).await;
}
async fn run(app: &App, id: &str, cancel: &CancellationToken, replay: bool) -> Result<()> {
    if cancel.is_cancelled(){return Err(Error::bad("Đã hủy"));}
    let task = job(app, id).await?;
    let user = owner(app, &task.owner).await?;
    if task.batch_total > 0 { return run_batch(app, &user, &task, cancel, replay).await; }
    let (saved, ready) = load_source(app, id).await?;
    let entry = match saved {
        SavedSource::List { folder, entries } => {
            if task.parent_job_id.is_some() { return Err(Error::bad("Không hỗ trợ batch lồng nhau")); }
            expand(app, &user, &task, &folder, entries).await?;
            return run_batch(app, &user, &job(app, id).await?, cancel, false).await;
        }
        SavedSource::Single { entry } => entry,
    };
    let slot = tokio::select! {
        _ = cancel.cancelled() => return Err(Error::bad("Đã hủy")),
        slot = app.remote_slots.clone().acquire_owned() => slot.map_err(anyhow::Error::from)?,
    };
    let entry = prepare_entry(app, &user, entry)?;
    if resume_staged(app, &task, ready, cancel).await? { return Ok(()); }
    storage::discard_staging(app, id).await?;
    let ctx = ModuleContext { app: app.clone(), job: task.clone(), cancel: cancel.clone() };
    app.progress(id, "resolving", 0, 0, None).await;
    let module = app.modules.select(&entry.module, &entry.url)?;
    let plan = tokio::select! {
        _ = cancel.cancelled() => return Err(Error::bad("Đã hủy")),
        plan = module.resolve(&ctx, &entry.url, &entry.options) => plan?,
    };
    match plan {
        DownloadPlan::Batch { folder, entries } => {
            if task.parent_job_id.is_some() { return Err(Error::bad("Module tập không được trả batch lồng nhau")); }
            expand(app, &user, &task, &folder, entries).await?;
            drop(slot); // Child jobs acquire their own slots; never hold one while waiting.
            run_batch(app, &user, &job(app, id).await?, cancel, false).await
        }
        plan => execute(app, &user, &task, &entry, plan, cancel).await,
    }
}
async fn resume_staged(app: &App, task: &Job, ready: bool, cancel: &CancellationToken) -> Result<bool> {
    if ready {
        if let Ok(meta) = tokio::fs::metadata(app.cfg.upload_path(&task.id)).await {
            if meta.len() == task.size as u64 {
                storage::upload(app, &task.id, cancel).await?;
                return Ok(true);
            }
        }
    }
    sqlx::query("UPDATE remote_sources SET staged_complete=0 WHERE job_id=?").bind(&task.id).execute(&app.db).await?;
    Ok(false)
}
fn filename(task: &Job, name: &str) -> Result<String> {
    db::valid_name(name)?;
    let mut name = if let Some(index) = task.batch_index { let prefix=format!("{index:03} - "); if name.starts_with(&prefix){name.to_owned()}else{format!("{prefix}{name}")} } else { name.to_owned() };
    while name.len() > 240 { name.pop(); }
    Ok(name)
}
async fn execute(app: &App, user: &User, task: &Job, entry: &SourceItem, plan: DownloadPlan, cancel: &CancellationToken) -> Result<()> {
    let ctx = ModuleContext { app: app.clone(), job: task.clone(), cancel: cancel.clone() };
    match plan {
        DownloadPlan::Http { url, filename: proposed, headers } => {
            let name = filename(task, entry.filename.as_deref().or(proposed.as_deref()).unwrap_or(&task.name))?;
            sqlx::query("UPDATE jobs SET name=? WHERE id=?").bind(&name).bind(&task.id).execute(&app.db).await?;
            let mut current = task.clone(); current.name = name;
            remote::download_url(app, user, &current, &url, &headers, cancel).await
        }
        DownloadPlan::Hls { url, filename: proposed, headers } => {
            let path = crate::modules::hls::download(&ctx, &url, &headers).await?;
            remote::accept_local(&ctx, &path, &filename(task, entry.filename.as_deref().unwrap_or(&proposed))?, "video/mp4").await
        }
        DownloadPlan::Local { path, filename: proposed, mime } => {
            remote::accept_local(&ctx, &path, &filename(task, entry.filename.as_deref().unwrap_or(&proposed))?, &mime).await
        }
        DownloadPlan::Telegram { url } => { user.admin()?; remote::download_telegram(app, user, task, &url, cancel).await }
        DownloadPlan::Tool { tool, url } => {
            user.admin()?;
            if !matches!(tool, "torrent" | "ytdlp") { return Err(Error::bad("Tool không được đăng ký")); }
            // Tool batches create their own file jobs; do not nest them in episode lists.
            if task.parent_job_id.is_some() { return Err(Error::bad("Danh sách tập hiện dùng HTTP/HLS/Local/Telegram; chạy yt-dlp/torrent thành tác vụ riêng")); }
            remote::external(app, user, task, tool, &url, cancel).await
        }
        DownloadPlan::Batch { .. } => Err(Error::bad("Không hỗ trợ batch lồng nhau")),
    }
}
async fn expand(app: &App, user: &User, parent: &Job, folder: &str, entries: Vec<SourceItem>) -> Result<()> {
    db::valid_name(folder)?;
    let entries = prepare_entries(app, user, entries)?;
    for entry in &entries {
        if matches!(entry.module.as_str(), "html-playlist" | "torrent" | "ytdlp") {
            return Err(Error::bad("Module tập cần trả một file, không dùng playlist/torrent/yt-dlp trong batch"));
        }
    }
    let lock = app.lock(format!("files:{}", user.id));
    let _guard = lock.lock().await;
    db::parent(&app.db, &user.id, &parent.parent_id).await?;
    let folder_id = db::id();
    let mut folder_name = folder.to_owned();
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes WHERE owner=? AND parent_id IS ? AND name=? AND deleted_at IS NULL)")
        .bind(&user.id).bind(&parent.parent_id).bind(&folder_name).fetch_one(&app.db).await?;
    if exists {
        while folder_name.len() > 220 { folder_name.pop(); }
        folder_name = format!("{}-{}", folder_name, &parent.id[..8]);
    }
    let mut tx = app.db.begin().await?;
    sqlx::query("INSERT INTO nodes(id,owner,parent_id,name,kind,created_at,updated_at) VALUES(?,?,?,?,'folder',?,?)")
        .bind(&folder_id).bind(&user.id).bind(&parent.parent_id).bind(folder_name).bind(db::now()).bind(db::now()).execute(&mut *tx).await?;
    for (index, entry) in entries.iter().enumerate() {
        let id = db::id();
        let name = entry.filename.clone().unwrap_or_else(|| format!("Episode {:03}", index + 1));
        let encrypted = crypto::seal(&app.cfg.master_key, &serde_json::to_string(&SavedSource::Single { entry: entry.clone() })?)?;
        sqlx::query("INSERT INTO jobs(id,owner,name,parent_id,size,mime,status,source,module_id,parent_job_id,batch_index,part_size,created_at,updated_at) VALUES(?,?,?,?,0,'application/octet-stream','batch_pending',?,?,?,?,?,?,?)")
            .bind(&id).bind(&user.id).bind(name).bind(&folder_id).bind(source_label(&entry.url)).bind(&entry.module)
            .bind(&parent.id).bind((index + 1) as i64).bind(app.cfg.part_size as i64).bind(db::now()).bind(db::now()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO remote_sources(job_id,encrypted_config) VALUES(?,?)").bind(&id).bind(encrypted).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE jobs SET node_id=?,batch_total=?,batch_done=0,status='batch',mime='application/x-itclub-cloud-batch' WHERE id=?")
        .bind(&folder_id).bind(entries.len() as i64).bind(&parent.id).execute(&mut *tx).await?;
    tx.commit().await?;
    app.changed(&user.id);
    Ok(())
}
async fn run_episode(app: &App, user: &User, task: &Job, cancel: &CancellationToken) -> Result<()> {
    let _slot = tokio::select! {
        _ = cancel.cancelled() => return Err(Error::bad("Đã hủy")),
        slot = app.remote_slots.clone().acquire_owned() => slot.map_err(anyhow::Error::from)?,
    };
    let (source, ready) = load_source(app, &task.id).await?;
    let SavedSource::Single { entry } = source else { return Err(Error::bad("Không hỗ trợ batch lồng nhau")); };
    let entry = prepare_entry(app, user, entry)?;
    if resume_staged(app, task, ready, cancel).await? { return Ok(()); }
    storage::discard_staging(app, &task.id).await?;
    let ctx = ModuleContext { app: app.clone(), job: task.clone(), cancel: cancel.clone() };
    app.progress(&task.id, "resolving", 0, 0, None).await;
    let module = app.modules.select(&entry.module, &entry.url)?;
    let plan = tokio::select! {
        _ = cancel.cancelled() => return Err(Error::bad("Đã hủy")),
        plan = module.resolve(&ctx, &entry.url, &entry.options) => plan?,
    };
    execute(app, user, task, &entry, plan, cancel).await
}
async fn run_batch(app: &App, user: &User, parent: &Job, cancel: &CancellationToken, replay: bool) -> Result<()> {
    if cancel.is_cancelled(){return Err(Error::bad("Đã hủy"));}
    if replay {
        sqlx::query("UPDATE jobs SET status='batch_pending',error=NULL WHERE parent_job_id=? AND status IN ('error','interrupted','cancelled')").bind(&parent.id).execute(&app.db).await?;
    }
    let children: Vec<Job> = sqlx::query_as("SELECT * FROM jobs WHERE parent_job_id=? ORDER BY batch_index,id").bind(&parent.id).fetch_all(&app.db).await?;
    app.progress(&parent.id, "batch", parent.batch_done as u64, parent.batch_total as u64, None).await;
    for snapshot in children {
        let child_lock=app.lock(format!("upload:{}",snapshot.id));
        let child_guard=child_lock.lock().await;
        let mut child=job(app,&snapshot.id).await?;
        if matches!(child.status.as_str(),"done"|"cancelled") { continue; }
        if cancel.is_cancelled() {
            let state = if app.stopping.is_cancelled() { "interrupted" } else { "cancelled" };
            sqlx::query("UPDATE jobs SET status=?,updated_at=? WHERE parent_job_id=? AND status!='done'").bind(state).bind(db::now()).bind(&parent.id).execute(&app.db).await?;
            return Err(Error::bad("Đã dừng danh sách tập; các tập hoàn tất được giữ nguyên"));
        }
        // Each child can be cancelled alone. Cancelling the parent propagates to it.
        let child_cancel = cancel.child_token();
        app.cancel.lock().await.insert(child.id.clone(), child_cancel.clone());
        let update = sqlx::query("UPDATE jobs SET status='queued',error=NULL,updated_at=? WHERE id=?").bind(db::now()).bind(&child.id).execute(&app.db).await;
        if let Err(error) = update { app.cancel.lock().await.remove(&child.id); return Err(error.into()); }
        child.status = "queued".into();
        drop(child_guard);
        let current_user = owner(app, &user.id).await;
        let result = match current_user { Ok(user) => run_episode(app, &user, &child, &child_cancel).await, Err(error) => Err(error) };
        finish(app, &child.id, &child_cancel, result).await;
        app.cancel.lock().await.remove(&child.id);
        recount(app, &parent.id, false).await?;
    }
    let (done, total) = recount(app, &parent.id, false).await?;
    if cancel.is_cancelled() { return Err(Error::bad("Đã dừng danh sách tập")); }
    if done != total { return Err(Error::bad(format!("Đã lưu {done}/{total} tập. Bấm thử lại để tiếp tục các tập chưa hoàn tất"))); }
    app.progress(&parent.id, "done", done as u64, total as u64, None).await;
    Ok(())
}
async fn recount(app: &App, parent: &str, complete_if_idle: bool) -> Result<(i64, i64)> {
    let (done, total): (i64, i64) = sqlx::query_as("SELECT COALESCE(SUM(CASE WHEN status='done' THEN 1 ELSE 0 END),0),COUNT(*) FROM jobs WHERE parent_job_id=?")
        .bind(parent).fetch_one(&app.db).await?;
    sqlx::query("UPDATE jobs SET batch_done=?,updated_at=? WHERE id=? AND batch_total>0").bind(done).bind(db::now()).bind(parent).execute(&app.db).await?;
    if complete_if_idle && total > 0 && done == total && !app.cancel.lock().await.contains_key(parent) {
        sqlx::query("UPDATE jobs SET status='done',progress=100,error=NULL WHERE id=? AND batch_total>0").bind(parent).execute(&app.db).await?;
    }
    let task = job(app, parent).await?;
    if task.batch_total > 0 { app.progress(parent, &task.status, done as u64, total as u64, task.error.as_deref()).await; }
    Ok((done, total))
}

/// Cancel a stopped parent and its outstanding children, without deleting a
/// staging file that an active child is still writing.
pub async fn cancel_children(app:&App,parent:&Job)->Result<()> {
    if parent.batch_total==0{return Ok(());}
    let children:Vec<Job>=sqlx::query_as("SELECT * FROM jobs WHERE parent_job_id=? AND status!='done'").bind(&parent.id).fetch_all(&app.db).await?;
    for child in children {
        let child_lock=app.lock(format!("upload:{}",child.id));let _guard=child_lock.lock().await;
        let token=app.cancel.lock().await.get(&child.id).cloned();
        if let Some(token)=token{token.cancel();}else{
            storage::discard_staging(app,&child.id).await?;
            app.progress(&child.id,"cancelled",0,0,None).await;
        }
    }
    Ok(())
}
