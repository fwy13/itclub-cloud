use crate::{db::{self,Job,Node,Part,User},error::{Error,Result},state::App,telegram::media_file};
use axum::{body::Body,http::{header,HeaderMap,StatusCode},response::{Response,IntoResponse}};
use bytes::Bytes;
use serde_json::json;
use sha2::{Digest,Sha256};
use std::{io::SeekFrom,path::{Path,PathBuf},time::Duration};
use tokio::io::{AsyncReadExt,AsyncSeekExt,AsyncWriteExt};
use tokio_util::sync::CancellationToken;

pub async fn create_job(app:&App,u:&User,name:&str,parent:Option<String>,size:i64,mime:&str,status:&str,replace:Option<String>)->Result<Job> {
    db::valid_name(name)?;db::parent(&app.db,&u.id,&parent).await?;
    if size<0||size as u64>app.cfg.max_upload{return Err(Error::bad("File vượt giới hạn MAX_UPLOAD_GIB"));}
    let lock=app.lock(format!("quota:{}",u.id));let _guard=lock.lock().await;
    if let Some(id)=&replace{let n=db::live_node(&app.db,&u.id,id).await?;if n.kind!="file"{return Err(Error::bad("Không thể ghi đè thư mục"));}}
    if u.quota_bytes>0 {
        let used:i64=sqlx::query_scalar("SELECT COALESCE(SUM(size),0) FROM nodes WHERE owner=? AND kind='file'").bind(&u.id).fetch_one(&app.db).await?;
        let reserved:i64=sqlx::query_scalar("SELECT COALESCE(SUM(size),0) FROM jobs WHERE owner=? AND status NOT IN ('done','cancelled','error')").bind(&u.id).fetch_one(&app.db).await?;
        if used.saturating_add(reserved).saturating_add(size)>u.quota_bytes{return Err(Error::bad("Không đủ hạn mức lưu trữ"));}
    }
    let id=db::id();sqlx::query("INSERT INTO jobs(id,owner,name,parent_id,size,mime,status,replace_id,part_size,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)").bind(&id).bind(&u.id).bind(name).bind(parent).bind(size).bind(mime).bind(status).bind(replace).bind(app.cfg.part_size as i64).bind(db::now()).bind(db::now()).execute(&app.db).await?;
    get_job(app,&u.id,&id).await
}
pub async fn get_job(app:&App,owner:&str,id:&str)->Result<Job>{sqlx::query_as("SELECT * FROM jobs WHERE id=? AND owner=?").bind(id).bind(owner).fetch_optional(&app.db).await?.ok_or_else(Error::not_found)}
pub async fn enqueue(app:App,id:String)->Result<()> {
    let mut tasks=app.cancel.lock().await;
    if tasks.contains_key(&id){return Ok(());}
    let cancel=CancellationToken::new();tasks.insert(id.clone(),cancel.clone());drop(tasks);
    sqlx::query("UPDATE jobs SET status='queued',error=NULL,updated_at=? WHERE id=?").bind(db::now()).bind(&id).execute(&app.db).await?;
    tokio::spawn(async move {let result=upload(&app,&id,&cancel).await;
        if let Err(e)=result {let state=if app.stopping.is_cancelled(){"interrupted"}else if cancel.is_cancelled(){"cancelled"}else{"error"};app.progress(&id,state,0,0,Some(&e.1)).await;
            if state=="cancelled"{let _=discard_staging(&app,&id).await;}}
        app.cancel.lock().await.remove(&id);
    });Ok(())
}
pub async fn upload(app:&App,id:&str,cancel:&CancellationToken)->Result<String> {
    let _slot=tokio::select! {p=app.uploads.clone().acquire_owned()=>p.map_err(|_|Error::bad("Server đang dừng"))?,_=cancel.cancelled()=>return Err(Error::bad("Đã hủy"))};
    if !app.tg.ready().await{return Err(Error::bad("Đăng nhập Telegram trước khi upload"));}
    let job:Job=sqlx::query_as("SELECT * FROM jobs WHERE id=?").bind(id).fetch_one(&app.db).await?;
    if job.status=="done"{return job.node_id.ok_or_else(Error::not_found);}
    let user:User=sqlx::query_as("SELECT * FROM users WHERE id=? AND disabled=0").bind(&job.owner).fetch_optional(&app.db).await?.ok_or_else(Error::forbidden)?;
    db::parent(&app.db,&job.owner,&job.parent_id).await?;
    let path=app.cfg.upload_path(id);let size=tokio::fs::metadata(&path).await?.len();
    if size>app.cfg.max_upload{return Err(Error::bad("File vượt giới hạn dung lượng"));}
    if size as i64!=job.size{return Err(Error::bad("Dung lượng file tạm không khớp"));}
    let chat=app.tg.chat().await?;
    let mut input=tokio::fs::File::open(&path).await?;
    let mut hasher=Sha256::new();let mut buf=vec![0;1024*1024];
    app.progress(id,"hashing",0,size,None).await;
    loop{if cancel.is_cancelled(){return Err(Error::bad("Đã hủy"));}let n=input.read(&mut buf).await?;if n==0{break;}hasher.update(&buf[..n]);}
    let etag=hex::encode(hasher.finalize());let part_size=job.part_size as u64;let part_count=size.div_ceil(part_size);
    for index in 0..part_count {
        if cancel.is_cancelled(){return Err(Error::bad("Đã hủy"));}
        let start=index*part_size;let length=(size-start).min(part_size);
        let existing:Option<i64>=sqlx::query_scalar("SELECT message_id FROM job_parts WHERE job_id=? AND part_index=?").bind(id).bind(index as i64).fetch_optional(&app.db).await?;
        if existing.is_some(){continue;}
        let part_path=app.cfg.data.join("temp").join(format!("{id}.{index}.part"));
        input.seek(SeekFrom::Start(start)).await?;
        let mut output=crate::disk::create_sized(app,&part_path,length,false).await?;
        let copied=tokio::io::copy(&mut (&mut input).take(length),&mut output).await?;
        output.flush().await?;drop(output);
        if copied!=length{return Err(Error::bad("Không đọc đủ dữ liệu của part"));}
        let tag=format!("tc_{}_{}",id.replace('-', ""),index);
        let account=app.tg.choose(chat).await?;
        app.progress(id,"telegram",start,size,None).await;
        let progress_app=app.clone();let jid=id.to_string();
        let sent=app.tg.send_document(&account,chat,&part_path,&tag,cancel,move|n|{
            let app=progress_app.clone();let jid=jid.clone();async move{app.progress(&jid,"telegram",start+n.min(length),size,None).await;}
        }).await?;
        let message_id=sent["id"].as_i64().ok_or_else(||Error::bad("Telegram không trả message ID"))?;
        sqlx::query("INSERT INTO job_parts(job_id,part_index,chat_id,message_id,account,size) VALUES(?,?,?,?,?,?)").bind(id).bind(index as i64).bind(chat).bind(message_id).bind(&account.key).bind(length as i64).execute(&app.db).await?;
        app.progress(id,"telegram",start+length,size,None).await;
        let _=tokio::fs::remove_file(part_path).await;
    }
    if cancel.is_cancelled(){return Err(Error::bad("Đã hủy"));}
    let owner_lock=app.lock(format!("files:{}",job.owner));let _guard=owner_lock.lock().await;
    db::parent(&app.db,&job.owner,&job.parent_id).await?;
    if user.quota_bytes>0 {
        let used:i64=sqlx::query_scalar("SELECT COALESCE(SUM(size),0) FROM nodes WHERE owner=? AND kind='file'").bind(&job.owner).fetch_one(&app.db).await?;
        let old=if let Some(r)=&job.replace_id{db::node(&app.db,&job.owner,r).await?.size}else{0};
        if used-old+job.size>user.quota_bytes{return Err(Error::bad("Vượt hạn mức lưu trữ"));}
    }
    let node_id=job.replace_id.clone().unwrap_or_else(db::id);
    let mut name=job.name.clone();
    if job.replace_id.is_none(){for i in 0..10000 {
        let found:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes WHERE owner=? AND parent_id IS ? AND name=? AND deleted_at IS NULL)").bind(&job.owner).bind(&job.parent_id).bind(&name).fetch_one(&app.db).await?;
        if !found{break;}name=alternative_name(&job.name,i+1);
    }}
    let mut tx=app.db.begin().await?;
    if job.replace_id.is_some() {
        sqlx::query("INSERT OR IGNORE INTO gc_messages(chat_id,message_id,account) SELECT chat_id,message_id,account FROM parts WHERE node_id=?").bind(&node_id).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM parts WHERE node_id=?").bind(&node_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE nodes SET size=?,mime=?,etag=?,updated_at=?,deleted_at=NULL WHERE id=? AND owner=?").bind(job.size).bind(&job.mime).bind(&etag).bind(db::now()).bind(&node_id).bind(&job.owner).execute(&mut *tx).await?;
    }else{
        sqlx::query("INSERT INTO nodes(id,owner,parent_id,name,kind,size,mime,etag,created_at,updated_at) VALUES(?,?,?,?,'file',?,?,?,?,?)").bind(&node_id).bind(&job.owner).bind(&job.parent_id).bind(&name).bind(job.size).bind(&job.mime).bind(&etag).bind(db::now()).bind(db::now()).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO parts(node_id,part_index,chat_id,message_id,account,size) SELECT ?,part_index,chat_id,message_id,account,size FROM job_parts WHERE job_id=? ORDER BY part_index").bind(&node_id).bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM job_parts WHERE job_id=?").bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM chunks WHERE job_id=?").bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE jobs SET node_id=?,name=?,status='done',progress=100,bytes_done=size,error=NULL,updated_at=? WHERE id=?").bind(&node_id).bind(&name).bind(db::now()).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    thumbnail(app,&path,&node_id,&job.mime).await;
    let _=tokio::fs::remove_file(&path).await;
    app.progress(id,"done",size,size,None).await;app.changed(&job.owner);
    db::audit(&app.db,&job.owner,"upload",&node_id).await;Ok(node_id)
}
fn alternative_name(name:&str,index:u32)->String {
    let path=Path::new(name);let ext=path.extension().and_then(|s|s.to_str()).unwrap_or("");let stem=path.file_stem().and_then(|s|s.to_str()).unwrap_or(name);
    let mut stem=stem.to_owned();while stem.len()+ext.len()+20>240{stem.pop();}
    if ext.is_empty(){format!("{stem} ({index})")}else{format!("{stem} ({index}).{ext}")}
}
pub async fn thumbnail(app:&App,path:&Path,node:&str,mime:&str) {
    if !(mime.starts_with("video/")||mime.starts_with("image/"))||app.cfg.ffmpeg=="disabled"{return;}
    let dest=app.cfg.data.join("thumbs").join(format!("{node}.jpg"));
    let mut command=tokio::process::Command::new(&app.cfg.ffmpeg);
    command.args(["-nostdin","-hide_banner","-loglevel","error","-y","-protocol_whitelist","file,crypto,data"]);
    if mime.starts_with("video/"){command.args(["-ss","1"]);}
    command.arg("-i").arg(path).args(["-frames:v","1","-vf","scale=360:240:force_original_aspect_ratio=decrease","-threads","1"]).arg(dest).kill_on_drop(true);
    let _=tokio::time::timeout(Duration::from_secs(30),command.output()).await;
}
pub async fn discard_staging(app:&App,id:&str)->Result<()> {
    let mut tx=app.db.begin().await?;
    sqlx::query("INSERT OR IGNORE INTO gc_messages(chat_id,message_id,account) SELECT chat_id,message_id,account FROM job_parts WHERE job_id=?").bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM job_parts WHERE job_id=?").bind(id).execute(&mut *tx).await?;sqlx::query("DELETE FROM chunks WHERE job_id=?").bind(id).execute(&mut *tx).await?;tx.commit().await?;
    let _=tokio::fs::remove_file(app.cfg.upload_path(id)).await;Ok(())
}
pub async fn parts(app:&App,id:&str)->Result<Vec<Part>> {Ok(sqlx::query_as("SELECT part_index,chat_id,message_id,account,size FROM parts WHERE node_id=? ORDER BY part_index").bind(id).fetch_all(&app.db).await?)}
async fn block(app:&App,part:&Part,offset:u64,len:u64)->anyhow::Result<Bytes> {
    let cache_key=format!("{}:{}:{offset}:{len}",part.chat_id,part.message_id);
    if let Some(b)=app.cache.get(&cache_key).await{return Ok(b);}
    let lock=app.lock(format!("tg-file:{}:{}",part.chat_id,part.message_id));let _guard=lock.lock().await;
    if let Some(b)=app.cache.get(&cache_key).await{return Ok(b);}
    let _budget=app.downloads.clone().acquire_owned().await?;
    let request=json!({"@type":"getMessage","chat_id":part.chat_id,"message_id":part.message_id});
    let (key,message)=match app.tg.call(&part.account,request.clone()).await {Ok(v)=>(part.account.as_str(),v),Err(_)=>("main",app.tg.call("main",request).await?)};
    let file=media_file(&message).ok_or_else(||anyhow::anyhow!("Tin nhắn Telegram không còn file"))?;
    let fid=file["id"].as_i64().ok_or_else(||anyhow::anyhow!("Telegram file ID missing"))?;
    let mut last_error=String::new();
    for attempt in 0..3 {
        crate::disk::require_free(app,len).await?;
        let result=app.tg.call(key,json!({"@type":"downloadFile","file_id":fid,"priority":32,"offset":offset,"limit":len,"synchronous":true})).await;
        match result {
            Ok(f)=>{
                let prefix=app.tg.call(key,json!({"@type":"getFileDownloadedPrefixSize","file_id":fid,"offset":offset})).await?;
                if prefix["size"].as_u64().unwrap_or(0)>=len||f["local"]["is_downloading_completed"]==true {
                    if let Some(path)=f["local"]["path"].as_str(){
                        let mut local=tokio::fs::File::open(path).await?;local.seek(SeekFrom::Start(offset)).await?;
                        let mut b=vec![0;len as usize];local.read_exact(&mut b).await?;let b=Bytes::from(b);app.cache.insert(cache_key,b.clone()).await;return Ok(b);
                    }
                }
                last_error="Telegram chưa tải đủ vùng dữ liệu".into();
            },Err(e)=>last_error=e.to_string(),
        }
        tokio::time::sleep(Duration::from_secs(1+attempt)).await;
    }
    anyhow::bail!("{last_error}")
}
pub fn byte_stream(app:App,parts:Vec<Part>,start:u64,end:u64)->impl futures_util::Stream<Item=std::result::Result<Bytes,std::io::Error>>+Send+'static {
    async_stream::try_stream!{
        let mut base=0u64;
        for part in parts {
            let stop=base+part.size as u64;
            if start<stop&&end>=base {
                let mut pos=start.saturating_sub(base);let limit=(end-base+1).min(part.size as u64);
                while pos<limit {
                    let aligned=pos/(1024*1024)*(1024*1024);let count=(part.size as u64-aligned).min(1024*1024);
                    let data=block(&app,&part,aligned,count).await.map_err(std::io::Error::other)?;
                    let begin=(pos-aligned) as usize;let take=(limit-pos).min(data.len() as u64-begin as u64) as usize;
                    if take==0{Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof,"Empty Telegram chunk"))?;}
                    yield data.slice(begin..begin+take);pos+=take as u64;
                }
            }
            base=stop;if base>end{break;}
        }
    }
}
pub fn range(headers:&HeaderMap,size:u64,etag:&str)->std::result::Result<(u64,u64,bool),()> {
    let default=(0,size.saturating_sub(1),false);
    if let Some(if_range)=headers.get(header::IF_RANGE).and_then(|h|h.to_str().ok()){if if_range!=format!("\"{etag}\""){return Ok(default);}}
    let Some(value)=headers.get(header::RANGE).and_then(|h|h.to_str().ok())else{return Ok(default);};
    let text=value.strip_prefix("bytes=").ok_or(())?;
    if text.contains(','){return Err(());} // Explicitly reject multipart ranges instead of returning incorrect bytes.
    let (a,b)=text.split_once('-').ok_or(())?;if size==0{return Err(());}
    if a.is_empty(){let suffix:u64=b.parse().map_err(|_|())?;if suffix==0{return Err(());}return Ok((size.saturating_sub(suffix),size-1,true));}
    let start:u64=a.parse().map_err(|_|())?;let end=if b.is_empty(){size-1}else{b.parse::<u64>().map_err(|_|())?.min(size-1)};
    if start>=size||start>end{return Err(());}Ok((start,end,true))
}
pub async fn serve(app:App,node:Node,headers:HeaderMap,head:bool,download:bool)->Result<Response> {
    if node.kind!="file"{return Err(Error::bad("Đây là thư mục"));}
    let etag=format!("\"{}\"",node.etag);
    if headers.get(header::IF_NONE_MATCH).and_then(|v|v.to_str().ok())==Some(etag.as_str()){return Ok((StatusCode::NOT_MODIFIED,[(header::ETAG,etag)]).into_response());}
    let (start,end,partial)=match range(&headers,node.size as u64,&node.etag){Ok(r)=>r,Err(_)=>return Ok((StatusCode::RANGE_NOT_SATISFIABLE,[(header::CONTENT_RANGE,format!("bytes */{}",node.size))]).into_response())};
    let length=if node.size==0{0}else{end-start+1};
    let parts=parts(&app,&node.id).await?;
    if parts.iter().map(|p|p.size).sum::<i64>()!=node.size{return Err(Error::bad("Danh sách part không khớp dung lượng file"));}
    let safe_inline=node.mime.starts_with("video/")||node.mime.starts_with("audio/")||matches!(node.mime.as_str(),"application/pdf"|"image/png"|"image/jpeg"|"image/gif"|"image/webp"|"image/avif"|"text/vtt");
    let disposition=if download||!safe_inline{"attachment"}else{"inline"};
    let name=percent_encoding::utf8_percent_encode(&node.name,percent_encoding::NON_ALPHANUMERIC);
    let mut response=Response::builder().status(if partial{StatusCode::PARTIAL_CONTENT}else{StatusCode::OK})
        .header(header::CONTENT_TYPE,&node.mime).header(header::CONTENT_LENGTH,length.to_string()).header(header::ACCEPT_RANGES,"bytes")
        .header(header::ETAG,etag).header(header::CACHE_CONTROL,"private, no-cache").header("x-accel-buffering","no")
        .header(header::CONTENT_DISPOSITION,format!("{disposition}; filename*=UTF-8''{name}"));
    if partial {response=response.header(header::CONTENT_RANGE,format!("bytes {start}-{end}/{}",node.size));}
    let body=if head||length==0 {Body::empty()} else {Body::from_stream(byte_stream(app,parts,start,end))};
    response.body(body).map_err(|e|Error::bad(e.to_string()))
}
pub async fn materialize(app:&App,node:&Node,path:&Path)->Result<()> {
    use futures_util::StreamExt;
    let parts=parts(app,&node.id).await?;let stream=byte_stream(app.clone(),parts,0,node.size.saturating_sub(1) as u64);tokio::pin!(stream);
    let mut file=crate::disk::create_sized(app,path,node.size.max(0) as u64,false).await?;let mut written=0;
    while let Some(b)=stream.next().await{let b=b?;written+=b.len() as i64;file.write_all(&b).await?;}
    file.flush().await?;if written!=node.size{return Err(Error::bad("Tải file không đủ dữ liệu"));}Ok(())
}
pub async fn collect_garbage(app:&App) {
    let rows:Vec<(i64,i64,String)>=sqlx::query_as("SELECT chat_id,message_id,account FROM gc_messages WHERE attempts<20 LIMIT 100").fetch_all(&app.db).await.unwrap_or_default();
    for (chat,msg,account) in rows {
        let used:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM parts WHERE chat_id=? AND message_id=? UNION ALL SELECT 1 FROM job_parts WHERE chat_id=? AND message_id=?)").bind(chat).bind(msg).bind(chat).bind(msg).fetch_one(&app.db).await.unwrap_or(true);
        if used{let _=sqlx::query("DELETE FROM gc_messages WHERE chat_id=? AND message_id=?").bind(chat).bind(msg).execute(&app.db).await;continue;}
        match app.tg.delete(&account,chat,msg).await {
            Ok(())=>{let _=sqlx::query("DELETE FROM gc_messages WHERE chat_id=? AND message_id=?").bind(chat).bind(msg).execute(&app.db).await;},
            Err(e)=>{let _=sqlx::query("UPDATE gc_messages SET attempts=attempts+1,last_error=? WHERE chat_id=? AND message_id=?").bind(e.to_string()).bind(chat).bind(msg).execute(&app.db).await;},
        }
    }
}
pub async fn await_job(app:&App,id:&str)->Result<Node> {
    enqueue(app.clone(),id.to_owned()).await?;
    loop {
        let j:Job=sqlx::query_as("SELECT * FROM jobs WHERE id=?").bind(id).fetch_one(&app.db).await?;
        match j.status.as_str(){"done"=>return db::live_node(&app.db,&j.owner,j.node_id.as_deref().ok_or_else(Error::not_found)?).await,"error"|"cancelled"|"interrupted"=>return Err(Error::bad(j.error.unwrap_or(j.status))),_=>{}}
        tokio::select!{_=tokio::time::sleep(Duration::from_millis(350))=>{},_=app.stopping.cancelled()=>return Err(Error::bad("Server đang dừng; tác vụ được lưu để thử lại"))}
    }
}
