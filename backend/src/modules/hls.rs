//! Bounded VOD downloader used by the HLS and HTML modules. No whole-video RAM buffer.
//! Remote URLs are fetched through the same DNS-pinned HTTP client as direct uploads.
use super::*;
use futures_util::{StreamExt,stream};
use tokio::io::AsyncWriteExt;
use std::{sync::{Arc,atomic::{AtomicU64,Ordering}},process::Stdio};

async fn playlist(ctx:&ModuleContext,url:&str,headers:&Headers)->Result<(url::Url,String)> {
    let r=ctx.get(url,headers).await?;let base=r.url().clone();let mut s=r.bytes_stream();let mut bytes=Vec::new();
    while let Some(b)=s.next().await{ctx.check_cancelled()?;let b=b?;if bytes.len()+b.len()>4*1024*1024{anyhow::bail!("Playlist quá lớn");}bytes.extend_from_slice(&b);}
    let text=String::from_utf8(bytes)?;if !text.trim_start().starts_with("#EXTM3U"){anyhow::bail!("Không phải playlist HLS");}Ok((base,text))
}
fn attribute<'a>(line:&'a str,key:&str)->Option<&'a str> {
    let needle=format!("{key}=");let (_,s)=line.split_once(needle.as_str())?;
    if let Some(s)=s.strip_prefix('"'){s.split_once('"').map(|(v,_)|v)}else{Some(s.split(',').next().unwrap_or(s))}
}
pub async fn download(ctx:&ModuleContext,url:&str,headers:&Headers)->Result<PathBuf> {
    let mut current=url.to_owned();let mut resolved=None;
    for _ in 0..5 {
        let (base,text)=playlist(ctx,&current,headers).await?;
        if !text.contains("#EXT-X-STREAM-INF:"){resolved=Some((base,text));break;}
        let lines:Vec<_>=text.lines().collect();let mut variants=vec![];
        for (i,line) in lines.iter().enumerate(){if line.starts_with("#EXT-X-STREAM-INF:"){
            if attribute(line,"AUDIO").is_some(){anyhow::bail!("Playlist dùng audio riêng: chọn module yt-dlp hoặc viết module hỗ trợ audio rendition");}
            let bandwidth=attribute(line,"BANDWIDTH").and_then(|s|s.parse::<u64>().ok()).unwrap_or(0);
            if let Some(next)=lines.get(i+1).filter(|s|!s.starts_with('#')){variants.push((bandwidth,base.join(next.trim())?.to_string()));}
        }}
        variants.sort_by_key(|(b,_)|*b);current=variants.pop().context("Không có variant HLS")?.1;
    }
    let (base,text)=resolved.context("Playlist lồng quá sâu")?;
    if !text.contains("#EXT-X-ENDLIST"){anyhow::bail!("Module này nhận HLS VOD, chưa nhận livestream");}
    if text.lines().any(|l|l.starts_with("#EXT-X-KEY:")&&attribute(l,"METHOD")!=Some("NONE")){anyhow::bail!("HLS có mã hóa: hãy thêm logic giải mã trong module của website");}
    if text.contains("#EXT-X-BYTERANGE"){anyhow::bail!("HLS byte-range chưa hỗ trợ trong module mẫu");}
    let dir=ctx.work_dir().await?;let mut rewritten=String::new();let mut downloads=vec![];
    for line in text.lines(){let line=line.trim();
        if line.starts_with("#EXT-X-MAP:") {
            if attribute(line,"BYTERANGE").is_some(){anyhow::bail!("HLS init segment byte-range chưa hỗ trợ");}
            let uri=attribute(line,"URI").context("EXT-X-MAP thiếu URI")?;let name=format!("init-{}.mp4",downloads.len());downloads.push((base.join(uri)?.to_string(),dir.join(&name)));
            rewritten.push_str(&format!("#EXT-X-MAP:URI=\"{name}\"\n"));
        }else if !line.is_empty()&&!line.starts_with('#') {
            let name=format!("segment-{:06}.ts",downloads.len());downloads.push((base.join(line)?.to_string(),dir.join(&name)));rewritten.push_str(&name);rewritten.push('\n');
        }else{rewritten.push_str(line);rewritten.push('\n');}
    }
    if downloads.is_empty()||downloads.len()>20000{anyhow::bail!("Số segment không hợp lệ");}
    let total=downloads.len() as u64;let transferred=Arc::new(AtomicU64::new(0));let count=Arc::new(AtomicU64::new(0));
    let tasks=stream::iter(downloads.into_iter().map(|(url,path)|{
        let transferred=transferred.clone();let count=count.clone();
        async move {
            let r=ctx.get(&url,headers).await?;let mut stream=r.bytes_stream();let mut out=tokio::fs::File::create(path).await?;let mut size=0u64;
            while let Some(b)=stream.next().await {ctx.check_cancelled()?;let b=b?;size+=b.len() as u64;
                if size>64*1024*1024{anyhow::bail!("Segment quá lớn");}
                let bytes=transferred.fetch_add(b.len() as u64,Ordering::Relaxed)+b.len() as u64;if bytes>ctx.app.cfg.max_upload{anyhow::bail!("Video vượt giới hạn dung lượng");}crate::disk::write(&ctx.app,&mut out,&b,true).await?;
            }
            out.flush().await?;let n=count.fetch_add(1,Ordering::Relaxed)+1;ctx.progress("segments",n,total).await;Ok::<(),anyhow::Error>(())
        }
    })).buffer_unordered(4);
    tokio::pin!(tasks);while let Some(result)=tasks.next().await{result?;}
    let manifest=dir.join("local.m3u8");tokio::fs::write(&manifest,rewritten).await?;
    ctx.progress("muxing",0,0).await;let output=dir.join("output.mp4");
    // FFmpeg only sees local files. All external HTTP has already passed DNS checks.
    let mut command=tokio::process::Command::new(&ctx.app.cfg.ffmpeg);
    command.args(["-nostdin","-y","-hide_banner","-loglevel","error","-protocol_whitelist","file,crypto,data","-allowed_extensions","ALL","-i"]).arg(manifest)
        .args(["-c","copy","-movflags","+faststart"]).arg(&output).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    let child=command.spawn().context("Không chạy được FFmpeg")?;
    let watchdog=async {loop {crate::disk::check(&ctx.app).await?;tokio::time::sleep(std::time::Duration::from_millis(500)).await;} #[allow(unreachable_code)] Ok::<(),crate::error::Error>(())};
    let result=tokio::select!{_=ctx.cancel.cancelled()=>anyhow::bail!("Đã hủy mux"),limit=watchdog=>{limit?;anyhow::bail!("Dừng mux do giới hạn SSD");},r=child.wait_with_output()=>r?};
    if !result.status.success(){anyhow::bail!("FFmpeg: {}",String::from_utf8_lossy(&result.stderr));}
    Ok(output)
}
