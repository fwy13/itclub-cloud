use crate::{crypto,db::{self,User},error::Result,state::App,storage,telegram::media_file};
use axum::{extract::State,Extension,Json};
use serde_json::{json,Value};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub async fn link_code(State(app):State<App>,Extension(u):Extension<User>)->Result<Json<Value>> {
    let code=crypto::random_token();sqlx::query("DELETE FROM link_codes WHERE user_id=?").bind(&u.id).execute(&app.db).await?;
    sqlx::query("INSERT INTO link_codes(token_hash,user_id,expires_at) VALUES(?,?,?)").bind(crypto::sha(&code)).bind(&u.id).bind(db::now()+600).execute(&app.db).await?;
    Ok(Json(json!({"command":format!("/link {code}"),"expires_in":600})))
}
pub async fn unlink(State(app):State<App>,Extension(u):Extension<User>)->Result<Json<Value>> {
    sqlx::query("UPDATE users SET telegram_user_id=NULL WHERE id=?").bind(&u.id).execute(&app.db).await?;
    sqlx::query("DELETE FROM link_codes WHERE user_id=?").bind(&u.id).execute(&app.db).await?;Ok(Json(json!({"ok":true})))
}
pub fn start(app:App) {
    let Some(hub)=app.tg.hub.as_ref()else{return;};let mut events=hub.events.subscribe();
    tokio::spawn(async move {loop {tokio::select!{
        _=app.stopping.cancelled()=>break,
        event=events.recv()=>{
            let value=match event{Ok(v)=>v,Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>continue,Err(_)=>break};
            if value["@type"]!="updateNewMessage"{continue;}let client=value["@client_id"].as_i64().unwrap_or(0);
            let account=app.tg.accounts.read().await.values().find(|a|a.bot&&a.client as i64==client).cloned();let Some(account)=account else{continue;};
            let message=value["message"].clone();if message["is_outgoing"]==true{continue;}
            let sender=message["sender_id"]["user_id"].as_i64().unwrap_or(0);if sender<=0||message["chat_id"].as_i64()!=Some(sender){continue;}
            // Only direct messages from explicitly linked users are imported.
            let cloned=app.clone();tokio::spawn(async move{if let Err(e)=receive(cloned,&account.key,sender,message).await{tracing::warn!(error=%e,"Inbox import failed");}});
        }
    }}});
}
async fn reply(app:&App,account:&str,chat:i64,text:&str){let _=app.tg.call(account,json!({"@type":"sendMessage","chat_id":chat,"input_message_content":{"@type":"inputMessageText","text":{"@type":"formattedText","text":text,"entities":[]},"link_preview_options":{"@type":"linkPreviewOptions","is_disabled":true},"clear_draft":false}})).await;}
async fn receive(app:App,account:&str,sender:i64,message:Value)->anyhow::Result<()> {
    let text=message["content"]["text"]["text"].as_str().unwrap_or("");
    if let Some(code)=text.trim().strip_prefix("/link ") {
        let lock=app.lock("link-code");let _guard=lock.lock().await;
        let owner:Option<String>=sqlx::query_scalar("SELECT user_id FROM link_codes WHERE token_hash=? AND expires_at>?").bind(crypto::sha(code.trim())).bind(db::now()).fetch_optional(&app.db).await?;
        if let Some(owner)=owner {
            let mut tx=app.db.begin().await?;sqlx::query("UPDATE users SET telegram_user_id=? WHERE id=?").bind(sender).bind(&owner).execute(&mut *tx).await?;sqlx::query("DELETE FROM link_codes WHERE user_id=?").bind(&owner).execute(&mut *tx).await?;tx.commit().await?;
            reply(&app,account,sender,"Đã liên kết với ITClub Cloud. Gửi tài liệu/video cho bot để đưa vào thư mục gốc của bạn.").await;
        }else{reply(&app,account,sender,"Mã liên kết không đúng hoặc đã hết hạn.").await;}return Ok(());
    }
    let Some(file)=media_file(&message)else{return Ok(());};
    let user:Option<User>=sqlx::query_as("SELECT * FROM users WHERE telegram_user_id=? AND disabled=0").bind(sender).fetch_optional(&app.db).await?;let Some(u)=user else{return Ok(());};
    let message_id=message["id"].as_i64().unwrap_or(0);let claimed=sqlx::query("INSERT OR IGNORE INTO inbox_receipts(account,chat_id,message_id) VALUES(?,?,?)").bind(account).bind(sender).bind(message_id).execute(&app.db).await?;if claimed.rows_affected()==0{return Ok(());}
    let size=file["size"].as_i64().unwrap_or(0);let fid=file["id"].as_i64().unwrap_or(0);
    let content=&message["content"];let fallback=format!("telegram-{message_id}.bin");
    let name=content["document"]["file_name"].as_str().or_else(||content["video"]["file_name"].as_str()).or_else(||content["audio"]["file_name"].as_str()).filter(|s|!s.is_empty()).unwrap_or(&fallback);
    let mime=mime_guess::from_path(name).first_or_octet_stream().to_string();
    let job=storage::create_job(&app,&u,name,None,size,&mime,"downloading",None).await?;
    sqlx::query("UPDATE inbox_receipts SET job_id=? WHERE account=? AND chat_id=? AND message_id=?").bind(&job.id).bind(account).bind(sender).bind(message_id).execute(&app.db).await?;
    let cancel=CancellationToken::new();app.cancel.lock().await.insert(job.id.clone(),cancel.clone());
    let result:crate::error::Result<()>=async {
        let _slot=app.downloads.clone().acquire_owned().await.map_err(anyhow::Error::from)?;
        crate::disk::require_free(&app,size.max(0) as u64).await?;
        app.tg.call(account,json!({"@type":"downloadFile","file_id":fid,"priority":8,"offset":0,"limit":0,"synchronous":false})).await?;
        let path=loop{
            if let Err(e)=crate::disk::check(&app).await{let _=app.tg.call(account,json!({"@type":"cancelDownloadFile","file_id":fid,"only_if_pending":false})).await;return Err(e);}
            if cancel.is_cancelled(){let _=app.tg.call(account,json!({"@type":"cancelDownloadFile","file_id":fid,"only_if_pending":false})).await;return Err(crate::error::Error::bad("Đã hủy"));}
            let f=app.tg.call(account,json!({"@type":"getFile","file_id":fid})).await?;
            app.progress(&job.id,"downloading",f["local"]["downloaded_size"].as_u64().unwrap_or(0),size as u64,None).await;
            if f["local"]["is_downloading_completed"]==true{break f["local"]["path"].as_str().unwrap_or("").to_owned();}
            tokio::time::sleep(Duration::from_secs(1)).await;
        };
        crate::disk::copy(&app,std::path::Path::new(&path),&app.cfg.upload_path(&job.id),true).await?;drop(_slot);
        storage::upload(&app,&job.id,&cancel).await?;Ok(())
    }.await;
    match result{Ok(())=>reply(&app,account,sender,&format!("Đã lưu: {name}")).await,Err(e)=>{app.progress(&job.id,"error",0,0,Some(&e.1)).await;reply(&app,account,sender,"Không lưu được file. Xem chi tiết trong trang Tác vụ của ITClub Cloud.").await;}}
    app.cancel.lock().await.remove(&job.id);Ok(())
}
