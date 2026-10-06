use crate::{crypto,db::{self,User},error::{Error,Result},state::App};
use axum::{extract::{State,ConnectInfo,Path},http::{HeaderMap,StatusCode,header},middleware::Next,response::{IntoResponse,Response},body::Body,Json,Extension};
use serde::Deserialize;
use serde_json::{json,Value};
use std::net::SocketAddr;
use subtle::ConstantTimeEq;

pub fn cookie(headers:&HeaderMap,name:&str)->Option<String>{headers.get(header::COOKIE)?.to_str().ok()?.split(';').find_map(|v|{let(k,v)=v.trim().split_once('=')?;if k==name{Some(v.to_owned())}else{None}})}
pub async fn user(app:&App,headers:&HeaderMap)->Result<User> {
    let u=if let Some(v)=headers.get(header::AUTHORIZATION).and_then(|x|x.to_str().ok()).and_then(|s|s.strip_prefix("Bearer ")) {
        sqlx::query_as::<_,User>("SELECT * FROM users WHERE api_hash=? AND disabled=0").bind(crypto::sha(v)).fetch_optional(&app.db).await?
    }else if let Some(token)=cookie(headers,"tc_session") {
        sqlx::query_as::<_,User>("SELECT u.* FROM users u JOIN sessions s ON s.user_id=u.id WHERE s.token_hash=? AND s.expires_at>? AND u.disabled=0").bind(crypto::sha(token)).bind(db::now()).fetch_optional(&app.db).await?
    }else{None};
    u.ok_or_else(Error::unauthorized)
}
pub async fn require(State(app):State<App>,mut req:axum::extract::Request,next:Next)->Result<Response> {let u=user(&app,req.headers()).await?;req.extensions_mut().insert(u);Ok(next.run(req).await)}
pub async fn origin_guard(State(app):State<App>,req:axum::extract::Request,next:Next)->Result<Response> {
    if !matches!(*req.method(),axum::http::Method::GET|axum::http::Method::HEAD|axum::http::Method::OPTIONS) {
        if let Some(origin)=req.headers().get(header::ORIGIN).and_then(|s|s.to_str().ok()) {
            if origin!=app.cfg.origin {return Err(Error::forbidden());}
        }
        if cookie(req.headers(),"tc_session").is_some()&&req.uri().path().starts_with("/api/")&&!matches!(req.headers().get("x-requested-with").and_then(|s|s.to_str().ok()), Some("ITClub Cloud") | Some("ItClub Cloud") | Some("TeleCloud")) {
            return Err(Error::bad("Thiếu X-Requested-With: ITClub Cloud"));
        }
    }
    let mut res=next.run(req).await;
    res.headers_mut().insert("x-content-type-options","nosniff".parse().unwrap());
    res.headers_mut().insert("referrer-policy","same-origin".parse().unwrap());
    res.headers_mut().insert("x-frame-options","SAMEORIGIN".parse().unwrap());
    Ok(res)
}
pub async fn check_rate(app:&App,key:&str)->Result<()> {
    let map=app.throttle.lock().await;
    if map.get(key).is_some_and(|(count,expiry)|*count>=15&&*expiry>db::now()) {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS,"Quá nhiều lần thử. Vui lòng đợi 15 phút".into()));
    }
    Ok(())
}
pub async fn rate(app:&App,key:String)->Result<()> {
    let now=db::now();let mut map=app.throttle.lock().await;
    map.retain(|_,(_,expiry)|*expiry>now);
    if map.len()>4096&&!map.contains_key(&key){return Err(Error(StatusCode::TOO_MANY_REQUESTS,"Thử lại sau".into()));}
    let entry=map.entry(key).or_insert((0,now+900));entry.0+=1;
    if entry.0>15 {return Err(Error(StatusCode::TOO_MANY_REQUESTS,"Quá nhiều lần thử. Vui lòng đợi 15 phút".into()));}Ok(())
}
#[derive(Deserialize)]pub struct Credentials {pub username:String,pub password:String,pub setup_token:Option<String>}
pub async fn status(State(app):State<App>,headers:HeaderMap)->Result<Json<Value>> {
    let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(&app.db).await?;
    let u=user(&app,&headers).await.ok();
    Ok(Json(json!({"setup_required":count==0,"user":u,"version":env!("CARGO_PKG_VERSION")})))
}
pub fn valid_username(name:&str)->Result<()> {if name.len()<3||name.len()>40||!name.bytes().all(|c|c.is_ascii_alphanumeric()||c==b'_'||c==b'-'){return Err(Error::bad("Tên tài khoản: 3–40 ký tự a-z, 0-9, _ hoặc -"));}Ok(())}
pub async fn setup(State(app):State<App>,ConnectInfo(addr):ConnectInfo<SocketAddr>,Json(p):Json<Credentials>)->Result<Response> {
    rate(&app,format!("setup:{}",addr.ip())).await?;
    let lock=app.lock("setup");let _guard=lock.lock().await;
    let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(&app.db).await?;if count!=0{return Err(Error::forbidden());}
    if !bool::from(p.setup_token.unwrap_or_default().as_bytes().ct_eq(app.cfg.setup_token.as_bytes())){return Err(Error::bad("Mã thiết lập không đúng; xem log của server hoặc SETUP_TOKEN"));}
    valid_username(&p.username)?;let hash=crypto::password_hash(p.password).await?;let id=db::id();
    sqlx::query("INSERT INTO users(id,username,password_hash,role,created_at) VALUES(?,?,?,'admin',?)").bind(&id).bind(&p.username).bind(hash).bind(db::now()).execute(&app.db).await?;
    let u:User=sqlx::query_as("SELECT * FROM users WHERE id=?").bind(id).fetch_one(&app.db).await?;session(&app,&u).await
}
pub async fn login(State(app):State<App>,ConnectInfo(addr):ConnectInfo<SocketAddr>,Json(p):Json<Credentials>)->Result<Response> {
    rate(&app,format!("login:{}",addr.ip())).await?;
    let u:Option<User>=sqlx::query_as("SELECT * FROM users WHERE username=? AND disabled=0").bind(&p.username).fetch_optional(&app.db).await?;
    let Some(u)=u else{tokio::time::sleep(std::time::Duration::from_millis(250)).await;return Err(Error::unauthorized());};
    if !crypto::verify(p.password,u.password_hash.clone()).await {db::audit(&app.db,&u.id,"login_failed","").await;return Err(Error::unauthorized());}
    db::audit(&app.db,&u.id,"login","").await;session(&app,&u).await
}
pub async fn session(app:&App,u:&User)->Result<Response> {
    let token=crypto::random_token();sqlx::query("INSERT INTO sessions(token_hash,user_id,expires_at) VALUES(?,?,?)").bind(crypto::sha(&token)).bind(&u.id).bind(db::now()+30*86400).execute(&app.db).await?;
    let cookie=format!("tc_session={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000{}",if app.cfg.secure_cookie{"; Secure"}else{""});
    Ok(([(header::SET_COOKIE,cookie)],Json(json!({"user":u}))).into_response())
}
pub async fn logout(State(app):State<App>,headers:HeaderMap)->Result<Response> {
    if let Some(c)=cookie(&headers,"tc_session"){sqlx::query("DELETE FROM sessions WHERE token_hash=?").bind(crypto::sha(c)).execute(&app.db).await?;}
    Ok(([(header::SET_COOKIE,"tc_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")],Json(json!({"ok":true}))).into_response())
}
#[derive(Deserialize)]pub struct Password {old_password:Option<String>,password:String}
pub async fn change_password(State(app):State<App>,Extension(u):Extension<User>,Json(p):Json<Password>)->Result<Json<Value>> {
    if !crypto::verify(p.old_password.unwrap_or_default(),u.password_hash.clone()).await{return Err(Error::forbidden());}
    let hash=crypto::password_hash(p.password).await?;
    let mut tx=app.db.begin().await?;sqlx::query("UPDATE users SET password_hash=? WHERE id=?").bind(hash).bind(&u.id).execute(&mut *tx).await?;sqlx::query("DELETE FROM sessions WHERE user_id=?").bind(&u.id).execute(&mut *tx).await?;tx.commit().await?;
    db::audit(&app.db,&u.id,"password_changed","").await;Ok(Json(json!({"ok":true})))
}
pub async fn list_users(State(app):State<App>,Extension(u):Extension<User>)->Result<Json<Vec<User>>> {u.admin()?;Ok(Json(sqlx::query_as("SELECT * FROM users ORDER BY created_at").fetch_all(&app.db).await?))}
#[derive(Deserialize)]pub struct CreateUser {username:String,password:String,#[serde(default)]quota_bytes:i64}
pub async fn create_user(State(app):State<App>,Extension(u):Extension<User>,Json(p):Json<CreateUser>)->Result<Json<Value>> {
    u.admin()?;valid_username(&p.username)?;if p.quota_bytes<0{return Err(Error::bad("Quota không hợp lệ"));}let id=db::id();let hash=crypto::password_hash(p.password).await?;
    sqlx::query("INSERT INTO users(id,username,password_hash,role,quota_bytes,created_at) VALUES(?,?,?,'user',?,?)").bind(&id).bind(p.username).bind(hash).bind(p.quota_bytes).bind(db::now()).execute(&app.db).await?;
    db::audit(&app.db,&u.id,"create_user",&id).await;Ok(Json(json!({"id":id})))
}
#[derive(Deserialize)]pub struct EditUser {disabled:Option<bool>,quota_bytes:Option<i64>,password:Option<String>}
pub async fn edit_user(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>,Json(p):Json<EditUser>)->Result<Json<Value>> {
    u.admin()?;let target:User=sqlx::query_as("SELECT * FROM users WHERE id=?").bind(&id).fetch_optional(&app.db).await?.ok_or_else(Error::not_found)?;
    if target.role=="admin" {return Err(Error::bad("Dùng trang Hồ sơ để thay đổi tài khoản quản trị"));}
    if let Some(x)=p.quota_bytes{if x<0{return Err(Error::bad("Quota không hợp lệ"));}sqlx::query("UPDATE users SET quota_bytes=? WHERE id=?").bind(x).bind(&id).execute(&app.db).await?;}
    if let Some(x)=p.disabled{sqlx::query("UPDATE users SET disabled=? WHERE id=?").bind(x).bind(&id).execute(&app.db).await?;}
    if let Some(x)=p.password{let hash=crypto::password_hash(x).await?;sqlx::query("UPDATE users SET password_hash=? WHERE id=?").bind(hash).bind(&id).execute(&app.db).await?;}
    sqlx::query("DELETE FROM sessions WHERE user_id=?").bind(&id).execute(&app.db).await?;
    db::audit(&app.db,&u.id,"edit_user",&id).await;Ok(Json(json!({"ok":true})))
}
pub async fn keys(State(app):State<App>,Extension(u):Extension<User>,Json(p):Json<Value>)->Result<Json<Value>> {
    match p["kind"].as_str(){
        Some("api")=>{let key=crypto::random_token();sqlx::query("UPDATE users SET api_hash=? WHERE id=?").bind(crypto::sha(&key)).bind(&u.id).execute(&app.db).await?;Ok(Json(json!({"key":key})))},
        Some("s3")=>{let access=format!("IC{}",&crypto::sha(crypto::random_token())[..18]);let secret=crypto::random_token();sqlx::query("UPDATE users SET s3_access=?,s3_secret=? WHERE id=?").bind(&access).bind(crypto::seal(&app.cfg.master_key,&secret)?).bind(&u.id).execute(&app.db).await?;Ok(Json(json!({"access_key":access,"secret_key":secret,"endpoint":format!("{}/s3",app.cfg.origin),"bucket":"itclub-cloud","region":"us-east-1"})))},
        Some("revoke_api")=>{sqlx::query("UPDATE users SET api_hash=NULL WHERE id=?").bind(&u.id).execute(&app.db).await?;Ok(Json(json!({"ok":true})))},
        Some("revoke_s3")=>{sqlx::query("UPDATE users SET s3_access=NULL,s3_secret=NULL WHERE id=?").bind(&u.id).execute(&app.db).await?;Ok(Json(json!({"ok":true})))},
        _=>Err(Error::bad("Loại khóa không hợp lệ"))
    }
}
pub async fn basic(app:&App,headers:&HeaderMap)->Result<User> {
    use base64::Engine;
    let value=headers.get(header::AUTHORIZATION).and_then(|x|x.to_str().ok()).and_then(|s|s.strip_prefix("Basic ")).ok_or_else(Error::unauthorized)?;
    let decoded=base64::engine::general_purpose::STANDARD.decode(value).map_err(|_|Error::unauthorized())?;
    let text=String::from_utf8(decoded).map_err(|_|Error::unauthorized())?;let (name,password)=text.split_once(':').ok_or_else(Error::unauthorized)?;
    let u:User=sqlx::query_as("SELECT * FROM users WHERE username=? AND disabled=0").bind(name).fetch_optional(&app.db).await?.ok_or_else(Error::unauthorized)?;
    // An API key is accepted as an app password for WebDAV, avoiding repeated Argon2 work.
    let api:Option<String>=sqlx::query_scalar("SELECT api_hash FROM users WHERE id=?").bind(&u.id).fetch_one(&app.db).await?;
    if api.as_deref()==Some(crypto::sha(password).as_str())||crypto::verify(password.into(),u.password_hash.clone()).await{Ok(u)}else{Err(Error::unauthorized())}
}
