use crate::{auth,db::{self,Node,User},error::{Error,Result},files,storage,state::App};
use axum::{extract::{State,Request,ConnectInfo},body::Body,http::{HeaderMap,Method,StatusCode},response::{Response,IntoResponse}};
use futures_util::StreamExt;
use serde_json::json;
use tokio::io::AsyncWriteExt;
use std::net::SocketAddr;

pub fn escape(s:&str)->String{s.replace('&',"&amp;").replace('<',"&lt;").replace('>',"&gt;").replace('"',"&quot;").replace('\'',"&apos;")}
pub fn encode_path(s:&str)->String{s.split('/').map(|p|percent_encoding::utf8_percent_encode(p,percent_encoding::NON_ALPHANUMERIC).to_string()).collect::<Vec<_>>().join("/")}
pub fn xml(status:StatusCode,body:String)->Response{(status,[("content-type","application/xml; charset=utf-8")],body).into_response()}
fn decode_path(path:&str)->Result<String>{let decoded=percent_encoding::percent_decode_str(path).decode_utf8().map_err(|_|Error::bad("Invalid UTF-8 path"))?;for part in decoded.split('/').filter(|s|!s.is_empty()){db::valid_name(part)?;}Ok(decoded.trim_matches('/').to_owned())}
pub async fn handle(State(app):State<App>,ConnectInfo(addr):ConnectInfo<SocketAddr>,req:Request)->Response {
    if req.method()==Method::OPTIONS{return (StatusCode::NO_CONTENT,[("dav","1, 2"),("allow","OPTIONS, PROPFIND, GET, HEAD, PUT, DELETE, MKCOL, MOVE, COPY, LOCK, UNLOCK, PROPPATCH")]).into_response();}
    let rate_key=format!("dav:{}",addr.ip());
    if let Err(e)=auth::check_rate(&app,&rate_key).await{return e.into_response();}
    let user=match auth::basic(&app,req.headers()).await{Ok(u)=>u,Err(_)=>{let _=auth::rate(&app,rate_key).await;return (StatusCode::UNAUTHORIZED,[("www-authenticate","Basic realm=\"ITClub Cloud\", charset=\"UTF-8\"")]).into_response();}};
    match dispatch(app,user,req).await{Ok(r)=>r,Err(e)=>xml(e.0,format!("<?xml version=\"1.0\"?><d:error xmlns:d=\"DAV:\"><d:responsedescription>{}</d:responsedescription></d:error>",escape(&e.1)))}
}
async fn dispatch(app:App,u:User,req:Request)->Result<Response> {
    let (parts,body)=req.into_parts();let method=parts.method.as_str();let headers=parts.headers;
    let path=decode_path(parts.uri.path().strip_prefix("/webdav").unwrap_or(""))?;
    let existing=db::resolve_path(&app.db,&u.id,&path).await;
    if !matches!(method,"GET"|"HEAD"|"PROPFIND"|"OPTIONS"|"LOCK"|"UNLOCK") {check_locks(&app,&u,&path,&headers).await?;}
    match method {
        "GET"|"HEAD"=>{let n=existing?.ok_or_else(||Error::bad("Dùng PROPFIND để liệt kê thư mục"))?;storage::serve(app,n,headers,method=="HEAD",false).await},
        "PROPFIND"=>{
            let depth=headers.get("depth").and_then(|h|h.to_str().ok()).unwrap_or("1");if depth=="infinity"{return Err(Error(StatusCode::FORBIDDEN,"Depth infinity không được hỗ trợ".into()));}
            let root=existing?;let mut nodes=vec![(path.clone(),root.clone())];
            if depth!="0"&&root.as_ref().map(|n|n.kind=="folder").unwrap_or(true){let parent=root.as_ref().map(|n|n.id.clone());let children:Vec<Node>=sqlx::query_as("SELECT * FROM nodes WHERE owner=? AND parent_id IS ? AND deleted_at IS NULL").bind(&u.id).bind(parent).fetch_all(&app.db).await?;for n in children{let p=if path.is_empty(){n.name.clone()}else{format!("{path}/{}",n.name)};nodes.push((p,Some(n)));}}
            let mut out=String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?><d:multistatus xmlns:d=\"DAV:\">");
            for (p,n) in nodes {
                let folder=n.as_ref().map(|n|n.kind=="folder").unwrap_or(true);let display=n.as_ref().map(|n|n.name.as_str()).unwrap_or("ITClub Cloud");let size=n.as_ref().map(|n|n.size).unwrap_or(0);let modified=n.as_ref().map(|n|n.updated_at).unwrap_or(db::now());let mime=n.as_ref().map(|n|n.mime.as_str()).unwrap_or("httpd/unix-directory");let etag=n.as_ref().map(|n|n.etag.as_str()).unwrap_or("root");
                let href=format!("/webdav/{}{}",encode_path(&p),if folder&&!p.is_empty(){"/"}else{""});
                let time=httpdate::fmt_http_date(std::time::UNIX_EPOCH+std::time::Duration::from_secs(modified.max(0) as u64));
                out.push_str(&format!("<d:response><d:href>{}</d:href><d:propstat><d:prop><d:displayname>{}</d:displayname><d:resourcetype>{}</d:resourcetype><d:getcontentlength>{size}</d:getcontentlength><d:getcontenttype>{}</d:getcontenttype><d:getlastmodified>{time}</d:getlastmodified><d:getetag>&quot;{}&quot;</d:getetag><d:supportedlock><d:lockentry><d:lockscope><d:exclusive/></d:lockscope><d:locktype><d:write/></d:locktype></d:lockentry></d:supportedlock></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>",escape(&href),escape(display),if folder{"<d:collection/>"}else{""},escape(mime),escape(etag)));
            }out.push_str("</d:multistatus>");Ok(xml(StatusCode::MULTI_STATUS,out))
        },
        "MKCOL"=>{
            if path.is_empty()||existing.is_ok(){return Err(Error(StatusCode::METHOD_NOT_ALLOWED,"Thư mục đã tồn tại".into()));}
            let (dir,name)=split(&path)?;let parent=resolve_parent(&app,&u,&dir).await?;let id=db::id();
            sqlx::query("INSERT INTO nodes(id,owner,parent_id,name,kind,created_at,updated_at) VALUES(?,?,?,?,'folder',?,?)").bind(id).bind(&u.id).bind(parent).bind(name).bind(db::now()).bind(db::now()).execute(&app.db).await?;app.changed(&u.id);Ok(StatusCode::CREATED.into_response())
        },
        "PUT"=>{
            let old=existing.ok().flatten();preconditions(&headers,old.as_ref())?;
            let (dir,name)=split(&path)?;let parent=resolve_parent(&app,&u,&dir).await?;
            let mime=headers.get("content-type").and_then(|v|v.to_str().ok()).unwrap_or("application/octet-stream");
            let n=put_body(&app,&u,&name,parent,mime,old.as_ref().map(|n|n.id.clone()),body,None).await?;
            Ok((if old.is_some(){StatusCode::NO_CONTENT}else{StatusCode::CREATED},[("etag",format!("\"{}\"",n.etag))]).into_response())
        },
        "DELETE"=>{let n=existing?.ok_or_else(Error::forbidden)?;preconditions(&headers,Some(&n))?;files::trash_node(&app,&u,&n.id).await?;Ok(StatusCode::NO_CONTENT.into_response())},
        "MOVE"|"COPY"=>{
            let source=existing?.ok_or_else(Error::forbidden)?;
            let dest=headers.get("destination").and_then(|h|h.to_str().ok()).ok_or_else(||Error::bad("Thiếu Destination"))?;
            let url=url::Url::parse(&app.cfg.origin).map_err(|e|Error::bad(e.to_string()))?.join(dest).map_err(|e|Error::bad(e.to_string()))?;
            if url.origin().ascii_serialization()!=app.cfg.origin{return Err(Error::forbidden());}
            let destination=decode_path(url.path().strip_prefix("/webdav/").ok_or_else(Error::forbidden)?)?;
            if destination==path{return Err(Error::bad("Đích trùng với nguồn"));}check_locks(&app,&u,&destination,&headers).await?;
            let (dir,name)=split(&destination)?;let parent=resolve_parent(&app,&u,&dir).await?;
            if let Some(p)=&parent{if db::within(&app.db,&source,p).await?{return Err(Error::bad("Đích nằm trong nguồn"));}}
            let target=db::resolve_path(&app.db,&u.id,&destination).await.ok().flatten();
            if target.is_some()&&headers.get("overwrite").and_then(|h|h.to_str().ok())==Some("F"){return Err(Error(StatusCode::PRECONDITION_FAILED,"Đích đã tồn tại".into()));}
            if let Some(n)=&target{files::trash_node(&app,&u,&n.id).await?;}
            if method=="MOVE"{sqlx::query("UPDATE nodes SET parent_id=?,name=?,updated_at=? WHERE id=?").bind(parent).bind(name).bind(db::now()).bind(&source.id).execute(&app.db).await?;}
            else{files::copy_node(&app,&u,&source.id,parent,Some(name)).await?;}
            app.changed(&u.id);Ok((if target.is_some(){StatusCode::NO_CONTENT}else{StatusCode::CREATED}).into_response())
        },
        "LOCK"=>{
            let _body=axum::body::to_bytes(body,64*1024).await.map_err(|_|Error::bad("Lock request quá lớn"))?;
            let token=if let Some((token,))=sqlx::query_as::<_,(String,)>("SELECT token FROM dav_locks WHERE owner=? AND path=? AND expires_at>?").bind(&u.id).bind(&path).bind(db::now()).fetch_optional(&app.db).await? {
                if !headers.get("if").and_then(|h|h.to_str().ok()).unwrap_or("").contains(token.as_str()){return Err(Error(StatusCode::LOCKED,"Tài nguyên đang khóa".into()));}token
            }else{format!("opaquelocktoken:{}",db::id())};
            sqlx::query("INSERT INTO dav_locks(path,owner,token,expires_at) VALUES(?,?,?,?) ON CONFLICT(token) DO UPDATE SET expires_at=excluded.expires_at").bind(&path).bind(&u.id).bind(&token).bind(db::now()+3600).execute(&app.db).await?;
            let out=format!("<?xml version=\"1.0\"?><d:prop xmlns:d=\"DAV:\"><d:lockdiscovery><d:activelock><d:locktype><d:write/></d:locktype><d:lockscope><d:exclusive/></d:lockscope><d:depth>infinity</d:depth><d:timeout>Second-3600</d:timeout><d:locktoken><d:href>{}</d:href></d:locktoken><d:lockroot><d:href>/webdav/{}</d:href></d:lockroot></d:activelock></d:lockdiscovery></d:prop>",escape(&token),escape(&encode_path(&path)));
            let mut res=xml(StatusCode::OK,out);res.headers_mut().insert("lock-token",format!("<{token}>").parse().unwrap());Ok(res)
        },
        "UNLOCK"=>{let token=headers.get("lock-token").and_then(|h|h.to_str().ok()).unwrap_or("").trim_matches(['<','>']);let r=sqlx::query("DELETE FROM dav_locks WHERE token=? AND owner=? AND path=?").bind(token).bind(&u.id).bind(&path).execute(&app.db).await?;if r.rows_affected()==0{return Err(Error(StatusCode::CONFLICT,"Lock token không hợp lệ".into()));}Ok(StatusCode::NO_CONTENT.into_response())},
        "PROPPATCH"=>Err(Error(StatusCode::FORBIDDEN,"Dead properties chưa được hỗ trợ; dùng MOVE để đổi tên".into())),
        _=>Err(Error(StatusCode::METHOD_NOT_ALLOWED,"WebDAV method không hỗ trợ".into())),
    }
}
pub fn split(path:&str)->Result<(String,String)>{let path=path.trim_matches('/');let (dir,name)=path.rsplit_once('/').unwrap_or(("",path));db::valid_name(name)?;Ok((dir.into(),name.into()))}
async fn resolve_parent(app:&App,u:&User,path:&str)->Result<Option<String>> {let parent=db::resolve_path(&app.db,&u.id,path).await.map_err(|_|Error(StatusCode::CONFLICT,"Thư mục cha chưa tồn tại".into()))?;if parent.as_ref().is_some_and(|n|n.kind!="folder"){return Err(Error::bad("Thư mục cha không hợp lệ"));}Ok(parent.map(|n|n.id))}
pub fn preconditions(headers:&HeaderMap,node:Option<&Node>)->Result<()> {
    if let Some(value)=headers.get("if-match").and_then(|h|h.to_str().ok()){if !node.is_some_and(|n|value=="*"||value==format!("\"{}\"",n.etag)){return Err(Error(StatusCode::PRECONDITION_FAILED,"ETag không khớp".into()));}}
    if headers.get("if-none-match").and_then(|h|h.to_str().ok())==Some("*")&&node.is_some(){return Err(Error(StatusCode::PRECONDITION_FAILED,"Đối tượng đã tồn tại".into()));}Ok(())
}
async fn check_locks(app:&App,u:&User,path:&str,headers:&HeaderMap)->Result<()> {
    let rows:Vec<(String,String)>=sqlx::query_as("SELECT path,token FROM dav_locks WHERE owner=? AND expires_at>?").bind(&u.id).bind(db::now()).fetch_all(&app.db).await?;
    let tokens=format!("{} {}",headers.get("if").and_then(|h|h.to_str().ok()).unwrap_or(""),headers.get("lock-token").and_then(|h|h.to_str().ok()).unwrap_or(""));
    for (p,t) in rows{if (p.is_empty()||path.is_empty()||path==p||path.starts_with(&format!("{p}/"))||p.starts_with(&format!("{path}/")))&&!tokens.contains(t.as_str()){return Err(Error(StatusCode::LOCKED,"Tài nguyên đang khóa".into()));}}Ok(())
}
pub async fn put_body(app:&App,u:&User,name:&str,parent:Option<String>,mime:&str,replace:Option<String>,body:Body,expected_hash:Option<&str>)->Result<Node> {
    use sha2::{Digest,Sha256};
    let job=storage::create_job(app,u,name,parent,0,mime,"staging",replace).await?;
    let mut stream=body.into_data_stream();let mut file=tokio::fs::File::create(app.cfg.upload_path(&job.id)).await?;let mut size=0u64;let mut hash=Sha256::new();
    let result:Result<()>=async {while let Some(chunk)=stream.next().await{let chunk=chunk.map_err(|e|Error::bad(e.to_string()))?;size+=chunk.len() as u64;if size>app.cfg.max_upload{return Err(Error::bad("File vượt dung lượng cho phép"));}hash.update(&chunk);crate::disk::write(app,&mut file,&chunk,true).await?;}file.sync_all().await?;Ok(())}.await;
    if let Err(e)=result{drop(file);let _=storage::discard_staging(app,&job.id).await;app.progress(&job.id,"error",0,0,Some(&e.1)).await;return Err(e);}drop(file);
    if let Some(expected)=expected_hash.filter(|x|*x!="UNSIGNED-PAYLOAD"){if hex::encode(hash.finalize())!=expected{let _=storage::discard_staging(app,&job.id).await;app.progress(&job.id,"error",0,0,Some("Payload hash mismatch")).await;return Err(Error::bad("Payload hash mismatch"));}}
    sqlx::query("UPDATE jobs SET size=? WHERE id=?").bind(size as i64).bind(&job.id).execute(&app.db).await?;
    storage::await_job(app,&job.id).await
}
