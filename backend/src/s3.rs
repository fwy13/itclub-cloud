//! Path-style S3 adapter with actual SigV4 verification, including presigned requests.
use crate::{crypto,db::{self,User,Node},error::{Error,Result},files,storage,state::App,webdav::{self,escape,xml}};
use axum::{extract::{State,Request},body::Body,http::{HeaderMap,Method,StatusCode,Uri},response::{Response,IntoResponse}};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD,Engine};
use hmac::{Hmac,Mac};
use sha2::{Sha256,Digest};
use md5::Md5;
use subtle::ConstantTimeEq;
use std::collections::{BTreeMap,HashMap};
use futures_util::StreamExt;
use tokio::io::{AsyncWriteExt,AsyncReadExt};
const NS:&str="http://s3.amazonaws.com/doc/2006-03-01/";
const EMPTY_HASH:&str="e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
type Query=BTreeMap<String,String>;
fn query(uri:&Uri)->Query{url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes()).into_owned().collect()}
fn enc(s:&str,slash:bool)->String{let mut out=String::new();for b in s.bytes(){if b.is_ascii_alphanumeric()||b"-_.~".contains(&b)||(slash&&b==b'/'){out.push(b as char);}else{out.push_str(&format!("%{b:02X}"));}}out}
fn hmac(key:&[u8],value:&str)->Vec<u8>{let mut mac=<Hmac<Sha256> as Mac>::new_from_slice(key).expect("HMAC accepts arbitrary keys");mac.update(value.as_bytes());mac.finalize().into_bytes().to_vec()}
async fn authenticate(app:&App,method:&Method,uri:&Uri,headers:&HeaderMap)->Result<(User,String)> {
    let q=query(uri);let presigned=q.contains_key("X-Amz-Signature");
    let (credential,signed_headers,signature,date)=if presigned {
        if q.get("X-Amz-Algorithm").map(String::as_str)!=Some("AWS4-HMAC-SHA256"){return Err(Error::forbidden());}
        (q.get("X-Amz-Credential").cloned().unwrap_or_default(),q.get("X-Amz-SignedHeaders").cloned().unwrap_or_default(),q.get("X-Amz-Signature").cloned().unwrap_or_default(),q.get("X-Amz-Date").cloned().unwrap_or_default())
    }else{
        let auth=headers.get("authorization").and_then(|h|h.to_str().ok()).and_then(|s|s.strip_prefix("AWS4-HMAC-SHA256 ")).ok_or_else(Error::unauthorized)?;
        let fields:HashMap<_,_>=auth.split(',').filter_map(|s|s.trim().split_once('=')).collect();
        (fields.get("Credential").unwrap_or(&"").to_string(),fields.get("SignedHeaders").unwrap_or(&"").to_string(),fields.get("Signature").unwrap_or(&"").to_string(),headers.get("x-amz-date").and_then(|h|h.to_str().ok()).unwrap_or("").to_owned())
    };
    let parts:Vec<_>=credential.split('/').collect();if parts.len()!=5||parts[3]!="s3"||parts[4]!="aws4_request"||parts[2].is_empty(){return Err(Error::forbidden());}
    if date.len()!=16||!date.is_ascii()||&date[..8]!=parts[1]{return Err(Error::forbidden());}
    let time=chrono::NaiveDateTime::parse_from_str(&date,"%Y%m%dT%H%M%SZ").map_err(|_|Error::forbidden())?.and_utc().timestamp();
    let expires=if presigned{q.get("X-Amz-Expires").and_then(|s|s.parse::<i64>().ok()).filter(|n|*n>=1&&*n<=604800).ok_or_else(Error::forbidden)?}else{900};
    if time>db::now()+900||time+expires<db::now(){return Err(Error(StatusCode::FORBIDDEN,"RequestTimeTooSkewed".into()));}
    let u:User=sqlx::query_as("SELECT * FROM users WHERE s3_access=? AND disabled=0").bind(parts[0]).fetch_optional(&app.db).await?.ok_or_else(Error::forbidden)?;
    let encrypted:String=sqlx::query_scalar("SELECT s3_secret FROM users WHERE id=?").bind(&u.id).fetch_one(&app.db).await?;let secret=crypto::open(&app.cfg.master_key,&encrypted)?;
    let names:Vec<_>=signed_headers.split(';').collect();let mut ordered=names.clone();ordered.sort_unstable();ordered.dedup();
    if names!=ordered||!names.contains(&"host")||names.iter().any(|h|h.is_empty()||*h!=h.to_ascii_lowercase()){return Err(Error::forbidden());}
    let mut canonical_headers=String::new();for name in &names {
        let values=headers.get_all(*name).iter().map(|v|v.to_str().map(|s|s.split_whitespace().collect::<Vec<_>>().join(" ")).map_err(|_|Error::forbidden())).collect::<Result<Vec<_>>>()?;
        if values.is_empty(){return Err(Error::forbidden());}canonical_headers.push_str(&format!("{name}:{}\n",values.join(",")));
    }
    let payload=headers.get("x-amz-content-sha256").and_then(|h|h.to_str().ok()).map(str::to_owned).or_else(||q.get("X-Amz-Content-Sha256").cloned()).unwrap_or_else(||if presigned{"UNSIGNED-PAYLOAD".into()}else{EMPTY_HASH.into()});
    if payload.starts_with("STREAMING-"){return Err(Error(StatusCode::NOT_IMPLEMENTED,"Streaming SigV4 chunks không hỗ trợ; dùng UNSIGNED-PAYLOAD qua HTTPS hoặc SHA256 đầy đủ".into()));}
    if payload!="UNSIGNED-PAYLOAD"&&(payload.len()!=64||!payload.bytes().all(|b|b.is_ascii_hexdigit())){return Err(Error::forbidden());}
    let mut pairs:Vec<_>=url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes()).filter(|(k,_)|!presigned||k!="X-Amz-Signature").map(|(k,v)|(enc(&k,false),enc(&v,false))).collect();pairs.sort();
    let canonical_query=pairs.into_iter().map(|(k,v)|format!("{k}={v}")).collect::<Vec<_>>().join("&");
    let path=percent_encoding::percent_decode_str(uri.path()).decode_utf8().map_err(|_|Error::bad("Invalid path"))?;
    let canonical=format!("{}\n{}\n{}\n{}\n{}\n{}",method.as_str(),enc(&path,true),canonical_query,canonical_headers,signed_headers,payload);
    let scope=parts[1..].join("/");let to_sign=format!("AWS4-HMAC-SHA256\n{date}\n{scope}\n{}",crypto::sha(canonical));
    let date_key=hmac(format!("AWS4{secret}").as_bytes(),parts[1]);let region=hmac(&date_key,parts[2]);let service=hmac(&region,"s3");let signing=hmac(&service,"aws4_request");
    let actual=hmac(&signing,&to_sign);let received=hex::decode(signature).map_err(|_|Error::forbidden())?;if !bool::from(actual.ct_eq(&received)){return Err(Error(StatusCode::FORBIDDEN,"SignatureDoesNotMatch".into()));}
    Ok((u,payload))
}
pub async fn handle(State(app): State<App>, req: Request) -> Response {
    let id = db::id();
    let result = async {
        let (u, payload) = authenticate(&app, req.method(), req.uri(), req.headers()).await?;
        dispatch(app, u, req, &payload).await
    }
    .await;

    let mut response = match result {
        Ok(r) => r,
        Err(e) => {
            let code = match e.0 {
                StatusCode::NOT_FOUND => "NoSuchKey",
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => "AccessDenied",
                StatusCode::NOT_IMPLEMENTED => "NotImplemented",
                StatusCode::CONFLICT => "OperationAborted",
                StatusCode::PRECONDITION_FAILED => "PreconditionFailed",
                _ => "InvalidRequest",
            };
            xml(
                e.0,
                format!(
                    "<?xml version=\"1.0\"?><Error><Code>{code}</Code><Message>{}</Message><RequestId>{id}</RequestId></Error>",
                    escape(&e.1)
                ),
            )
        }
    };
    response.headers_mut().insert("x-amz-request-id", id.parse().unwrap());
    response
}
async fn dispatch(app:App,u:User,req:Request,payload:&str)->Result<Response> {
    let (request,body)=req.into_parts();let q=query(&request.uri);let method=request.method;let headers=request.headers;
    let path=percent_encoding::percent_decode_str(request.uri.path().strip_prefix("/s3").unwrap_or("")).decode_utf8().map_err(|_|Error::bad("Invalid path"))?.trim_start_matches('/').to_string();
    let (bucket,key)=path.split_once('/').unwrap_or((&path,""));
    if bucket.is_empty()&&method==Method::GET{return Ok(xml(StatusCode::OK,format!("<?xml version=\"1.0\"?><ListAllMyBucketsResult xmlns=\"{NS}\"><Owner><ID>{}</ID><DisplayName>{}</DisplayName></Owner><Buckets><Bucket><Name>itclub-cloud</Name><CreationDate>2026-01-01T00:00:00Z</CreationDate></Bucket></Buckets></ListAllMyBucketsResult>",escape(&u.id),escape(&u.username))));}
    if bucket!="itclub-cloud"&&bucket!="telecloud"{return Err(Error(StatusCode::NOT_FOUND,"NoSuchBucket".into()));}
    if q.contains_key("versionId")||q.contains_key("versions"){return Err(Error(StatusCode::NOT_IMPLEMENTED,"Versioning không hỗ trợ".into()));}
    if ["acl","lifecycle","policy","cors","website","tagging","retention","legal-hold","replication","accelerate","logging","notification","object-lock"].iter().any(|name|q.contains_key(*name)){return Err(Error(StatusCode::NOT_IMPLEMENTED,"S3 subresource chưa hỗ trợ".into()));}
    for segment in key.split('/').filter(|s|!s.is_empty()){db::valid_name(segment)?;}
    if let Some(upload)=q.get("uploadId"){return multipart_operation(&app,&u,key,upload,&q,method,&headers,body,payload).await;}
    if key.is_empty(){
        return match method{
            Method::HEAD=>Ok(StatusCode::OK.into_response()),
            Method::GET if q.contains_key("location")=>Ok(xml(StatusCode::OK,format!("<LocationConstraint xmlns=\"{NS}\">us-east-1</LocationConstraint>"))),
            Method::GET if q.contains_key("versioning")=>Ok(xml(StatusCode::OK,format!("<VersioningConfiguration xmlns=\"{NS}\"/>"))),
            Method::GET if q.contains_key("uploads")=>list_multipart(&app,&u).await,
            Method::GET=>list_objects(&app,&u,&q).await,
            Method::PUT=>Ok(StatusCode::OK.into_response()),
            Method::POST if q.contains_key("delete")=>{
                let raw=read_xml(body,payload).await?;let doc=roxmltree::Document::parse(&raw).map_err(|e|Error::bad(e.to_string()))?;
                let keys:Vec<String>=doc.descendants().filter(|n|n.has_tag_name("Key")).filter_map(|n|n.text().map(str::to_owned)).collect();if keys.len()>1000{return Err(Error::bad("Tối đa 1000 object"));}
                let mut out=format!("<DeleteResult xmlns=\"{NS}\">");
                for key in keys{if let Ok(Some(n))=db::resolve_path(&app.db,&u.id,&key).await{files::trash_node(&app,&u,&n.id).await?;}out.push_str(&format!("<Deleted><Key>{}</Key></Deleted>",escape(&key)));}out.push_str("</DeleteResult>");Ok(xml(StatusCode::OK,out))
            },
            _=>Err(Error(StatusCode::NOT_IMPLEMENTED,"Bucket operation không hỗ trợ".into())),
        };
    }
    if method==Method::POST&&q.contains_key("uploads") {
        let id=db::id();let mime=headers.get("content-type").and_then(|v|v.to_str().ok()).unwrap_or("application/octet-stream");
        sqlx::query("INSERT INTO multipart(id,owner,object_key,mime,created_at) VALUES(?,?,?,?,?)").bind(&id).bind(&u.id).bind(key).bind(mime).bind(db::now()).execute(&app.db).await?;
        return Ok(xml(StatusCode::OK,format!("<InitiateMultipartUploadResult xmlns=\"{NS}\"><Bucket>itclub-cloud</Bucket><Key>{}</Key><UploadId>{id}</UploadId></InitiateMultipartUploadResult>",escape(key))));
    }
    match method {
        Method::GET|Method::HEAD=>{let n=db::resolve_path(&app.db,&u.id,key).await?.ok_or_else(Error::not_found)?;
            if n.kind=="folder"{return Ok((StatusCode::OK,[("content-length","0")]).into_response());}storage::serve(app,n,headers,method==Method::HEAD,false).await},
        Method::PUT=>{
            if key.ends_with('/') {db::ensure_path(&app.db,&u.id,key).await?;return Ok(StatusCode::OK.into_response());}
            let (dir,name)=webdav::split(key)?;let parent=db::ensure_path(&app.db,&u.id,&dir).await?;let old=db::resolve_path(&app.db,&u.id,key).await.ok().flatten();webdav::preconditions(&headers,old.as_ref())?;
            if let Some(copy_source)=headers.get("x-amz-copy-source").and_then(|s|s.to_str().ok()){
                let source=percent_encoding::percent_decode_str(copy_source).decode_utf8().map_err(|_|Error::bad("Copy source không hợp lệ"))?;
                let source=source.strip_prefix("/itclub-cloud/").or_else(||source.strip_prefix("itclub-cloud/")).or_else(||source.strip_prefix("/telecloud/")).or_else(||source.strip_prefix("telecloud/")).ok_or_else(Error::forbidden)?;
                let src=db::resolve_path(&app.db,&u.id,source).await?.ok_or_else(Error::not_found)?;
                if let Some(n)=&old{if n.id==src.id{return Ok(xml(StatusCode::OK,format!("<CopyObjectResult><ETag>&quot;{}&quot;</ETag><LastModified>{}</LastModified></CopyObjectResult>",src.etag,timestamp(src.updated_at))));}files::trash_node(&app,&u,&n.id).await?;}
                let id=files::copy_node(&app,&u,&src.id,parent,Some(name)).await?;let n=db::live_node(&app.db,&u.id,&id).await?;
                return Ok(xml(StatusCode::OK,format!("<CopyObjectResult><ETag>&quot;{}&quot;</ETag><LastModified>{}</LastModified></CopyObjectResult>",n.etag,timestamp(n.updated_at))));
            }
            let mime=headers.get("content-type").and_then(|v|v.to_str().ok()).unwrap_or("application/octet-stream");
            let n=webdav::put_body(&app,&u,&name,parent,mime,old.map(|n|n.id),body,Some(payload)).await?;
            Ok((StatusCode::OK,[("etag",format!("\"{}\"",n.etag))]).into_response())
        },
        Method::DELETE=>{if let Ok(Some(n))=db::resolve_path(&app.db,&u.id,key).await{files::trash_node(&app,&u,&n.id).await?;}Ok(StatusCode::NO_CONTENT.into_response())},
        _=>Err(Error(StatusCode::NOT_IMPLEMENTED,"S3 operation không hỗ trợ".into())),
    }
}
fn timestamp(t:i64)->String{chrono::DateTime::from_timestamp(t,0).unwrap_or_default().to_rfc3339_opts(chrono::SecondsFormat::Secs,true)}
async fn list_objects(app:&App,u:&User,q:&Query)->Result<Response>{
    let prefix=q.get("prefix").cloned().unwrap_or_default();let delimiter=q.get("delimiter").cloned().unwrap_or_default();
    let marker=q.get("continuation-token").and_then(|s|URL_SAFE_NO_PAD.decode(s).ok()).and_then(|b|String::from_utf8(b).ok()).or_else(||q.get("start-after").cloned()).or_else(||q.get("marker").cloned()).unwrap_or_default();
    let max=q.get("max-keys").and_then(|s|s.parse::<usize>().ok()).unwrap_or(1000).min(1000);
    let nodes:Vec<Node>=sqlx::query_as("SELECT * FROM nodes WHERE owner=? AND deleted_at IS NULL").bind(&u.id).fetch_all(&app.db).await?;
    let map:HashMap<&str,&Node>=nodes.iter().map(|n|(n.id.as_str(),n)).collect();let mut results:BTreeMap<String,Option<&Node>>=BTreeMap::new();
    for node in &nodes{let mut pieces=vec![node.name.as_str()];let mut parent=node.parent_id.as_deref();for _ in 0..128{let Some(id)=parent else{break;};let Some(n)=map.get(id)else{break;};pieces.push(&n.name);parent=n.parent_id.as_deref();}pieces.reverse();let mut key=pieces.join("/");if node.kind=="folder"{key.push('/');}
        if !key.starts_with(&prefix){continue;}
        if !delimiter.is_empty(){if let Some(pos)=key[prefix.len()..].find(delimiter.as_str()){let common=key[..prefix.len()+pos+delimiter.len()].to_owned();if common>marker{results.insert(common,None);}continue;}}
        if key>marker{results.insert(key,Some(node));}
    }
    let truncated=results.len()>max;let selected:Vec<_>=results.into_iter().take(max).collect();let next=selected.last().map(|(k,_)|k.clone()).unwrap_or_default();
    let encode=q.get("encoding-type").map(String::as_str)==Some("url");let key_xml=|s:&str|escape(&if encode{enc(s,false)}else{s.to_owned()});
    let mut out=format!("<ListBucketResult xmlns=\"{NS}\"><Name>itclub-cloud</Name><Prefix>{}</Prefix><MaxKeys>{max}</MaxKeys><KeyCount>{}</KeyCount><IsTruncated>{truncated}</IsTruncated>",key_xml(&prefix),selected.len());
    if encode{out.push_str("<EncodingType>url</EncodingType>");}if !delimiter.is_empty(){out.push_str(&format!("<Delimiter>{}</Delimiter>",key_xml(&delimiter)));}
    for (key,node) in selected {if let Some(n)=node {out.push_str(&format!("<Contents><Key>{}</Key><LastModified>{}</LastModified><ETag>&quot;{}&quot;</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass></Contents>",key_xml(&key),timestamp(n.updated_at),escape(&n.etag),n.size));}else{out.push_str(&format!("<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>",key_xml(&key)));}}
    if truncated{if q.get("list-type").map(String::as_str)==Some("2"){out.push_str(&format!("<NextContinuationToken>{}</NextContinuationToken>",URL_SAFE_NO_PAD.encode(next)));}else{out.push_str(&format!("<NextMarker>{}</NextMarker>",key_xml(&next)));}}out.push_str("</ListBucketResult>");Ok(xml(StatusCode::OK,out))
}
async fn read_xml(body:Body,payload:&str)->Result<String>{let b=axum::body::to_bytes(body,2*1024*1024).await.map_err(|_|Error::bad("XML quá lớn"))?;if payload!="UNSIGNED-PAYLOAD"&&crypto::sha(&b)!=payload{return Err(Error::bad("Payload hash mismatch"));}String::from_utf8(b.to_vec()).map_err(|_|Error::bad("XML không hợp lệ"))}
async fn list_multipart(app:&App,u:&User)->Result<Response>{
    let rows:Vec<(String,String,i64)>=sqlx::query_as("SELECT id,object_key,created_at FROM multipart WHERE owner=?").bind(&u.id).fetch_all(&app.db).await?;
    let mut out=format!("<ListMultipartUploadsResult xmlns=\"{NS}\"><Bucket>itclub-cloud</Bucket><IsTruncated>false</IsTruncated>");for(id,key,at)in rows{out.push_str(&format!("<Upload><Key>{}</Key><UploadId>{id}</UploadId><Initiated>{}</Initiated></Upload>",escape(&key),timestamp(at)));}out.push_str("</ListMultipartUploadsResult>");Ok(xml(StatusCode::OK,out))
}
async fn multipart_operation(app:&App,u:&User,key:&str,id:&str,q:&Query,method:Method,headers:&HeaderMap,body:Body,payload:&str)->Result<Response>{
    let row:Option<(String,String)>=sqlx::query_as("SELECT object_key,mime FROM multipart WHERE id=? AND owner=?").bind(id).bind(&u.id).fetch_optional(&app.db).await?;
    let (stored_key,mime)=row.ok_or_else(Error::not_found)?;if stored_key!=key{return Err(Error::not_found());}
    let lock=app.lock(format!("multipart:{id}"));let _guard=lock.lock().await;
    match method {
        Method::PUT=>{
            let number=q.get("partNumber").and_then(|s|s.parse::<i64>().ok()).filter(|n|(1..=10000).contains(n)).ok_or_else(||Error::bad("Invalid partNumber"))?;
            if headers.contains_key("x-amz-copy-source"){return Err(Error(StatusCode::NOT_IMPLEMENTED,"UploadPartCopy chưa hỗ trợ".into()));}
            let path=app.cfg.data.join("temp").join(format!("mp-{id}-{number}.part"));let temp=path.with_extension("writing");let mut out=tokio::fs::File::create(&temp).await?;
            let mut stream=body.into_data_stream();let mut size=0u64;let mut sha=Sha256::new();let mut md5=Md5::new();
            while let Some(chunk)=stream.next().await{let b=chunk.map_err(|e|Error::bad(e.to_string()))?;size+=b.len() as u64;if size>app.cfg.max_upload.min(5*1024*1024*1024){return Err(Error::bad("Part quá lớn"));}sha.update(&b);md5.update(&b);out.write_all(&b).await?;}
            out.sync_all().await?;drop(out);if payload!="UNSIGNED-PAYLOAD"&&hex::encode(sha.finalize())!=payload{let _=tokio::fs::remove_file(&temp).await;return Err(Error::bad("Payload hash mismatch"));}
            let etag=hex::encode(md5.finalize());tokio::fs::rename(temp,path).await?;
            sqlx::query("INSERT INTO multipart_parts(upload_id,part_number,size,etag) VALUES(?,?,?,?) ON CONFLICT(upload_id,part_number) DO UPDATE SET size=excluded.size,etag=excluded.etag").bind(id).bind(number).bind(size as i64).bind(&etag).execute(&app.db).await?;
            Ok((StatusCode::OK,[("etag",format!("\"{etag}\""))]).into_response())
        },
        Method::GET=>{
            let rows:Vec<(i64,i64,String)>=sqlx::query_as("SELECT part_number,size,etag FROM multipart_parts WHERE upload_id=? ORDER BY part_number").bind(id).fetch_all(&app.db).await?;
            let mut out=format!("<ListPartsResult xmlns=\"{NS}\"><Bucket>itclub-cloud</Bucket><Key>{}</Key><UploadId>{id}</UploadId><IsTruncated>false</IsTruncated>",escape(key));for(n,size,etag)in rows{out.push_str(&format!("<Part><PartNumber>{n}</PartNumber><ETag>&quot;{etag}&quot;</ETag><Size>{size}</Size></Part>"));}out.push_str("</ListPartsResult>");Ok(xml(StatusCode::OK,out))
        },
        Method::DELETE=>{remove_multipart(app,id).await?;Ok(StatusCode::NO_CONTENT.into_response())},
        Method::POST=>{
            let text=read_xml(body,payload).await?;let doc=roxmltree::Document::parse(&text).map_err(|e|Error::bad(e.to_string()))?;
            let mut wanted=vec![];for p in doc.descendants().filter(|n|n.has_tag_name("Part")){let number=p.children().find(|n|n.has_tag_name("PartNumber")).and_then(|n|n.text()).and_then(|s|s.parse::<i64>().ok()).ok_or_else(||Error::bad("Invalid PartNumber"))?;let etag=p.children().find(|n|n.has_tag_name("ETag")).and_then(|n|n.text()).unwrap_or("").trim_matches('"').to_owned();wanted.push((number,etag));}
            if wanted.is_empty()||wanted.len()>10000||wanted.windows(2).any(|w|w[0].0>=w[1].0){return Err(Error::bad("Danh sách part phải tăng dần và không trùng"));}
            let mut total=0;let mut md5=Md5::new();for(i,(number,etag))in wanted.iter().enumerate(){
                let saved:Option<(i64,String)>=sqlx::query_as("SELECT size,etag FROM multipart_parts WHERE upload_id=? AND part_number=?").bind(id).bind(number).fetch_optional(&app.db).await?;let (size,expected)=saved.ok_or_else(||Error::bad("InvalidPart"))?;
                if &expected!=etag{return Err(Error::bad("InvalidPart ETag"));}if i+1<wanted.len()&&size<5*1024*1024{return Err(Error::bad("EntityTooSmall: part ngoài part cuối cần ít nhất 5 MiB"));}total+=size;md5.update(hex::decode(etag).map_err(|_|Error::bad("Invalid ETag"))?);
            }
            if total as u64>app.cfg.max_upload{return Err(Error::bad("File quá lớn"));}
            let (dir,name)=webdav::split(key)?;let parent=db::ensure_path(&app.db,&u.id,&dir).await?;let old=db::resolve_path(&app.db,&u.id,key).await.ok().flatten();webdav::preconditions(headers,old.as_ref())?;
            let job=storage::create_job(app,u,&name,parent,total,&mime,"staging",old.map(|n|n.id)).await?;let mut out=tokio::fs::File::create(app.cfg.upload_path(&job.id)).await?;
            for (number,_)in &wanted{let mut f=tokio::fs::File::open(app.cfg.data.join("temp").join(format!("mp-{id}-{number}.part"))).await?;tokio::io::copy(&mut f,&mut out).await?;}out.sync_all().await?;drop(out);
            let n=storage::await_job(app,&job.id).await?;let etag=format!("{}-{}",hex::encode(md5.finalize()),wanted.len());sqlx::query("UPDATE nodes SET etag=? WHERE id=?").bind(&etag).bind(&n.id).execute(&app.db).await?;remove_multipart(app,id).await?;
            Ok(xml(StatusCode::OK,format!("<CompleteMultipartUploadResult xmlns=\"{NS}\"><Location>{}/s3/itclub-cloud/{}</Location><Bucket>itclub-cloud</Bucket><Key>{}</Key><ETag>&quot;{etag}&quot;</ETag></CompleteMultipartUploadResult>",escape(&app.cfg.origin),escape(&webdav::encode_path(key)),escape(key))))
        },_=>Err(Error(StatusCode::METHOD_NOT_ALLOWED,"Multipart method không hợp lệ".into())),
    }
}
pub async fn remove_multipart(app:&App,id:&str)->Result<()> {
    let nums:Vec<i64>=sqlx::query_scalar("SELECT part_number FROM multipart_parts WHERE upload_id=?").bind(id).fetch_all(&app.db).await?;
    for number in nums{let _=tokio::fs::remove_file(app.cfg.data.join("temp").join(format!("mp-{id}-{number}.part"))).await;}
    sqlx::query("DELETE FROM multipart WHERE id=?").bind(id).execute(&app.db).await?;Ok(())
}
