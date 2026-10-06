use crate::{auth,crypto,db::{self,Node,User},error::{Error,Result},state::App,storage};
use axum::{extract::{State,Path,Query,ConnectInfo},Extension,Json,http::{HeaderMap,Method,header,StatusCode},response::{Response,IntoResponse}};
use serde::Deserialize;
use serde_json::{json,Value};
use std::net::SocketAddr;
#[derive(sqlx::FromRow)]struct Share {token:String,node_id:String,owner:String,password_hash:Option<String>,expires_at:Option<i64>,downloads:i64,created_at:i64}
#[derive(Deserialize)]pub struct NewShare {password:Option<String>,expires_in_days:Option<i64>}
pub async fn create(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>,Json(p):Json<NewShare>)->Result<Json<Value>> {
    db::live_node(&app.db,&u.id,&id).await?;let token=crypto::random_token();
    let hash=if let Some(p)=p.password.filter(|s|!s.is_empty()){Some(crypto::password_hash(p).await?)}else{None};
    let expires=p.expires_in_days.filter(|n|*n>0).map(|n|db::now()+n.min(3650)*86400);
    sqlx::query("INSERT INTO shares(token,node_id,owner,password_hash,expires_at,created_at) VALUES(?,?,?,?,?,?)").bind(&token).bind(&id).bind(&u.id).bind(hash).bind(expires).bind(db::now()).execute(&app.db).await?;
    Ok(Json(json!({"token":token,"url":format!("{}/s/{token}",app.cfg.origin),"direct_url":format!("{}/api/public/shares/{token}/stream?download=true",app.cfg.origin)})))
}
pub async fn list(State(app):State<App>,Extension(u):Extension<User>)->Result<Json<Value>> {
    let rows:Vec<(String,String,String,Option<i64>,i64,i64)>=sqlx::query_as("SELECT s.token,n.name,s.node_id,s.expires_at,s.downloads,s.password_hash IS NOT NULL FROM shares s JOIN nodes n ON s.node_id=n.id WHERE s.owner=? ORDER BY s.created_at DESC").bind(&u.id).fetch_all(&app.db).await?;
    Ok(Json(json!(rows.into_iter().map(|(token,name,node,expiry,count,protected)|json!({"token":token,"name":name,"node_id":node,"expires_at":expiry,"downloads":count,"protected":protected==1,"url":format!("{}/s/{token}",app.cfg.origin)})).collect::<Vec<_>>())))
}
pub async fn revoke(State(app):State<App>,Extension(u):Extension<User>,Path(token):Path<String>)->Result<Json<Value>> {sqlx::query("DELETE FROM shares WHERE token=? AND owner=?").bind(token).bind(&u.id).execute(&app.db).await?;Ok(Json(json!({"ok":true})))}
async fn lookup(app:&App,token:&str)->Result<Share>{let s:Share=sqlx::query_as("SELECT * FROM shares WHERE token=?").bind(token).fetch_optional(&app.db).await?.ok_or_else(Error::not_found)?;if s.expires_at.is_some_and(|t|t<db::now()){return Err(Error(StatusCode::GONE,"Liên kết đã hết hạn".into()));}Ok(s)}
async fn allowed(app:&App,s:&Share,headers:&HeaderMap)->bool {
    if s.password_hash.is_none(){return true;}
    if let Ok(u)=auth::user(app,headers).await{if u.id==s.owner{return true;}}
    let Some(token)=auth::cookie(headers,&format!("tc_share_{}",&s.token[..12]))else{return false;};
    sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM share_sessions WHERE token_hash=? AND share_token=? AND expires_at>?)").bind(crypto::sha(token)).bind(&s.token).bind(db::now()).fetch_one(&app.db).await.unwrap_or(false)
}
#[derive(Deserialize)]pub struct Browse {node:Option<String>,#[serde(default)]download:bool}
pub async fn public_node(app:&App,token:&str,id:Option<&str>,headers:&HeaderMap)->Result<Node>{
    let s=lookup(app,token).await?;if !allowed(app,&s,headers).await{return Err(Error(StatusCode::UNAUTHORIZED,"share_password_required".into()));}
    let root=db::live_node(&app.db,&s.owner,&s.node_id).await?;
    if let Some(id)=id{if !db::within(&app.db,&root,id).await?{return Err(Error::forbidden());}db::live_node(&app.db,&s.owner,id).await}else{Ok(root)}
}
pub async fn public_info(State(app):State<App>,Path(token):Path<String>,Query(q):Query<Browse>,headers:HeaderMap)->Result<Json<Value>>{
    let n=public_node(&app,&token,q.node.as_deref(),&headers).await?;
    let children:Vec<Node>=if n.kind=="folder"{sqlx::query_as("SELECT * FROM nodes WHERE owner=? AND parent_id=? AND deleted_at IS NULL ORDER BY kind DESC,name").bind(&n.owner).bind(&n.id).fetch_all(&app.db).await?}else{vec![]};
    let public=|n:Node|json!({"id":n.id,"name":n.name,"kind":n.kind,"size":n.size,"mime":n.mime,"updated_at":n.updated_at});
    Ok(Json(json!({"node":public(n),"children":children.into_iter().map(public).collect::<Vec<_>>()})))
}
pub async fn unlock(State(app):State<App>,ConnectInfo(addr):ConnectInfo<SocketAddr>,Path(token):Path<String>,Json(p):Json<Value>)->Result<Response>{
    auth::rate(&app,format!("share:{}",addr.ip())).await?;let share=lookup(&app,&token).await?;
    if let Some(hash)=share.password_hash{if !crypto::verify(p["password"].as_str().unwrap_or("").into(),hash).await{return Err(Error::unauthorized());}}
    let session=crypto::random_token();sqlx::query("INSERT INTO share_sessions(token_hash,share_token,expires_at) VALUES(?,?,?)").bind(crypto::sha(&session)).bind(&token).bind(db::now()+86400).execute(&app.db).await?;
    Ok(([(header::SET_COOKIE,format!("tc_share_{}={session}; HttpOnly; SameSite=Lax; Path=/; Max-Age=86400{}",&token[..12],if app.cfg.secure_cookie{"; Secure"}else{""}))],Json(json!({"ok":true}))).into_response())
}
pub async fn public_stream(State(app):State<App>,Path(token):Path<String>,Query(q):Query<Browse>,headers:HeaderMap,method:Method)->Result<Response>{
    let node=public_node(&app,&token,q.node.as_deref(),&headers).await?;
    if method!=Method::HEAD{sqlx::query("UPDATE shares SET downloads=downloads+1 WHERE token=?").bind(&token).execute(&app.db).await?;}
    storage::serve(app,node,headers,method==Method::HEAD,q.download).await
}
