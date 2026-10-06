mod archives;
mod auth;
mod backup;
mod config;
mod crypto;
mod db;
mod downloads;
mod error;
mod files;
mod inbox;
mod modules;
mod passkey;
mod remote;
mod s3;
mod settings;
mod shares;
mod state;
mod storage;
mod tdlib;
mod telegram;
mod webdav;

use axum::{Router,routing::{get,post,put,delete,any},middleware,extract::{State,Request,DefaultBodyLimit},body::Body,response::{Response,IntoResponse},http::StatusCode,Json};
use state::App;
use serde_json::json;
use std::{net::SocketAddr,time::Duration};
use tower::ServiceExt;

#[tokio::main]
async fn main()->anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_|"itclub_cloud=info,tower_http=info".into())).init();
    let cfg=config::Config::load()?;
    let current_database = cfg.data.join("itclub-cloud.db");
    let legacy_database = cfg.data.join("telecloud.db");
    if current_database.exists() && legacy_database.exists() {
        anyhow::bail!("DATA_DIR có cả itclub-cloud.db và telecloud.db; chọn đúng database trước khi khởi động");
    }
    let database = if legacy_database.exists() { legacy_database } else { current_database };
    let pending=cfg.data.join("restore.pending.db");
    if pending.exists(){
        if database.exists(){let old=db::connect(&database).await?;sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)").execute(&old).await?;old.close().await;
            tokio::fs::rename(&database,cfg.data.join(format!("before-restore-{}.db",db::now()))).await?;}
        tokio::fs::rename(pending,&database).await?;
    }
    let pool=db::connect(&database).await?;
    let user_count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(&pool).await?;
    if user_count==0{tracing::warn!("Thiết lập lần đầu tại {} · SETUP_TOKEN={}",cfg.origin,cfg.setup_token);}
    sqlx::query("UPDATE jobs SET status='interrupted',error='Server đã khởi động lại. Bấm thử lại nếu file tạm còn đủ dữ liệu.' WHERE status IN ('queued','hashing','telegram','downloading','processing','segments','muxing','resolving','batch')").execute(&pool).await?;
    let tg=telegram::Telegram::new(cfg.clone(),pool.clone());let app=state::State::new(cfg.clone(),pool,tg.clone())?;
    if let Err(e)=tg.boot().await{tracing::warn!(error=%e,"Telegram chờ cấu hình/đăng nhập; web vẫn hoạt động");}
    inbox::start(app.clone());background(app.clone());
    let public=Router::new()
        .route("/healthz",get(||async{Json(json!({"ok":true,"service":"itclub-cloud"}))}))
        .route("/api/auth/status",get(auth::status))
        .route("/api/auth/setup",post(auth::setup))
        .route("/api/auth/login",post(auth::login))
        .route("/api/auth/logout",post(auth::logout))
        .route("/api/auth/passkey/begin",post(passkey::login_begin))
        .route("/api/auth/passkey/finish",post(passkey::login_finish))
        .route("/api/public/shares/{token}",get(shares::public_info))
        .route("/api/public/shares/{token}/unlock",post(shares::unlock))
        .route("/api/public/shares/{token}/stream",get(shares::public_stream).head(shares::public_stream))
        .route("/webdav",any(webdav::handle)).route("/webdav/",any(webdav::handle)).route("/webdav/{*path}",any(webdav::handle))
        .route("/s3",any(s3::handle)).route("/s3/",any(s3::handle)).route("/s3/{*path}",any(s3::handle));
    let protected=Router::new()
        .route("/api/nodes",get(files::list))
        .route("/api/folders",post(files::mkdir))
        .route("/api/nodes/{id}",put(files::edit).delete(files::trash))
        .route("/api/nodes/{id}/copy",post(files::copy))
        .route("/api/nodes/{id}/restore",post(files::restore))
        .route("/api/nodes/{id}/permanent",delete(files::purge))
        .route("/api/nodes/{id}/stream",get(files::stream).head(files::stream))
        .route("/api/nodes/{id}/thumbnail",get(files::thumb))
        .route("/api/nodes/{id}/zip",get(archives::folder_zip))
        .route("/api/nodes/{id}/archive",get(archives::list))
        .route("/api/nodes/{id}/archive/resource",get(archives::resource))
        .route("/api/trash",delete(files::empty_trash))
        .route("/api/uploads",post(files::begin_upload))
        .route("/api/uploads/{id}",get(files::upload_status))
        .route("/api/uploads/{id}/chunks/{index}",put(files::chunk))
        .route("/api/uploads/{id}/complete",post(files::complete))
        .route("/api/jobs",get(files::jobs))
        .route("/api/jobs/{id}",delete(files::cancel))
        .route("/api/jobs/{id}/retry",post(files::retry))
        .route("/api/jobs/{id}/children",get(files::children))
        .route("/api/modules",get(remote::modules))
        .route("/api/remote",post(remote::start))
        .route("/api/remote/batch",post(downloads::start_batch))
        .route("/api/nodes/{id}/shares",post(shares::create))
        .route("/api/shares",get(shares::list))
        .route("/api/shares/{token}",delete(shares::revoke))
        .route("/api/settings",get(settings::get).put(settings::save))
        .route("/api/settings/telegram",get(settings::telegram_status).post(settings::telegram_config))
        .route("/api/settings/telegram/auth",post(settings::telegram_auth))
        .route("/api/settings/telegram/chats",get(settings::chats))
        .route("/api/settings/bots",post(settings::add_bot))
        .route("/api/settings/bots/{id}",delete(settings::remove_bot))
        .route("/api/profile/password",post(auth::change_password))
        .route("/api/profile/keys",post(auth::keys))
        .route("/api/profile/telegram/link",post(inbox::link_code).delete(inbox::unlink))
        .route("/api/profile/passkeys",get(passkey::list))
        .route("/api/profile/passkeys/begin",post(passkey::register_begin))
        .route("/api/profile/passkeys/finish",post(passkey::register_finish))
        .route("/api/profile/passkeys/{id}",delete(passkey::remove))
        .route("/api/admin/users",get(auth::list_users).post(auth::create_user))
        .route("/api/admin/users/{id}",put(auth::edit_user).delete(files::delete_user))
        .route("/api/admin/audit",get(settings::audit))
        .route("/api/admin/backup",get(backup::download))
        .route("/api/admin/backup/telegram",post(backup::telegram))
        .route("/api/admin/restore",post(backup::restore))
        .route("/api/ws",get(settings::websocket))
        .route_layer(middleware::from_fn_with_state(app.clone(),auth::require));
    let router=Router::new().merge(public).merge(protected).fallback(spa)
        .layer(DefaultBodyLimit::max(2*1024*1024))
        .layer(middleware::from_fn_with_state(app.clone(),auth::origin_guard))
        .with_state(app.clone());
    let listener=tokio::net::TcpListener::bind(&cfg.bind).await?;
    tracing::info!(address=%cfg.bind,"ITClub Cloud đang lắng nghe");
    let shutdown_app=app.clone();
    axum::serve(listener,router.into_make_service_with_connect_info::<SocketAddr>()).with_graceful_shutdown(async move{
        shutdown_signal(&shutdown_app).await;shutdown_app.stopping.cancel();
        for token in shutdown_app.cancel.lock().await.values(){token.cancel();}
    }).await?;
    tg.stop().await;
    let _=sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)").execute(&app.db).await;app.db.close().await;Ok(())
}
async fn spa(State(app):State<App>,req:Request)->Response{
    if req.uri().path().starts_with("/api/"){return (StatusCode::NOT_FOUND,Json(json!({"error":"API không tồn tại"}))).into_response();}
    let service=tower_http::services::ServeDir::new(&app.cfg.frontend).not_found_service(tower_http::services::ServeFile::new(app.cfg.frontend.join("index.html")));
    match service.oneshot(req).await{Ok(response)=>response.map(Body::new),Err(_)=>StatusCode::INTERNAL_SERVER_ERROR.into_response()}
}
async fn shutdown_signal(app:&App){
    #[cfg(unix)]{let mut term=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");tokio::select!{_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{},_=app.shutdown.notified()=>{}}}
    #[cfg(not(unix))]{tokio::select!{_=tokio::signal::ctrl_c()=>{},_=app.shutdown.notified()=>{}}}
}
fn background(app:App){tokio::spawn(async move{
    let mut tick=tokio::time::interval(Duration::from_secs(600));
    loop{tokio::select!{_=app.stopping.cancelled()=>break,_=tick.tick()=>{}}
        for sql in ["DELETE FROM sessions WHERE expires_at<?","DELETE FROM share_sessions WHERE expires_at<?","DELETE FROM link_codes WHERE expires_at<?","DELETE FROM dav_locks WHERE expires_at<?"]{let _=sqlx::query(sql).bind(db::now()).execute(&app.db).await;}
        let _=sqlx::query("DELETE FROM audit WHERE created_at<?").bind(db::now()-90*86400).execute(&app.db).await;
        let stale:Vec<String>=sqlx::query_scalar("SELECT id FROM multipart WHERE created_at<?").bind(db::now()-86400).fetch_all(&app.db).await.unwrap_or_default();for id in stale{let _=s3::remove_multipart(&app,&id).await;}
        if app.tg.ready().await {
            storage::collect_garbage(&app).await;
            let last=db::setting(&app.db,"last_backup").await.and_then(|s|s.parse::<i64>().ok()).unwrap_or(0);
            if db::setting(&app.db,"backup_enabled").await.as_deref()==Some("true")&&db::now()-last>86400 {if let Err(e)=backup::send(&app).await{tracing::warn!(error=%e,"Automatic backup failed");}}
            let accounts:Vec<_>=app.tg.accounts.read().await.keys().cloned().collect();
            for key in accounts{let _=app.tg.call(&key,json!({"@type":"optimizeStorage","size":app.cfg.td_cache_bytes,"ttl":86400,"count":10000,"immunity_delay":3600,"file_types":[],"chat_ids":[],"exclude_chat_ids":[],"return_deleted_file_statistics":false,"chat_limit":0})).await;}
        }
    }
});}
