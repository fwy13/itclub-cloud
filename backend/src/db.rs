use crate::error::{Error,Result};
use serde::{Serialize,Deserialize};
use sqlx::{SqlitePool,sqlite::{SqliteConnectOptions,SqlitePoolOptions,SqliteJournalMode}};
use std::{path::Path,time::Duration};

pub fn now()->i64 {chrono::Utc::now().timestamp()}
pub fn id()->String {uuid::Uuid::new_v4().to_string()}
#[derive(Clone,Serialize,sqlx::FromRow)]
pub struct User {pub id:String,pub username:String,pub role:String,pub quota_bytes:i64,pub disabled:i64,#[serde(skip_serializing)]pub password_hash:String,pub telegram_user_id:Option<i64>,pub created_at:i64}
impl User {pub fn admin(&self)->Result<()> {if self.role=="admin" {Ok(())} else {Err(Error::forbidden())}}}
#[derive(Clone,Serialize,Deserialize,sqlx::FromRow)]
pub struct Node {pub id:String,pub owner:String,pub parent_id:Option<String>,pub name:String,pub kind:String,pub size:i64,pub mime:String,pub etag:String,pub deleted_at:Option<i64>,pub created_at:i64,pub updated_at:i64}
#[derive(Clone,Serialize,sqlx::FromRow)]
pub struct Part {pub part_index:i64,pub chat_id:i64,pub message_id:i64,pub account:String,pub size:i64}
#[derive(Clone,Serialize,sqlx::FromRow)]
pub struct Job {pub id:String,pub owner:String,pub name:String,pub parent_id:Option<String>,pub size:i64,pub mime:String,pub status:String,pub progress:i64,pub bytes_done:i64,pub error:Option<String>,pub node_id:Option<String>,pub source:Option<String>,pub module_id:Option<String>,pub parent_job_id:Option<String>,pub batch_index:Option<i64>,pub batch_total:i64,pub batch_done:i64,pub part_size:i64,pub chunk_size:i64,pub replace_id:Option<String>,pub created_at:i64,pub updated_at:i64}
pub async fn connect(path:&Path)->anyhow::Result<SqlitePool> {
    let opt=SqliteConnectOptions::new().filename(path).create_if_missing(true).foreign_keys(true).journal_mode(SqliteJournalMode::Wal).busy_timeout(Duration::from_secs(10));
    let pool=SqlitePoolOptions::new().max_connections(8).connect_with(opt).await?;
    sqlx::migrate!().run(&pool).await?;Ok(pool)
}
pub async fn setting(db:&SqlitePool,k:&str)->Option<String> {sqlx::query_scalar("SELECT value FROM settings WHERE key=?").bind(k).fetch_optional(db).await.ok().flatten()}
pub async fn set(db:&SqlitePool,k:&str,v:&str)->Result<()> {sqlx::query("INSERT INTO settings(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value").bind(k).bind(v).execute(db).await?;Ok(())}
pub async fn audit(db:&SqlitePool,user:&str,action:&str,detail:&str) {let _=sqlx::query("INSERT INTO audit(user_id,action,detail,created_at) VALUES(?,?,?,?)").bind(user).bind(action).bind(detail).bind(now()).execute(db).await;}
pub async fn node(db:&SqlitePool,owner:&str,id:&str)->Result<Node> {sqlx::query_as("SELECT * FROM nodes WHERE id=? AND owner=?").bind(id).bind(owner).fetch_optional(db).await?.ok_or_else(Error::not_found)}
pub async fn live_node(db:&SqlitePool,owner:&str,id:&str)->Result<Node> {let n=node(db,owner,id).await?;if n.deleted_at.is_some(){return Err(Error::not_found());}Ok(n)}
pub fn valid_name(name:&str)->Result<()> {
    if name.trim().is_empty()||name.len()>240||name=="."||name==".."||name.chars().any(|c|c.is_control()||c=='/'||c=='\\') {return Err(Error::bad("Tên không hợp lệ (tối đa 240 byte, không chứa dấu / hoặc ký tự điều khiển)"));}Ok(())
}
pub async fn parent(db:&SqlitePool,owner:&str,p:&Option<String>)->Result<()> {
    if let Some(id)=p {let n=live_node(db,owner,id).await?;if n.kind!="folder" {return Err(Error::bad("Thư mục đích không hợp lệ"));}}
    Ok(())
}
pub async fn descendants(db:&SqlitePool,owner:&str,id:&str)->Result<Vec<Node>> {
    Ok(sqlx::query_as("WITH RECURSIVE tree AS (SELECT * FROM nodes WHERE id=? AND owner=? UNION ALL SELECT n.* FROM nodes n JOIN tree t ON n.parent_id=t.id WHERE n.owner=?) SELECT * FROM tree").bind(id).bind(owner).bind(owner).fetch_all(db).await?)
}
pub async fn within(db:&SqlitePool,root:&Node,target:&str)->Result<bool> {
    let mut n=live_node(db,&root.owner,target).await?;
    for _ in 0..128 {if n.id==root.id {return Ok(true);}let Some(p)=n.parent_id else{return Ok(false);};n=live_node(db,&root.owner,&p).await?;}Ok(false)
}
pub async fn resolve_path(db:&SqlitePool,owner:&str,path:&str)->Result<Option<Node>> {
    let mut current=None;
    for part in path.split('/').filter(|s|!s.is_empty()) {
        valid_name(part)?;
        let n:Node=sqlx::query_as("SELECT * FROM nodes WHERE owner=? AND parent_id IS ? AND name=? AND deleted_at IS NULL").bind(owner).bind(&current).bind(part).fetch_optional(db).await?.ok_or_else(Error::not_found)?;
        current=Some(n.id.clone());
    }
    match current {Some(i)=>Ok(Some(live_node(db,owner,&i).await?)),None=>Ok(None)}
}
pub async fn ensure_path(db:&SqlitePool,owner:&str,path:&str)->Result<Option<String>> {
    let mut current=None;
    for segment in path.split('/').filter(|s|!s.is_empty()) {
        valid_name(segment)?;
        let existing:Option<Node>=sqlx::query_as("SELECT * FROM nodes WHERE owner=? AND parent_id IS ? AND name=? AND deleted_at IS NULL").bind(owner).bind(&current).bind(segment).fetch_optional(db).await?;
        if let Some(n)=existing {if n.kind!="folder" {return Err(Error::bad("Đường dẫn chứa file"));}current=Some(n.id);}
        else {let i=id();sqlx::query("INSERT OR IGNORE INTO nodes(id,owner,parent_id,name,kind,created_at,updated_at) VALUES(?,?,?,?,'folder',?,?)").bind(&i).bind(owner).bind(&current).bind(segment).bind(now()).bind(now()).execute(db).await?;
            current=Some(sqlx::query_scalar::<_,String>("SELECT id FROM nodes WHERE owner=? AND parent_id IS ? AND name=? AND deleted_at IS NULL").bind(owner).bind(&current).bind(segment).fetch_one(db).await?);}
    }Ok(current)
}
pub async fn full_path(db:&SqlitePool,n:&Node)->Result<String> {
    let mut pieces=vec![n.name.clone()];let mut parent=n.parent_id.clone();
    for _ in 0..128 {let Some(i)=parent else{break;};let p=node(db,&n.owner,&i).await?;pieces.push(p.name);parent=p.parent_id;}
    pieces.reverse();Ok(pieces.join("/"))
}
