use crate::{auth,crypto,db::{self,User},error::{Error,Result},state::App};
use axum::{extract::{State,Path,ConnectInfo},Extension,Json,http::{HeaderMap,header},response::{Response,IntoResponse}};
use serde_json::{json,Value};
use std::{collections::HashMap,net::SocketAddr};
use tokio::sync::Mutex;
use webauthn_rs::prelude::*;

enum Ceremony {Registration{owner:String,state:PasskeyRegistration},Authentication{owner:String,state:PasskeyAuthentication}}
pub struct PasskeyState {web:Option<Webauthn>,pending:Mutex<HashMap<String,(i64,Ceremony)>>}
impl PasskeyState {
    pub fn new(origin:&str)->anyhow::Result<Self> {
        let url=url::Url::parse(origin)?;let rp=url.host_str().unwrap_or("localhost");
        // Passkeys require a DNS RP ID. HTTP IP deployments can still use password login.
        let web=WebauthnBuilder::new(rp,&url).ok().and_then(|b|b.rp_name("ITClub Cloud").build().ok());
        Ok(Self{web,pending:Mutex::new(HashMap::new())})
    }
    fn web(&self)->Result<&Webauthn>{self.web.as_ref().ok_or_else(||Error::bad("Passkey cần PUBLIC_URL dùng HTTPS và hostname, hoặc http://localhost"))}
    async fn insert(&self,ceremony:Ceremony)->Result<String>{let mut p=self.pending.lock().await;p.retain(|_,(at,_)|*at>db::now());if p.len()>1000{return Err(Error::bad("Quá nhiều yêu cầu passkey"));}let token=crypto::random_token();p.insert(token.clone(),(db::now()+180,ceremony));Ok(token)}
    async fn take(&self,headers:&HeaderMap)->Result<Ceremony>{let token=auth::cookie(headers,"tc_webauthn").ok_or_else(Error::unauthorized)?;let (expires,state)=self.pending.lock().await.remove(&token).ok_or_else(||Error::bad("Yêu cầu passkey không tồn tại hoặc đã dùng"))?;if expires<db::now(){return Err(Error::bad("Yêu cầu passkey đã hết hạn"));}Ok(state)}
}
fn challenge(app:&App,token:&str,options:impl serde::Serialize)->Result<Response>{
    let cookie=format!("tc_webauthn={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=180{}",if app.cfg.secure_cookie{"; Secure"}else{""});
    Ok(([(header::SET_COOKIE,cookie)],Json(options)).into_response())
}
async fn credentials(app:&App,owner:&str)->Result<Vec<(String,Passkey)>>{
    let rows:Vec<(String,String)>=sqlx::query_as("SELECT id,credential FROM passkeys WHERE user_id=?").bind(owner).fetch_all(&app.db).await?;
    rows.into_iter().map(|(id,s)|Ok((id,serde_json::from_str(&s)?))).collect()
}
pub async fn register_begin(State(app):State<App>,Extension(u):Extension<User>)->Result<Response>{
    let existing=credentials(&app,&u.id).await?;let exclude=existing.iter().map(|(_,p)|p.cred_id().clone()).collect();
    let (options,state)=app.passkeys.web()?.start_passkey_registration(uuid::Uuid::parse_str(&u.id).map_err(|e|Error::bad(e.to_string()))?,&u.username,&u.username,Some(exclude)).map_err(|e|Error::bad(e.to_string()))?;
    let token=app.passkeys.insert(Ceremony::Registration{owner:u.id,state}).await?;challenge(&app,&token,options)
}
pub async fn register_finish(State(app):State<App>,Extension(u):Extension<User>,headers:HeaderMap,Json(p):Json<Value>)->Result<Json<Value>>{
    let Ceremony::Registration{owner,state}=app.passkeys.take(&headers).await?else{return Err(Error::bad("Sai loại yêu cầu"));};if owner!=u.id{return Err(Error::forbidden());}
    let credential:RegisterPublicKeyCredential=serde_json::from_value(p["credential"].clone())?;
    let passkey=app.passkeys.web()?.finish_passkey_registration(&credential,&state).map_err(|e|Error::bad(e.to_string()))?;
    let id=hex::encode(passkey.cred_id());let name=p["name"].as_str().unwrap_or("Passkey").chars().take(80).collect::<String>();
    sqlx::query("INSERT INTO passkeys(id,user_id,name,credential,created_at) VALUES(?,?,?,?,?)").bind(id).bind(&u.id).bind(name).bind(serde_json::to_string(&passkey)?).bind(db::now()).execute(&app.db).await?;
    db::audit(&app.db,&u.id,"passkey_registered","").await;Ok(Json(json!({"ok":true})))
}
pub async fn login_begin(State(app):State<App>,ConnectInfo(addr):ConnectInfo<SocketAddr>,Json(p):Json<Value>)->Result<Response>{
    auth::rate(&app,format!("passkey:{}",addr.ip())).await?;
    let u:User=sqlx::query_as("SELECT * FROM users WHERE username=? AND disabled=0").bind(p["username"].as_str().unwrap_or("")).fetch_optional(&app.db).await?.ok_or_else(Error::unauthorized)?;
    let keys=credentials(&app,&u.id).await?;let keys:Vec<_>=keys.into_iter().map(|(_,p)|p).collect();if keys.is_empty(){return Err(Error::bad("Tài khoản chưa đăng ký passkey"));}
    let (options,state)=app.passkeys.web()?.start_passkey_authentication(&keys).map_err(|e|Error::bad(e.to_string()))?;
    let token=app.passkeys.insert(Ceremony::Authentication{owner:u.id,state}).await?;challenge(&app,&token,options)
}
pub async fn login_finish(State(app):State<App>,headers:HeaderMap,Json(credential):Json<PublicKeyCredential>)->Result<Response>{
    let Ceremony::Authentication{owner,state}=app.passkeys.take(&headers).await?else{return Err(Error::bad("Sai loại yêu cầu"));};
    let result=app.passkeys.web()?.finish_passkey_authentication(&credential,&state).map_err(|_|Error::unauthorized())?;
    for (id,mut key) in credentials(&app,&owner).await? {if key.update_credential(&result).is_some(){sqlx::query("UPDATE passkeys SET credential=? WHERE id=?").bind(serde_json::to_string(&key)?).bind(id).execute(&app.db).await?;}}
    let u:User=sqlx::query_as("SELECT * FROM users WHERE id=? AND disabled=0").bind(&owner).fetch_optional(&app.db).await?.ok_or_else(Error::unauthorized)?;
    db::audit(&app.db,&owner,"passkey_login","").await;auth::session(&app,&u).await
}
pub async fn list(State(app):State<App>,Extension(u):Extension<User>)->Result<Json<Value>>{let rows:Vec<(String,String,i64)>=sqlx::query_as("SELECT id,name,created_at FROM passkeys WHERE user_id=?").bind(u.id).fetch_all(&app.db).await?;Ok(Json(json!(rows.into_iter().map(|(id,name,at)|json!({"id":id,"name":name,"created_at":at})).collect::<Vec<_>>())))}
pub async fn remove(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>)->Result<Json<Value>>{sqlx::query("DELETE FROM passkeys WHERE id=? AND user_id=?").bind(id).bind(&u.id).execute(&app.db).await?;db::audit(&app.db,&u.id,"passkey_deleted","").await;Ok(Json(json!({"ok":true})))}
