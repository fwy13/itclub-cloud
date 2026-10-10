use crate::{db::{self,User,Job},error::{Error,Result},state::App,storage};
use axum::{extract::State,Extension,Json};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json,Value};
use std::{net::IpAddr,path::PathBuf,process::Stdio,time::Duration};
use tokio::io::{AsyncReadExt,AsyncWriteExt};
use tokio_util::sync::CancellationToken;

fn public_ip(ip:IpAddr)->bool {
    match ip {
        IpAddr::V4(v)=>{
            let o=v.octets();!v.is_private()&&!v.is_loopback()&&!v.is_link_local()&&!v.is_broadcast()&&!v.is_multicast()&&!v.is_unspecified()&&!v.is_documentation()
                &&o[0]!=0&&o[0]<224&&!(o[0]==100&&(64..128).contains(&o[1]))&&!(o[0]==198&&(o[1]==18||o[1]==19))&&!(o[0]==192&&o[1]==0&&o[2]==0)
        },
        IpAddr::V6(v)=>{if let Some(v4)=v.to_ipv4_mapped(){return public_ip(IpAddr::V4(v4));}let s=v.segments();!v.is_loopback()&&!v.is_unspecified()&&(s[0]&0xe000)==0x2000&&!(s[0]==0x2001&&s[1]==0x0db8)&&!(s[0]==0x2002)&&!(s[0]==0x2001&&s[1]==0)},
    }
}
pub async fn validate_url(text:&str)->Result<(url::Url,Vec<std::net::SocketAddr>)> {
    let url=url::Url::parse(text).map_err(|_|Error::bad("URL không hợp lệ"))?;
    if !matches!(url.scheme(),"http"|"https")||!url.username().is_empty()||url.password().is_some(){return Err(Error::bad("Chỉ hỗ trợ URL HTTP/HTTPS công khai không kèm thông tin đăng nhập"));}
    let host=url.host_str().ok_or_else(||Error::bad("Thiếu hostname"))?;
    let port=url.port_or_known_default().unwrap_or(443);
    let addresses:Vec<_>=tokio::net::lookup_host((host,port)).await?.collect();
    if addresses.is_empty()||addresses.iter().any(|a|!public_ip(a.ip())){return Err(Error::bad("URL trỏ vào mạng nội bộ hoặc địa chỉ không công khai"));}Ok((url,addresses))
}
pub async fn safe_get(text:&str)->Result<reqwest::Response> {
    safe_get_headers(text,&crate::modules::Headers::new()).await
}
pub async fn safe_get_headers(text:&str,extra:&crate::modules::Headers)->Result<reqwest::Response> {
    let mut current=text.to_string();
    let original=url::Url::parse(text).map_err(|_|Error::bad("URL không hợp lệ"))?;
    for _ in 0..6 {
        let (url,addresses)=validate_url(&current).await?;
        // Pin DNS for the actual connection. Every redirect is separately resolved and checked.
        let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).connect_timeout(Duration::from_secs(20)).read_timeout(Duration::from_secs(120)).user_agent(concat!("ITClub-Cloud/",env!("CARGO_PKG_VERSION"))).resolve_to_addrs(url.host_str().unwrap(),&addresses).build().map_err(anyhow::Error::from)?;
        let mut request=client.get(url.clone());
        for (name,value) in extra {
            let lower=name.to_ascii_lowercase();
            if matches!(lower.as_str(),"host"|"connection"|"content-length"|"transfer-encoding"|"proxy-authorization"){continue;}
            if url.origin()!=original.origin()&&matches!(lower.as_str(),"authorization"|"cookie"){continue;}
            let name=reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_|Error::bad("Header name không hợp lệ"))?;
            let value=reqwest::header::HeaderValue::from_str(value).map_err(|_|Error::bad("Header value không hợp lệ"))?;
            request=request.header(name,value);
        }
        let response=request.send().await.map_err(anyhow::Error::from)?;
        if response.status().is_redirection(){let loc=response.headers().get(reqwest::header::LOCATION).and_then(|s|s.to_str().ok()).ok_or_else(||Error::bad("Redirect không có đích"))?;current=url.join(loc).map_err(|e|Error::bad(e.to_string()))?.to_string();continue;}
        return response.error_for_status().map_err(anyhow::Error::from).map_err(Error::from);
    }Err(Error::bad("URL redirect quá nhiều lần"))
}
#[derive(Deserialize)]pub struct Remote {pub url:String,pub kind:Option<String>,pub parent_id:Option<String>,pub name:Option<String>,pub options:Option<Value>}
pub async fn modules(State(app):State<App>,Extension(u):Extension<User>)->Json<Value>{Json(json!(app.modules.list(u.role=="admin")))}
pub use crate::downloads::{start,has_source,retry};
pub(crate) async fn download_url(app:&App,_u:&User,job:&Job,url:&str,headers:&crate::modules::Headers,cancel:&CancellationToken)->Result<()> {
    let response=safe_get_headers(url,headers).await?;let size=response.content_length().unwrap_or(0);
    if size>app.cfg.max_upload{return Err(Error::bad("File vượt giới hạn dung lượng"));}
    let mime=response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|h|h.to_str().ok()).map(|s|s.split(';').next().unwrap_or(s).to_owned()).unwrap_or_else(||mime_guess::from_path(&job.name).first_or_octet_stream().to_string());
    let mut body=response.bytes_stream();let mut out=tokio::fs::File::create(app.cfg.upload_path(&job.id)).await?;let mut total=0u64;let mut last=std::time::Instant::now();
    loop {let part=tokio::select!{_=cancel.cancelled()=>return Err(Error::bad("Đã hủy")),p=body.next()=>p};let Some(part)=part else{break;};let bytes=part.map_err(anyhow::Error::from)?;
        total+=bytes.len() as u64;if total>app.cfg.max_upload{return Err(Error::bad("File vượt giới hạn dung lượng"));}crate::disk::write(app,&mut out,&bytes,true).await?;
        if last.elapsed()>Duration::from_millis(500){app.progress(&job.id,"downloading",total,size,None).await;last=std::time::Instant::now();}
    }
    out.sync_all().await?;drop(out);if size>0&&total!=size{return Err(Error::bad("Nguồn tải trả thiếu dữ liệu"));}
    sqlx::query("UPDATE jobs SET size=?,mime=? WHERE id=?").bind(total as i64).bind(mime).bind(&job.id).execute(&app.db).await?;
    crate::downloads::staged(app,&job.id).await?;
    storage::upload(app,&job.id,cancel).await?;Ok(())
}
pub(crate) async fn accept_local(ctx:&crate::modules::ModuleContext,path:&std::path::Path,name:&str,mime:&str)->Result<()> {
    db::valid_name(name)?;ctx.check_cancelled()?;
    let work=tokio::fs::canonicalize(ctx.work_dir().await?).await?;
    let path=tokio::fs::canonicalize(path).await?;
    if !path.starts_with(&work){return Err(Error::bad("Module chỉ được trả file nằm trong work_dir của tác vụ"));}
    let meta=tokio::fs::metadata(&path).await?;if !meta.is_file()||meta.len()>ctx.app.cfg.max_upload{return Err(Error::bad("File module không hợp lệ hoặc quá lớn"));}
    tokio::fs::rename(path,ctx.app.cfg.upload_path(&ctx.job.id)).await?;
    sqlx::query("UPDATE jobs SET name=?,size=?,mime=? WHERE id=?").bind(name).bind(meta.len() as i64).bind(mime).bind(&ctx.job.id).execute(&ctx.app.db).await?;
    crate::downloads::staged(&ctx.app,&ctx.job.id).await?;
    storage::upload(&ctx.app,&ctx.job.id,&ctx.cancel).await?;
    let _=tokio::fs::remove_dir_all(work).await;Ok(())
}
pub(crate) async fn download_telegram(app:&App,_u:&User,job:&Job,url:&str,cancel:&CancellationToken)->Result<()> {
    let parsed=url::Url::parse(url).map_err(|_|Error::bad("Liên kết Telegram không hợp lệ"))?;
    if !matches!(parsed.host_str(),Some("t.me")|Some("telegram.me")){return Err(Error::bad("Nhập liên kết https://t.me/..."));}
    let info=app.tg.call("main",json!({"@type":"getMessageLinkInfo","url":url})).await?;
    let message=&info["message"];let file=crate::telegram::media_file(message).ok_or_else(||Error::bad("Không đọc được file; tài khoản Telegram cần quyền truy cập tin nhắn"))?;
    let size=file["size"].as_u64().unwrap_or(0);if size>app.cfg.max_upload{return Err(Error::bad("File quá lớn"));}
    let id=file["id"].as_i64().ok_or_else(||Error::bad("Không có file ID"))?;
    crate::disk::require_free(app,size).await?;
    app.tg.call("main",json!({"@type":"downloadFile","file_id":id,"priority":16,"offset":0,"limit":0,"synchronous":false})).await?;
    let path=loop {
        if let Err(e)=crate::disk::check(app).await{let _=app.tg.call("main",json!({"@type":"cancelDownloadFile","file_id":id,"only_if_pending":false})).await;return Err(e);}
        if cancel.is_cancelled(){let _=app.tg.call("main",json!({"@type":"cancelDownloadFile","file_id":id,"only_if_pending":false})).await;return Err(Error::bad("Đã hủy"));}
        let downloaded=app.tg.call("main",json!({"@type":"getFile","file_id":id})).await?;
        app.progress(&job.id,"downloading",downloaded["local"]["downloaded_size"].as_u64().unwrap_or(0),size,None).await;
        if downloaded["local"]["is_downloading_completed"]==true{break downloaded["local"]["path"].as_str().ok_or_else(||Error::bad("Không có file tải xuống"))?.to_owned();}
        tokio::select!{_=cancel.cancelled()=>{},_=tokio::time::sleep(Duration::from_secs(1))=>{}}
    };
    let name=message["content"]["document"]["file_name"].as_str().or_else(||message["content"]["video"]["file_name"].as_str()).filter(|s|!s.is_empty()).unwrap_or(&job.name);
    db::valid_name(name)?;crate::disk::copy(app,std::path::Path::new(&path),&app.cfg.upload_path(&job.id),true).await?;
    let actual=tokio::fs::metadata(app.cfg.upload_path(&job.id)).await?.len();let mime=mime_guess::from_path(name).first_or_octet_stream().to_string();
    sqlx::query("UPDATE jobs SET name=?,size=?,mime=? WHERE id=?").bind(name).bind(actual as i64).bind(mime).bind(&job.id).execute(&app.db).await?;
    crate::downloads::staged(app,&job.id).await?;
    storage::upload(app,&job.id,cancel).await?;Ok(())
}
pub(crate) async fn external(app:&App,u:&User,job:&Job,kind:&str,url:&str,cancel:&CancellationToken)->Result<()> {
    let dir=app.cfg.data.join("temp").join(format!("remote-{}",job.id));tokio::fs::create_dir_all(&dir).await?;
    let mut cmd=if kind=="ytdlp" {let mut c=tokio::process::Command::new(&app.cfg.ytdlp);c.args(["--no-playlist","--no-config","--restrict-filenames","--no-progress","--max-filesize",&app.cfg.max_upload.to_string(),"--paths"]).arg(&dir).args(["--output","%(title).160B.%(ext)s","--",url]);c}
    else {let mut c=tokio::process::Command::new(&app.cfg.aria2);c.args(["--no-conf=true","--enable-rpc=false","--seed-time=0","--file-allocation=none","--allow-overwrite=false","--auto-file-renaming=true","--bt-max-peers=24","--max-concurrent-downloads=2","--summary-interval=0","--dir"]).arg(&dir).arg("--").arg(url);c};
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(unix)]{cmd.process_group(0);}
    crate::disk::check(app).await?;
    let mut child=cmd.spawn().map_err(|e|Error::bad(format!("Không chạy được {kind}: {e}")))?;
    let stderr=child.stderr.take();let log_task=tokio::spawn(async move{let mut tail=Vec::new();if let Some(mut stderr)=stderr{let mut buf=[0;1024];while let Ok(n)=stderr.read(&mut buf).await{if n==0{break;}tail.extend_from_slice(&buf[..n]);if tail.len()>8192{tail.drain(..tail.len()-8192);}}}String::from_utf8_lossy(&tail).into_owned()});
    let mut ticker=tokio::time::interval(Duration::from_secs(2));
    let result=loop{tokio::select!{
        status=child.wait()=>break status?,
        _=cancel.cancelled()=>{kill_group(&mut child).await;let _=tokio::fs::remove_dir_all(&dir).await;log_task.abort();return Err(Error::bad("Đã hủy"));},
        _=ticker.tick()=>{if let Err(e)=crate::disk::check(app).await{kill_group(&mut child).await;let _=tokio::fs::remove_dir_all(&dir).await;log_task.abort();return Err(e);}let bytes=directory_files(&dir).await?.iter().map(|(_,n)|*n).sum::<u64>();if bytes>app.cfg.max_upload{kill_group(&mut child).await;let _=tokio::fs::remove_dir_all(&dir).await;log_task.abort();return Err(Error::bad("Nguồn tải vượt giới hạn dung lượng"));}app.progress(&job.id,"downloading",bytes,0,None).await;}
    }};
    let log=log_task.await.unwrap_or_default();if !result.success(){return Err(Error::bad(format!("{kind} thất bại: {log}")));}
    let files:Vec<_>=directory_files(&dir).await?.into_iter().filter(|(p,_)|!matches!(p.extension().and_then(|s|s.to_str()),Some("aria2"|"part"|"ytdl"))).collect();
    if files.is_empty(){return Err(Error::bad("Không nhận được file nào"));}
    let folder=if files.len()>1 {let id=db::id();sqlx::query("INSERT INTO nodes(id,owner,parent_id,name,kind,created_at,updated_at) VALUES(?,?,?,?,'folder',?,?)").bind(&id).bind(&u.id).bind(&job.parent_id).bind(format!("{}-{}",job.name,&job.id[..8])).bind(db::now()).bind(db::now()).execute(&app.db).await?;Some(id)}else{job.parent_id.clone()};
    let multi=files.len()>1;let mut completed=0u64;
    let total=files.iter().map(|(_,size)|*size).sum::<u64>();
    if multi {sqlx::query("UPDATE jobs SET size=? WHERE id=?").bind(total as i64).bind(&job.id).execute(&app.db).await?;}
    for (path,size) in &files {
        if cancel.is_cancelled(){return Err(Error::bad("Đã hủy"));}
        let name=path.file_name().and_then(|s|s.to_str()).ok_or_else(||Error::bad("Tên file không hỗ trợ"))?;
        let mime=mime_guess::from_path(path).first_or_octet_stream().to_string();
        let mut destination=folder.clone();
        if multi {
            let relative=path.strip_prefix(&dir).map_err(|_|Error::bad("Đường dẫn tải không hợp lệ"))?;
            if let Some(nested)=relative.parent(){for segment in nested.components(){
                let segment=segment.as_os_str().to_str().ok_or_else(||Error::bad("Tên thư mục không hợp lệ"))?;db::valid_name(segment)?;
                let existing:Option<String>=sqlx::query_scalar("SELECT id FROM nodes WHERE owner=? AND parent_id IS ? AND name=? AND kind='folder' AND deleted_at IS NULL").bind(&u.id).bind(&destination).bind(segment).fetch_optional(&app.db).await?;
                if let Some(id)=existing{destination=Some(id);}else{let id=db::id();sqlx::query("INSERT INTO nodes(id,owner,parent_id,name,kind,created_at,updated_at) VALUES(?,?,?,?,'folder',?,?)").bind(&id).bind(&u.id).bind(&destination).bind(segment).bind(db::now()).bind(db::now()).execute(&app.db).await?;destination=Some(id);}
            }}
        }
        let target=if multi{storage::create_job(app,u,name,destination,*size as i64,&mime,"queued",None).await?.id}
            else{sqlx::query("UPDATE jobs SET name=?,size=?,mime=? WHERE id=?").bind(name).bind(*size as i64).bind(&mime).bind(&job.id).execute(&app.db).await?;job.id.clone()};
        tokio::fs::rename(path,app.cfg.upload_path(&target)).await?;
        if multi {
            sqlx::query("UPDATE jobs SET parent_job_id=? WHERE id=?").bind(&job.id).bind(&target).execute(&app.db).await?;
            app.cancel.lock().await.insert(target.clone(),cancel.clone());
        }else{crate::downloads::staged(app,&target).await?;}
        let result=storage::upload(app,&target,cancel).await;
        if multi {app.cancel.lock().await.remove(&target);}
        if let Err(e)=result{app.progress(&target,if cancel.is_cancelled(){"cancelled"}else{"error"},0,0,Some(&e.1)).await;return Err(e);}
        completed+=*size;
        if multi {app.progress(&job.id,"processing",completed,total,None).await;}
    }
    if multi {sqlx::query("UPDATE jobs SET node_id=? WHERE id=?").bind(&folder).bind(&job.id).execute(&app.db).await?;app.progress(&job.id,"done",total,total,None).await;}
    let _=tokio::fs::remove_dir_all(&dir).await;app.changed(&u.id);Ok(())
}

async fn kill_group(child:&mut tokio::process::Child){
    #[cfg(unix)]if let Some(id)=child.id(){unsafe{libc::kill(-(id as i32),libc::SIGTERM);}}
    let _=child.kill().await;let _=child.wait().await;
}
async fn directory_files(root:&std::path::Path)->Result<Vec<(PathBuf,u64)>> {
    let mut dirs=vec![root.to_owned()];let mut files=vec![];
    while let Some(dir)=dirs.pop(){let mut read=tokio::fs::read_dir(dir).await?;while let Some(entry)=read.next_entry().await?{let ty=entry.file_type().await?;if ty.is_symlink(){continue;}if ty.is_dir(){dirs.push(entry.path());}else if ty.is_file(){files.push((entry.path(),entry.metadata().await?.len()));}if dirs.len()+files.len()>10000{return Err(Error::bad("Quá nhiều file trong tác vụ"));}}}Ok(files)
}
