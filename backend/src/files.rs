use crate::{db::{self,User,Node},error::{Error,Result},state::App,storage};
use axum::{extract::{State,Path,Query,Request},Extension,Json,http::{HeaderMap,Method,StatusCode},response::{Response,IntoResponse},body::Body};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value,json};
use std::io::SeekFrom;
use tokio::io::{AsyncSeekExt,AsyncWriteExt};

#[derive(Deserialize)]pub struct Listing {pub parent:Option<String>,pub q:Option<String>,pub trash:Option<bool>}
pub async fn list(State(app):State<App>,Extension(u):Extension<User>,Query(q):Query<Listing>)->Result<Json<Value>> {
    let nodes:Vec<Node>=if q.trash.unwrap_or(false){sqlx::query_as("SELECT * FROM nodes WHERE owner=? AND deleted_at IS NOT NULL ORDER BY deleted_at DESC LIMIT 2000").bind(&u.id).fetch_all(&app.db).await?}
    else if let Some(search)=q.q.filter(|s|!s.is_empty()) {sqlx::query_as("SELECT * FROM nodes WHERE owner=? AND deleted_at IS NULL AND name LIKE ? ESCAPE '\\' ORDER BY kind DESC,name LIMIT 2000").bind(&u.id).bind(format!("%{}%",search.replace('\\',"\\\\").replace('%',"\\%").replace('_',"\\_"))).fetch_all(&app.db).await?}
    else{db::parent(&app.db,&u.id,&q.parent).await?;sqlx::query_as("SELECT * FROM nodes WHERE owner=? AND parent_id IS ? AND deleted_at IS NULL ORDER BY CASE kind WHEN 'folder' THEN 0 ELSE 1 END,name LIMIT 2000").bind(&u.id).bind(&q.parent).fetch_all(&app.db).await?};
    let mut breadcrumbs=vec![];let mut current=q.parent;
    for _ in 0..128 {let Some(id)=current else{break;};let n=db::live_node(&app.db,&u.id,&id).await?;breadcrumbs.push(json!({"id":n.id,"name":n.name}));current=n.parent_id;}
    breadcrumbs.reverse();
    let used:i64=sqlx::query_scalar("SELECT COALESCE(SUM(size),0) FROM nodes WHERE owner=? AND kind='file'").bind(&u.id).fetch_one(&app.db).await?;
    Ok(Json(json!({"nodes":nodes,"breadcrumbs":breadcrumbs,"used_bytes":used,"quota_bytes":u.quota_bytes,"limit":2000})))
}
#[derive(Deserialize)]pub struct Folder {pub name:String,pub parent_id:Option<String>}
pub async fn mkdir(State(app):State<App>,Extension(u):Extension<User>,Json(p):Json<Folder>)->Result<Json<Value>> {
    db::valid_name(&p.name)?;db::parent(&app.db,&u.id,&p.parent_id).await?;let id=db::id();
    sqlx::query("INSERT INTO nodes(id,owner,parent_id,name,kind,created_at,updated_at) VALUES(?,?,?,?,'folder',?,?)").bind(&id).bind(&u.id).bind(p.parent_id).bind(p.name).bind(db::now()).bind(db::now()).execute(&app.db).await?;
    app.changed(&u.id);Ok(Json(json!({"id":id})))
}
#[derive(Deserialize)]pub struct Edit {pub name:Option<String>,pub parent_id:Option<String>,#[serde(default)]pub move_to_root:bool}
pub async fn edit(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>,Json(p):Json<Edit>)->Result<Json<Value>> {
    let lock=app.lock(format!("files:{}",u.id));let _guard=lock.lock().await;
    let n=db::live_node(&app.db,&u.id,&id).await?;
    let name=p.name.unwrap_or(n.name);db::valid_name(&name)?;
    let parent=if p.move_to_root{None}else{p.parent_id.or(n.parent_id.clone())};
    db::parent(&app.db,&u.id,&parent).await?;
    if let Some(p)=&parent{let root=db::live_node(&app.db,&u.id,&id).await?;if db::within(&app.db,&root,p).await?{return Err(Error::bad("Không thể di chuyển vào chính thư mục hoặc thư mục con"));}}
    sqlx::query("UPDATE nodes SET name=?,parent_id=?,updated_at=? WHERE id=?").bind(name).bind(parent).bind(db::now()).bind(&id).execute(&app.db).await?;
    app.changed(&u.id);Ok(Json(json!({"ok":true})))
}
pub async fn trash_node(app:&App,u:&User,id:&str)->Result<()> {
    db::node(&app.db,&u.id,id).await?;
    sqlx::query("WITH RECURSIVE tree(id) AS (SELECT id FROM nodes WHERE id=? AND owner=? UNION ALL SELECT n.id FROM nodes n JOIN tree t ON n.parent_id=t.id) UPDATE nodes SET deleted_at=COALESCE(deleted_at,?),updated_at=? WHERE id IN (SELECT id FROM tree)").bind(id).bind(&u.id).bind(db::now()).bind(db::now()).execute(&app.db).await?;
    app.changed(&u.id);Ok(())
}
pub async fn trash(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>)->Result<Json<Value>> {trash_node(&app,&u,&id).await?;Ok(Json(json!({"ok":true})))}
pub async fn restore(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>)->Result<Json<Value>> {
    let n=db::node(&app.db,&u.id,&id).await?;
    let lock=app.lock(format!("files:{}",u.id));let _guard=lock.lock().await;
    let mut tx=app.db.begin().await?;
    if let Some(parent)=n.parent_id {if db::live_node(&app.db,&u.id,&parent).await.is_err(){sqlx::query("UPDATE nodes SET parent_id=NULL WHERE id=?").bind(&id).execute(&mut *tx).await?;}}
    sqlx::query("WITH RECURSIVE tree(id) AS (SELECT id FROM nodes WHERE id=? UNION ALL SELECT n.id FROM nodes n JOIN tree t ON n.parent_id=t.id) UPDATE nodes SET deleted_at=NULL,updated_at=? WHERE id IN (SELECT id FROM tree)").bind(&id).bind(db::now()).execute(&mut *tx).await?;
    tx.commit().await?;app.changed(&u.id);Ok(Json(json!({"ok":true})))
}
pub async fn purge_node(app:&App,u:&User,id:&str)->Result<()> {
    let lock=app.lock(format!("files:{}",u.id));let _guard=lock.lock().await;
    let n=db::node(&app.db,&u.id,id).await?;let nodes=db::descendants(&app.db,&u.id,id).await?;
    let mut tx=app.db.begin().await?;
    for n in &nodes {sqlx::query("INSERT OR IGNORE INTO gc_messages(chat_id,message_id,account) SELECT chat_id,message_id,account FROM parts WHERE node_id=?").bind(&n.id).execute(&mut *tx).await?;}
    sqlx::query("WITH RECURSIVE tree(id) AS (SELECT id FROM nodes WHERE id=? UNION ALL SELECT n.id FROM nodes n JOIN tree t ON n.parent_id=t.id) DELETE FROM nodes WHERE id IN (SELECT id FROM tree)").bind(&n.id).execute(&mut *tx).await?;
    tx.commit().await?;
    for n in nodes{let _=tokio::fs::remove_file(app.cfg.data.join("thumbs").join(format!("{}.jpg",n.id))).await;}
    app.changed(&u.id);db::audit(&app.db,&u.id,"permanent_delete",id).await;Ok(())
}
pub async fn purge(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>)->Result<Json<Value>> {purge_node(&app,&u,&id).await?;Ok(Json(json!({"ok":true})))}
pub async fn empty_trash(State(app):State<App>,Extension(u):Extension<User>)->Result<Json<Value>> {
    let ids:Vec<String>=sqlx::query_scalar("SELECT id FROM nodes WHERE owner=? AND deleted_at IS NOT NULL AND (parent_id IS NULL OR parent_id NOT IN (SELECT id FROM nodes WHERE deleted_at IS NOT NULL))").bind(&u.id).fetch_all(&app.db).await?;
    for id in ids{purge_node(&app,&u,&id).await?;}Ok(Json(json!({"ok":true})))
}
#[derive(Deserialize)]pub struct Copy {pub parent_id:Option<String>,pub name:Option<String>}
pub async fn copy_node(app:&App,u:&User,id:&str,parent:Option<String>,name:Option<String>)->Result<String> {
    let lock=app.lock(format!("files:{}",u.id));let _guard=lock.lock().await;
    db::parent(&app.db,&u.id,&parent).await?;let source=db::live_node(&app.db,&u.id,id).await?;
    if let Some(p)=&parent{if db::within(&app.db,&source,p).await?{return Err(Error::bad("Không thể sao chép thư mục vào chính nó"));}}
    let name=name.unwrap_or_else(||format!("Copy {}",source.name));db::valid_name(&name)?;
    let nodes=db::descendants(&app.db,&u.id,id).await?;
    let total:i64=nodes.iter().filter(|n|n.deleted_at.is_none()).map(|n|n.size).sum();
    if u.quota_bytes>0{let used:i64=sqlx::query_scalar("SELECT COALESCE(SUM(size),0) FROM nodes WHERE owner=?").bind(&u.id).fetch_one(&app.db).await?;if used+total>u.quota_bytes{return Err(Error::bad("Không đủ quota"));}}
    let mut mapping=std::collections::HashMap::new();let root=db::id();mapping.insert(id.to_owned(),root.clone());
    let mut remaining:Vec<Node>=nodes.into_iter().filter(|n|n.deleted_at.is_none()).collect();
    let mut tx=app.db.begin().await?;
    while !remaining.is_empty(){let Some(i)=remaining.iter().position(|n|n.id==id||n.parent_id.as_ref().map(|p|mapping.contains_key(p)).unwrap_or(false))else{return Err(Error::bad("Cấu trúc thư mục không hợp lệ"));};
        let n=remaining.remove(i);let new_id=if n.id==id{root.clone()}else{db::id()};
        let new_parent=if n.id==id{parent.clone()}else{n.parent_id.as_ref().and_then(|p|mapping.get(p).cloned())};
        let new_name=if n.id==id{name.clone()}else{n.name.clone()};
        sqlx::query("INSERT INTO nodes(id,owner,parent_id,name,kind,size,mime,etag,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?)").bind(&new_id).bind(&u.id).bind(new_parent).bind(new_name).bind(&n.kind).bind(n.size).bind(&n.mime).bind(&n.etag).bind(db::now()).bind(db::now()).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO parts(node_id,part_index,chat_id,message_id,account,size) SELECT ?,part_index,chat_id,message_id,account,size FROM parts WHERE node_id=?").bind(&new_id).bind(&n.id).execute(&mut *tx).await?;mapping.insert(n.id,new_id);
    }tx.commit().await?;app.changed(&u.id);Ok(root)
}
pub async fn copy(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>,Json(p):Json<Copy>)->Result<Json<Value>> {Ok(Json(json!({"id":copy_node(&app,&u,&id,p.parent_id,p.name).await?})))}
#[derive(Deserialize)]pub struct StreamQuery {#[serde(default)]pub download:bool}
pub async fn stream(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>,Query(q):Query<StreamQuery>,headers:HeaderMap,method:Method)->Result<Response> {
    let n=db::live_node(&app.db,&u.id,&id).await?;storage::serve(app,n,headers,method==Method::HEAD,q.download).await
}
pub async fn thumb(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>)->Result<Response> {
    db::live_node(&app.db,&u.id,&id).await?;let path=app.cfg.data.join("thumbs").join(format!("{id}.jpg"));
    let f=tokio::fs::File::open(path).await.map_err(|_|Error::not_found())?;
    Ok(([("content-type","image/jpeg"),("cache-control","private, max-age=300")],Body::from_stream(tokio_util::io::ReaderStream::new(f))).into_response())
}
#[derive(Deserialize)]pub struct BeginUpload {pub name:String,pub size:i64,pub parent_id:Option<String>,pub mime:Option<String>,pub replace_id:Option<String>}
pub async fn begin_upload(State(app):State<App>,Extension(u):Extension<User>,Json(p):Json<BeginUpload>)->Result<Json<crate::db::Job>> {
    let mime=p.mime.filter(|m|!m.is_empty()).unwrap_or_else(||mime_guess::from_path(&p.name).first_or_octet_stream().to_string());
    let job=storage::create_job(&app,&u,&p.name,p.parent_id,p.size,&mime,"staging",p.replace_id).await?;
    let f=tokio::fs::File::create(app.cfg.upload_path(&job.id)).await?;f.set_len(p.size as u64).await?;Ok(Json(job))
}
pub async fn upload_status(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>)->Result<Json<Value>> {
    let job=storage::get_job(&app,&u.id,&id).await?;let chunks:Vec<i64>=sqlx::query_scalar("SELECT chunk_index FROM chunks WHERE job_id=? ORDER BY chunk_index").bind(&id).fetch_all(&app.db).await?;
    Ok(Json(json!({"job":job,"chunks":chunks})))
}
pub async fn chunk(State(app):State<App>,Extension(u):Extension<User>,Path((id,index)):Path<(String,u64)>,req:Request)->Result<Json<Value>> {
    let lock=app.lock(format!("upload:{id}"));let _guard=lock.lock().await;
    let job=storage::get_job(&app,&u.id,&id).await?;if job.status!="staging"{return Err(Error::bad("Upload không còn ở trạng thái nhận chunk"));}
    let total=(job.size as u64).div_ceil(job.chunk_size as u64);if index>=total{return Err(Error::bad("Chunk index không hợp lệ"));}
    let start=index*job.chunk_size as u64;let expected=(job.size as u64-start).min(job.chunk_size as u64);
    // Remove the prior receipt before overwriting: an interrupted retry must not leave a
    // partially overwritten chunk marked complete.
    sqlx::query("DELETE FROM chunks WHERE job_id=? AND chunk_index=?").bind(&id).bind(index as i64).execute(&app.db).await?;
    let mut f=tokio::fs::OpenOptions::new().write(true).open(app.cfg.upload_path(&id)).await?;f.seek(SeekFrom::Start(start)).await?;
    let mut body=req.into_body().into_data_stream();let mut written=0u64;
    while let Some(bytes)=body.next().await {let b=bytes.map_err(|e|Error::bad(e.to_string()))?;written+=b.len() as u64;if written>expected{return Err(Error::bad("Chunk quá lớn"));}f.write_all(&b).await?;}
    if written!=expected{return Err(Error::bad("Chunk chưa đủ dữ liệu"));}f.sync_data().await?;
    sqlx::query("INSERT INTO chunks(job_id,chunk_index,size) VALUES(?,?,?)").bind(&id).bind(index as i64).bind(written as i64).execute(&app.db).await?;
    let done:i64=sqlx::query_scalar("SELECT COALESCE(SUM(size),0) FROM chunks WHERE job_id=?").bind(&id).fetch_one(&app.db).await?;
    app.progress(&id,"staging",done as u64,job.size as u64,None).await;Ok(Json(json!({"ok":true,"bytes_done":done})))
}
pub async fn complete(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>)->Result<Json<Value>> {
    let lock=app.lock(format!("upload:{id}"));let _guard=lock.lock().await;let job=storage::get_job(&app,&u.id,&id).await?;
    if job.status=="done"{return Ok(Json(json!({"ok":true,"node_id":job.node_id})));}
    if job.status!="staging"{return Err(Error::bad("Upload đã được gửi xử lý"));}
    let received:i64=sqlx::query_scalar("SELECT COALESCE(SUM(size),0) FROM chunks WHERE job_id=?").bind(&id).fetch_one(&app.db).await?;
    if received!=job.size{return Err(Error::bad("Chưa nhận đủ chunk"));}storage::enqueue(app,id.clone()).await?;Ok(Json(json!({"id":id,"status":"queued"})))
}
pub async fn jobs(State(app):State<App>,Extension(u):Extension<User>)->Result<Json<Vec<crate::db::Job>>>{Ok(Json(sqlx::query_as("SELECT * FROM jobs WHERE owner=? AND (parent_job_id IS NULL OR parent_job_id IN (SELECT id FROM jobs WHERE batch_total=0)) ORDER BY created_at DESC LIMIT 200").bind(&u.id).fetch_all(&app.db).await?))}
pub async fn cancel(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>)->Result<Json<Value>> {
    let lock=app.lock(format!("upload:{id}"));let _guard=lock.lock().await;
    let job=storage::get_job(&app,&u.id,&id).await?;if job.status=="done"{return Err(Error::bad("Tác vụ đã hoàn tất"));}
    let token=app.cancel.lock().await.get(&id).cloned();
    if let Some(t)=token{t.cancel();}else{crate::downloads::cancel_children(&app,&job).await?;storage::discard_staging(&app,&id).await?;app.progress(&id,"cancelled",0,0,None).await;}
    Ok(Json(json!({"ok":true})))
}
pub async fn retry(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>)->Result<Json<Value>> {
    let lock=app.lock(format!("upload:{id}"));let _guard=lock.lock().await;
    let job=storage::get_job(&app,&u.id,&id).await?;
    if !matches!(job.status.as_str(),"error"|"interrupted"){return Err(Error::bad("Tác vụ chưa cần thử lại"));}
    if let Some(parent)=&job.parent_job_id{if app.cancel.lock().await.contains_key(parent){return Err(Error::bad("Đợi tác vụ cha kết thúc trước khi thử lại tập riêng"));}}
    if crate::remote::has_source(&app,&id).await? {
        crate::remote::retry(&app,&u,&id).await?;
        return Ok(Json(json!({"id":id,"status":"queued"})));
    }
    if !app.cfg.upload_path(&id).exists(){return Err(Error::bad("File nguồn không còn; hãy tạo lại tác vụ tải từ URL"));}
    storage::enqueue(app,id.clone()).await?;Ok(Json(json!({"id":id})))
}
pub async fn delete_user(State(app):State<App>,Extension(admin):Extension<User>,Path(id):Path<String>)->Result<Json<Value>> {
    admin.admin()?;let u:User=sqlx::query_as("SELECT * FROM users WHERE id=? AND role!='admin'").bind(&id).fetch_optional(&app.db).await?.ok_or_else(Error::forbidden)?;
    let active:i64=sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE owner=? AND status IN ('queued','hashing','telegram','downloading','processing','segments','muxing','resolving','batch','batch_pending')").bind(&id).fetch_one(&app.db).await?;if active>0{return Err(Error::bad("Hủy hoặc đợi các tác vụ của tài khoản trước khi xóa"));}
    let roots:Vec<String>=sqlx::query_scalar("SELECT id FROM nodes WHERE owner=? AND parent_id IS NULL").bind(&id).fetch_all(&app.db).await?;
    for root in roots{purge_node(&app,&u,&root).await?;}
    let tasks:Vec<String>=sqlx::query_scalar("SELECT id FROM jobs WHERE owner=?").bind(&id).fetch_all(&app.db).await?;for task in tasks{storage::discard_staging(&app,&task).await?;}
    sqlx::query("DELETE FROM users WHERE id=?").bind(&id).execute(&app.db).await?;db::audit(&app.db,&admin.id,"delete_user",&id).await;Ok(Json(json!({"ok":true})))
}

pub async fn children(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>)->Result<Json<Vec<crate::db::Job>>>{
    storage::get_job(&app,&u.id,&id).await?;
    Ok(Json(sqlx::query_as("SELECT * FROM jobs WHERE owner=? AND parent_job_id=? ORDER BY batch_index,created_at,id").bind(&u.id).bind(id).fetch_all(&app.db).await?))
}
