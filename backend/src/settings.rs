use crate::{auth,crypto,db::{self,User},error::{Error,Result},state::App};
use axum::{extract::{State,Path,WebSocketUpgrade},extract::ws::{Message,WebSocket},Extension,Json,response::Response};
use serde_json::{json,Value};
use futures_util::{SinkExt,StreamExt};
use std::time::Duration;

pub async fn get(State(app):State<App>,Extension(u):Extension<User>)->Result<Json<Value>> {
    let api_enabled:bool=sqlx::query_scalar("SELECT api_hash IS NOT NULL FROM users WHERE id=?").bind(&u.id).fetch_one(&app.db).await?;
    let access:Option<String>=sqlx::query_scalar("SELECT s3_access FROM users WHERE id=?").bind(&u.id).fetch_one(&app.db).await?;
    let mut value=json!({"user":u,"public_url":app.cfg.origin,"api_enabled":api_enabled,"s3_access_key":access,"max_upload_bytes":app.cfg.max_upload,"part_size":app.cfg.part_size});
    if u.role=="admin" {
        value["disk"]=json!(crate::disk::usage(&app).await?);
        value["telegram"]=app.tg.status().await;
        value["max_parallel_uploads"]=json!(app.cfg.max_parallel);
        value["storage_chat"]=json!(db::setting(&app.db,"storage_chat").await.unwrap_or_else(||"me".into()));
        value["telegram_api_id"]=json!(app.tg.credentials().await.map(|c|c.0).unwrap_or(0));
        value["backup_enabled"]=json!(db::setting(&app.db,"backup_enabled").await.as_deref()==Some("true"));
        value["last_backup"]=json!(db::setting(&app.db,"last_backup").await);
        let bots:Vec<(String,String)>=sqlx::query_as("SELECT id,name FROM bot_accounts").fetch_all(&app.db).await?;value["bots"]=json!(bots.into_iter().map(|(id,name)|json!({"id":id,"name":name})).collect::<Vec<_>>());
        let gc:i64=sqlx::query_scalar("SELECT COUNT(*) FROM gc_messages").fetch_one(&app.db).await?;value["pending_deletions"]=json!(gc);
    }
    Ok(Json(value))
}
pub async fn telegram_status(State(app):State<App>,Extension(u):Extension<User>)->Result<Json<Value>> {u.admin()?;Ok(Json(app.tg.status().await))}
pub async fn telegram_config(State(app):State<App>,Extension(u):Extension<User>,Json(p):Json<Value>)->Result<Json<Value>> {
    u.admin()?;
    if let Some(id)=p["api_id"].as_i64(){if id<=0{return Err(Error::bad("API ID không hợp lệ"));}db::set(&app.db,"tg_api_id",&id.to_string()).await?;}
    if let Some(hash)=p["api_hash"].as_str().filter(|s|!s.is_empty()){if hash.len()!=32||!hash.bytes().all(|c|c.is_ascii_hexdigit()){return Err(Error::bad("API Hash cần 32 ký tự hex"));}db::set(&app.db,"tg_api_hash",&crypto::seal(&app.cfg.master_key,hash)?).await?;}
    if let Ok(account)=app.tg.account("main").await {
        if !app.cancel.lock().await.is_empty(){return Err(Error::bad("Đợi hoặc hủy tác vụ trước khi khởi tạo lại Telegram"));}
        if account.state.read().await["@type"]!="authorizationStateClosed" {
            app.tg.call("main",json!({"@type":"close"})).await?;
            tokio::time::timeout(Duration::from_secs(20),async{while account.state.read().await["@type"]!="authorizationStateClosed"{tokio::time::sleep(Duration::from_millis(100)).await;}}).await.map_err(|_|Error::bad("Telegram đang đóng phiên; thử lại sau hoặc khởi động lại server"))?;
        }
        app.tg.accounts.write().await.remove("main");
    }
    app.tg.start("main",None).await?;db::audit(&app.db,&u.id,"telegram_config","").await;Ok(Json(json!({"ok":true})))
}
pub async fn telegram_auth(State(app):State<App>,Extension(u):Extension<User>,Json(p):Json<Value>)->Result<Json<Value>> {
    u.admin()?;app.tg.start("main",None).await?;
    let request=match p["action"].as_str(){
        Some("qr")=>json!({"@type":"requestQrCodeAuthentication","other_user_ids":[]}),
        Some("phone")=>json!({"@type":"setAuthenticationPhoneNumber","phone_number":p["value"],"settings":{"@type":"phoneNumberAuthenticationSettings","allow_flash_call":false,"allow_missed_call":false,"is_current_phone_number":false,"allow_sms_retriever_api":false}}),
        Some("code")=>json!({"@type":"checkAuthenticationCode","code":p["value"]}),
        Some("password")=>json!({"@type":"checkAuthenticationPassword","password":p["value"]}),
        Some("email")=>json!({"@type":"setAuthenticationEmailAddress","email_address":p["value"]}),
        Some("email_code")=>json!({"@type":"checkAuthenticationEmailCode","code":{"@type":"emailAddressAuthenticationCode","code":p["value"]}}),
        Some("logout")=>{db::audit(&app.db,&u.id,"telegram_logout","").await;json!({"@type":"logOut"})},
        _=>return Err(Error::bad("Thao tác đăng nhập không hợp lệ")),
    };
    app.tg.call("main",request).await?;Ok(Json(json!({"ok":true})))
}
pub async fn chats(State(app):State<App>,Extension(u):Extension<User>)->Result<Json<Value>> {
    u.admin()?;let _=app.tg.call("main",json!({"@type":"loadChats","chat_list":{"@type":"chatListMain"},"limit":100})).await;
    let list=app.tg.call("main",json!({"@type":"getChats","chat_list":{"@type":"chatListMain"},"limit":100})).await?;
    let mut chats=vec![];if let Some(ids)=list["chat_ids"].as_array(){for id in ids{if let Ok(chat)=app.tg.call("main",json!({"@type":"getChat","chat_id":id})).await{chats.push(json!({"id":chat["id"],"title":chat["title"],"type":chat["type"]["@type"]}));}}}
    Ok(Json(json!(chats)))
}
pub async fn save(State(app):State<App>,Extension(u):Extension<User>,Json(p):Json<Value>)->Result<Json<Value>> {
    u.admin()?;
    if let Some(chat)=p["storage_chat"].as_str(){
        if chat!="me"&&chat.parse::<i64>().is_err()&&!chat.starts_with('@'){return Err(Error::bad("Nhập me, @username hoặc chat ID"));}
        // Each part stores its own chat ID, so changing the default doesn't break old files.
        if chat!="me" {let req=if let Ok(id)=chat.parse::<i64>(){json!({"@type":"getChat","chat_id":id})}else{json!({"@type":"searchPublicChat","username":chat.trim_start_matches('@')})};app.tg.call("main",req).await?;}
        db::set(&app.db,"storage_chat",chat).await?;
    }
    if let Some(x)=p["backup_enabled"].as_bool(){db::set(&app.db,"backup_enabled",if x{"true"}else{"false"}).await?;}
    db::audit(&app.db,&u.id,"settings_changed","").await;Ok(Json(json!({"ok":true})))
}
pub async fn add_bot(State(app):State<App>,Extension(u):Extension<User>,Json(p):Json<Value>)->Result<Json<Value>> {
    u.admin()?;let token=p["token"].as_str().filter(|s|s.contains(':')&&s.len()<256).ok_or_else(||Error::bad("Bot token không hợp lệ"))?;
    let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM bot_accounts").fetch_one(&app.db).await?;if count>=16{return Err(Error::bad("Tối đa 16 bot"));}
    let id=db::id();let name=p["name"].as_str().filter(|s|!s.is_empty()).unwrap_or("Bot");
    sqlx::query("INSERT INTO bot_accounts(id,name,token) VALUES(?,?,?)").bind(&id).bind(name).bind(crypto::seal(&app.cfg.master_key,token)?).execute(&app.db).await?;
    app.tg.start(&id,Some(token.into())).await?;Ok(Json(json!({"id":id})))
}
pub async fn remove_bot(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>)->Result<Json<Value>> {
    u.admin()?;if id=="main"{return Err(Error::forbidden());}
    let active=app.cancel.lock().await.len();if active>0{return Err(Error::bad("Đợi hoặc hủy các tác vụ trước khi gỡ bot"));}
    let _=app.tg.call(&id,json!({"@type":"close"})).await;app.tg.accounts.write().await.remove(&id);
    sqlx::query("DELETE FROM bot_accounts WHERE id=?").bind(&id).execute(&app.db).await?;
    // Old references fall back to the main account, which must remain in the storage group.
    Ok(Json(json!({"ok":true})))
}
pub async fn audit(State(app):State<App>,Extension(u):Extension<User>)->Result<Json<Value>> {
    u.admin()?;let rows:Vec<(i64,Option<String>,String,String,i64)>=sqlx::query_as("SELECT id,user_id,action,detail,created_at FROM audit ORDER BY id DESC LIMIT 300").fetch_all(&app.db).await?;
    Ok(Json(json!(rows.into_iter().map(|(id,user,action,detail,at)|json!({"id":id,"user_id":user,"action":action,"detail":detail,"created_at":at})).collect::<Vec<_>>())))
}
pub async fn websocket(State(app):State<App>,Extension(u):Extension<User>,ws:WebSocketUpgrade)->Response {
    ws.on_upgrade(move|socket|listen(socket,app,u))
}
async fn listen(socket:WebSocket,app:App,user:User) {
    let (mut send,mut recv)=socket.split();let mut events=app.events.subscribe();let mut ping=tokio::time::interval(Duration::from_secs(25));
    loop{tokio::select!{
        _=app.stopping.cancelled()=>break,
        _=ping.tick()=>{if send.send(Message::Ping(Vec::new().into())).await.is_err(){break;}},
        incoming=recv.next()=>{if !matches!(incoming,Some(Ok(Message::Pong(_)|Message::Text(_)|Message::Binary(_)|Message::Ping(_)))){break;}},
        event=events.recv()=>match event{
            Ok(mut value)=>{if value["owner"].as_str()!=Some(&user.id){continue;}value.as_object_mut().map(|m|m.remove("owner"));if send.send(Message::Text(value.to_string().into())).await.is_err(){break;}},
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>{let _=send.send(Message::Text(json!({"type":"refresh"}).to_string().into())).await;},Err(_)=>break,
        }
    }}
}
