use crate::{db::{self,Node,User},error::{Error,Result},state::App,storage};
use axum::{extract::{State,Path,Query},Extension,Json,body::Body,response::{Response,IntoResponse}};
use serde::Deserialize;
use serde_json::{json,Value};
use std::{path::PathBuf,io::{Read,Write}};
use tokio::io::AsyncReadExt;

pub struct RemoveOnDrop(pub PathBuf);
impl Drop for RemoveOnDrop {fn drop(&mut self){let _=std::fs::remove_file(&self.0);}}
pub async fn download_temp(path:PathBuf,name:&str)->Result<Response> {
    let mut file=tokio::fs::File::open(&path).await?;let size=file.metadata().await?.len();let cleanup=RemoveOnDrop(path);
    let stream=async_stream::try_stream!{let _cleanup=cleanup;let mut buf=vec![0u8;64*1024];loop{let n=file.read(&mut buf).await?;if n==0{break;}yield bytes::Bytes::copy_from_slice(&buf[..n]);}};
    let stream:std::pin::Pin<Box<dyn futures_util::Stream<Item=std::result::Result<bytes::Bytes,std::io::Error>>+Send>>=Box::pin(stream);
    let name=percent_encoding::utf8_percent_encode(name,percent_encoding::NON_ALPHANUMERIC);
    Ok(([("content-type","application/zip".to_owned()),("content-length",size.to_string()),("content-disposition",format!("attachment; filename*=UTF-8''{name}"))],Body::from_stream(stream)).into_response())
}
pub async fn folder_zip(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>)->Result<Response> {
    let root=db::live_node(&app.db,&u.id,&id).await?;if root.kind!="folder"{return Err(Error::bad("Đây không phải thư mục"));}
    let nodes=db::descendants(&app.db,&u.id,&id).await?;let total:i64=nodes.iter().filter(|n|n.deleted_at.is_none()).map(|n|n.size).sum();
    if total as u64>app.cfg.max_upload{return Err(Error::bad("Thư mục vượt giới hạn ZIP; tải từng file hoặc dùng WebDAV"));}
    let output=app.cfg.data.join("temp").join(format!("folder-{}.zip",db::id()));let guard=RemoveOnDrop(output.clone());
    let mut writer=zip::ZipWriter::new(std::fs::File::create(&output)?);let prefix=db::full_path(&app.db,&root).await?;
    for n in nodes.into_iter().filter(|n|n.deleted_at.is_none()) {
        let full=db::full_path(&app.db,&n).await?;let name=full.strip_prefix(prefix.as_str()).unwrap_or(&full).trim_start_matches('/').to_owned();if name.is_empty(){continue;}
        if n.kind=="folder"{writer.add_directory(format!("{name}/"),zip::write::SimpleFileOptions::default()).map_err(anyhow::Error::from)?;continue;}
        let temp=app.cfg.data.join("temp").join(format!("zip-entry-{}",db::id()));let temp_guard=RemoveOnDrop(temp.clone());storage::materialize(&app,&n,&temp).await?;
        writer=tokio::task::spawn_blocking(move||->anyhow::Result<_>{writer.start_file(name,zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored))?;let mut file=std::fs::File::open(temp)?;std::io::copy(&mut file,&mut writer)?;Ok(writer)}).await.map_err(anyhow::Error::from)??;drop(temp_guard);
    }
    tokio::task::spawn_blocking(move||writer.finish()).await.map_err(anyhow::Error::from)?.map_err(anyhow::Error::from)?;
    // The response owns cleanup; don't remove before ReaderStream opens the file.
    let res=download_temp(output,&format!("{}.zip",root.name)).await?;std::mem::forget(guard);Ok(res)
}
async fn cache_archive(app:&App,node:&Node)->Result<PathBuf> {
    if node.size as u64>2*1024*1024*1024{return Err(Error::bad("Trình đọc archive giới hạn 2 GiB"));}
    let dest=app.cfg.data.join("archives").join(format!("{}-{}.zip",node.id,node.etag));
    let lock=app.lock(format!("archive:{}",node.id));let _guard=lock.lock().await;
    if !dest.exists(){let temp=dest.with_extension("partial");let guard=RemoveOnDrop(temp.clone());storage::materialize(app,node,&temp).await?;tokio::fs::rename(temp,&dest).await?;drop(guard);}Ok(dest)
}
pub async fn list(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>)->Result<Json<Value>> {
    let n=db::live_node(&app.db,&u.id,&id).await?;let path=cache_archive(&app,&n).await?;
    let entries=tokio::task::spawn_blocking(move||->anyhow::Result<Vec<Value>>{
        let mut zip=zip::ZipArchive::new(std::fs::File::open(path)?)?;if zip.len()>20000{anyhow::bail!("Quá nhiều entry trong ZIP");}let mut entries=vec![];
        for i in 0..zip.len(){let file=zip.by_index(i)?;if file.is_dir()||file.enclosed_name().is_none(){continue;}let name=file.name().to_owned();if file.size()>32*1024*1024{continue;}let mime=mime_guess::from_path(&name).first_or_octet_stream().to_string();if matches!(mime.as_str(),"image/jpeg"|"image/png"|"image/webp"|"image/gif"|"image/avif"){entries.push(json!({"name":name,"size":file.size(),"mime":mime}));}}
        entries.sort_by(|a,b|a["name"].as_str().cmp(&b["name"].as_str()));Ok(entries)
    }).await.map_err(anyhow::Error::from)??;
    Ok(Json(json!({"entries":entries})))
}
#[derive(Deserialize)]pub struct Resource {path:String}
pub async fn resource(State(app):State<App>,Extension(u):Extension<User>,Path(id):Path<String>,Query(q):Query<Resource>)->Result<Response>{
    let n=db::live_node(&app.db,&u.id,&id).await?;let path=cache_archive(&app,&n).await?;
    let (bytes,mime)=tokio::task::spawn_blocking(move||->anyhow::Result<_>{let mut zip=zip::ZipArchive::new(std::fs::File::open(path)?)?;let mut f=zip.by_name(&q.path)?;if f.enclosed_name().is_none()||f.size()>32*1024*1024{anyhow::bail!("Entry không hợp lệ");}let mime=mime_guess::from_path(f.name()).first_or_octet_stream().to_string();if !matches!(mime.as_str(),"image/jpeg"|"image/png"|"image/webp"|"image/gif"|"image/avif"){anyhow::bail!("Chỉ phục vụ ảnh trong archive");}let mut b=Vec::with_capacity(f.size() as usize);f.read_to_end(&mut b)?;Ok((b,mime))}).await.map_err(anyhow::Error::from)??;
    Ok(([("content-type",mime),("cache-control","private, max-age=300".into())],bytes).into_response())
}
