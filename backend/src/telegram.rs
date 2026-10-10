use crate::{config::Config,crypto,db,tdlib::Hub};
use anyhow::{bail,Context,Result};
use base64::{engine::general_purpose::STANDARD,Engine};
use serde_json::{json,Value};
use sqlx::SqlitePool;
use std::{collections::BTreeMap,sync::{Arc,atomic::{AtomicUsize,Ordering}},time::{Duration,Instant}};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Account {pub key:String,pub client:i32,pub state:Arc<RwLock<Value>>,pub me:Arc<RwLock<Value>>,pub cooldown:Arc<RwLock<Instant>>,pub bot:bool}
pub struct Telegram {
    pub hub:Option<Arc<Hub>>,pub load_error:Option<String>,pub accounts:RwLock<BTreeMap<String,Account>>,
    cfg:Config,db:SqlitePool,rotation:AtomicUsize,
}
impl Telegram {
    pub fn new(cfg:Config,db:SqlitePool)->Arc<Self> {
        let (hub,load_error)=match Hub::open(&cfg.tdlib){Ok(h)=>(Some(h),None),Err(e)=>(None,Some(e.to_string()))};
        Arc::new(Self{hub,load_error,accounts:RwLock::new(BTreeMap::new()),cfg,db,rotation:AtomicUsize::new(0)})
    }
    pub async fn credentials(&self)->Result<(i64,String)> {
        let id=db::setting(&self.db,"tg_api_id").await.and_then(|s|s.parse().ok()).unwrap_or(self.cfg.api_id);
        let hash=match db::setting(&self.db,"tg_api_hash").await{Some(v)=>crypto::open(&self.cfg.master_key,&v)?,None=>self.cfg.api_hash.clone()};
        if id<=0||hash.is_empty(){bail!("Cần TELEGRAM_API_ID và TELEGRAM_API_HASH (hoặc nhập trong Cài đặt)");}Ok((id,hash))
    }
    pub async fn boot(self:&Arc<Self>)->Result<()> {
        self.start("main",None).await?;
        let rows:Vec<(String,String)>=sqlx::query_as("SELECT id,token FROM bot_accounts").fetch_all(&self.db).await?;
        for (id,token) in rows {if let Ok(token)=crypto::open(&self.cfg.master_key,&token){if let Err(e)=self.start(&id,Some(token)).await {tracing::warn!(account=%id,error=%e,"Bot init failed");}}}
        Ok(())
    }
    pub async fn start(self:&Arc<Self>,key:&str,bot_token:Option<String>)->Result<Account> {
        let mut accounts=self.accounts.write().await;
        if let Some(a)=accounts.get(key){if a.state.read().await["@type"]!="authorizationStateClosed"{return Ok(a.clone());}}
        accounts.remove(key);
        let (api_id,api_hash)=self.credentials().await?;
        let hub=self.hub.clone().context(self.load_error.clone().unwrap_or_else(||"TDLib unavailable".into()))?;
        let mut events=hub.events.subscribe();
        let client=hub.create();
        let account=Account{key:key.into(),client,state:Arc::new(RwLock::new(json!({"@type":"starting"}))),me:Arc::new(RwLock::new(Value::Null)),cooldown:Arc::new(RwLock::new(Instant::now())),bot:bot_token.is_some()};
        accounts.insert(key.into(),account.clone());drop(accounts);
        let a=account.clone();let cfg=self.cfg.clone();
        let folder=cfg.data.join("tdlib").join(key);tokio::fs::create_dir_all(&folder).await?;
        let files=folder.join("files");tokio::fs::create_dir_all(&files).await?;
        tokio::spawn(async move {
            let mut next=hub.call(client,json!({"@type":"getAuthorizationState"})).await.ok();
            loop {
                let state=if let Some(s)=next.take(){s}else{
                    match events.recv().await {
                        Ok(v) if v["@client_id"].as_i64()==Some(client as i64)&&v["@type"]=="updateAuthorizationState"=>v["authorization_state"].clone(),
                        Ok(_)=>continue,Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>{next=hub.call(client,json!({"@type":"getAuthorizationState"})).await.ok();continue;},Err(_)=>break,
                    }
                };
                let kind=state["@type"].as_str().unwrap_or("").to_string();
                *a.state.write().await=state;
                let request=match kind.as_str() {
                    "authorizationStateWaitTdlibParameters"=>Some(json!({"@type":"setTdlibParameters","use_test_dc":false,"database_directory":folder,"files_directory":files,"database_encryption_key":STANDARD.encode(cfg.master_key),"use_file_database":true,"use_chat_info_database":true,"use_message_database":true,"use_secret_chats":false,"api_id":api_id,"api_hash":api_hash,"system_language_code":"en","device_model":"ITClub Cloud","system_version":std::env::consts::OS,"application_version":env!("CARGO_PKG_VERSION"),"enable_storage_optimizer":true})),
                    "authorizationStateWaitEncryptionKey"=>Some(json!({"@type":"checkDatabaseEncryptionKey","encryption_key":STANDARD.encode(cfg.master_key)})),
                    "authorizationStateWaitPhoneNumber" if bot_token.is_some()=>Some(json!({"@type":"checkAuthenticationBotToken","token":bot_token})),
                    "authorizationStateReady"=>{
                        if let Ok(me)=hub.call(client,json!({"@type":"getMe"})).await {*a.me.write().await=me;}
                        let _=hub.call(client,json!({"@type":"setOption","name":"use_storage_optimizer","value":{"@type":"optionValueBoolean","value":true}})).await;
                        None
                    },
                    "authorizationStateClosed"=>break,
                    _=>None,
                };
                if let Some(req)=request {
                    if let Err(e)=hub.call(client,req).await {a.state.write().await["error"]=json!(e.to_string());}
                }
            }
        });
        Ok(account)
    }
    pub async fn status(&self)->Value {
        let accounts:Vec<_>=self.accounts.read().await.values().cloned().collect();let mut list=vec![];
        for a in accounts {let me=a.me.read().await;list.push(json!({"key":a.key,"bot":a.bot,"state":a.state.read().await.clone(),"me":{"id":me["id"],"first_name":me["first_name"],"last_name":me["last_name"]},"cooldown_seconds":a.cooldown.read().await.saturating_duration_since(Instant::now()).as_secs()}));}
        json!({"available":self.hub.is_some(),"error":self.load_error,"accounts":list})
    }
    pub async fn account(&self,key:&str)->Result<Account>{self.accounts.read().await.get(key).cloned().context("Telegram chưa khởi tạo")}
    pub async fn ready(&self)->bool {match self.account("main").await{Ok(a)=>a.state.read().await["@type"]=="authorizationStateReady",Err(_)=>false}}
    pub async fn call(&self,key:&str,value:Value)->Result<Value>{let a=self.account(key).await?;self.hub.as_ref().context("TDLib unavailable")?.call(a.client,value).await}
    pub async fn choose(&self,chat:i64)->Result<Account> {
        let accounts:Vec<_>=self.accounts.read().await.values().cloned().collect();let mut ready=vec![];
        for a in accounts {if a.state.read().await["@type"]=="authorizationStateReady"&&*a.cooldown.read().await<=Instant::now()&&(!a.bot||chat<0){ready.push(a);}}
        if ready.is_empty(){bail!("Không có Telegram client sẵn sàng; thử lại sau khi hết thời gian chờ");}
        Ok(ready[self.rotation.fetch_add(1,Ordering::Relaxed)%ready.len()].clone())
    }
    pub async fn chat(&self)->Result<i64> {
        let saved=db::setting(&self.db,"storage_chat").await.unwrap_or_else(||"me".into());
        if saved=="me" {
            let a=self.account("main").await?;let uid=a.me.read().await["id"].as_i64().context("Telegram chưa đăng nhập")?;
            let chat=self.call("main",json!({"@type":"createPrivateChat","user_id":uid,"force":false})).await?;
            return chat["id"].as_i64().context("Không tìm thấy Saved Messages");
        }
        if let Ok(id)=saved.parse::<i64>() {self.call("main",json!({"@type":"getChat","chat_id":id})).await?;Ok(id)}
        else {let chat=self.call("main",json!({"@type":"searchPublicChat","username":saved.trim_start_matches('@')})).await?;chat["id"].as_i64().context("Không tìm thấy nhóm")}
    }
    pub async fn send_document<F,Fut>(&self,a:&Account,chat:i64,path:&std::path::Path,caption:&str,cancel:&CancellationToken,mut progress:F)->Result<Value>
    where F:FnMut(u64)->Fut+Send,Fut:std::future::Future<Output=()>+Send {
        let file_size=tokio::fs::metadata(path).await?.len();
        let hub=self.hub.as_ref().context("TDLib unavailable")?;
        // Subscribe before sendMessage; even small uploads can complete before its response.
        let mut updates=hub.events.subscribe();
        // Current TDLib schema: inputMessageDocument.document is inputDocument,
        // whose document field contains the InputFile. Passing InputFile directly
        // leaves the nested file unspecified in newer TDLib versions.
        let result = hub.call(a.client, json!({
            "@type": "sendMessage",
            "chat_id": chat,
            "input_message_content": {
                "@type": "inputMessageDocument",
                "document": {
                    "@type": "inputDocument",
                    "document": {"@type": "inputFileLocal", "path": path},
                    "thumbnail": null,
                    "disable_content_type_detection": true
                },
                "caption": {"@type": "formattedText", "text": caption, "entities": []}
            }
        })).await?;
        let pending=result["id"].as_i64().context("sendMessage returned no ID")?;
        if result.get("sending_state").map(Value::is_null).unwrap_or(true){progress(file_size).await;return Ok(result);}
        let fid=media_file(&result).and_then(|f|f["id"].as_i64()).unwrap_or(0);
        let mut ticker=tokio::time::interval(Duration::from_secs(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_report=Instant::now();
        let mut reported=0u64;
        let deadline=tokio::time::sleep(Duration::from_secs(24*3600));tokio::pin!(deadline);
        loop {tokio::select!{
            _=cancel.cancelled()=>{let _=hub.call(a.client,json!({"@type":"deleteMessages","chat_id":chat,"message_ids":[pending],"revoke":true})).await;bail!("Đã hủy upload");},
            _=&mut deadline=>bail!("Upload quá thời hạn"),
            _=ticker.tick()=>{if fid!=0 {if let Ok(f)=hub.call(a.client,json!({"@type":"getFile","file_id":fid})).await {let done=f["remote"]["uploaded_size"].as_u64().unwrap_or(0).min(file_size);if done>reported{reported=done;progress(done).await;last_report=Instant::now();}}}},
            event=updates.recv()=>{match event {
                Ok(v) if v["@client_id"].as_i64()==Some(a.client as i64)&&v["@type"]=="updateFile"&&v["file"]["id"].as_i64()==Some(fid)=>{
                    let done=v["file"]["remote"]["uploaded_size"].as_u64().unwrap_or(0).min(file_size);
                    if done>reported&&(last_report.elapsed()>=Duration::from_millis(500)||done==file_size){reported=done;progress(done).await;last_report=Instant::now();}
                },
                Ok(v) if v["@client_id"].as_i64()==Some(a.client as i64)&&v["message"]["chat_id"].as_i64()==Some(chat)&&v["old_message_id"].as_i64()==Some(pending)=>{
                    match v["@type"].as_str(){Some("updateMessageSendSucceeded")=>{progress(file_size).await;return Ok(v["message"].clone());},Some("updateMessageSendFailed")=>{
                        let e=v["error"]["message"].as_str().unwrap_or("Telegram không gửi được file");
                        *a.cooldown.write().await=Instant::now()+Duration::from_secs(30);bail!("{e}");},_=>{}}
                },
                Ok(_)=>{},Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>{
                    // A missed success update cannot safely be retried by sending another file.
                    // Reconcile from a stable unique caption in the upload journal instead.
                    let found=hub.call(a.client,json!({"@type":"searchChatMessages","chat_id":chat,"query":caption,"from_message_id":0,"offset":0,"limit":5,"filter":{"@type":"searchMessagesFilterDocument"},"message_thread_id":0,"saved_messages_topic_id":0})).await;
                    if let Ok(v)=found {if let Some(m)=v["messages"].as_array().and_then(|x|x.iter().find(|m| m["sending_state"].is_null() && m["content"]["caption"]["text"].as_str()==Some(caption))){progress(file_size).await;return Ok(m.clone());}}
                },Err(_)=>bail!("Telegram update stream closed"),
            }}
        }}
    }
    pub async fn delete(&self,account:&str,chat:i64,message:i64)->Result<()> {
        let request=json!({"@type":"deleteMessages","chat_id":chat,"message_ids":[message],"revoke":true});
        if self.call(account,request.clone()).await.is_err(){self.call("main",request).await?;}Ok(())
    }
    pub async fn stop(&self) {
        let accounts:Vec<_>=self.accounts.read().await.values().cloned().collect();
        if let Some(hub)=&self.hub {for a in &accounts{let _=hub.call(a.client,json!({"@type":"close"})).await;}
            let _=tokio::time::timeout(Duration::from_secs(15),async{loop {let mut closed=true;for a in &accounts{if a.state.read().await["@type"]!="authorizationStateClosed"{closed=false;}}if closed{break;}tokio::time::sleep(Duration::from_millis(100)).await;}}).await;
        }
    }
}
pub fn media_file(message:&Value)->Option<&Value> {
    let c=&message["content"];
    let f=match c["@type"].as_str()? {"messageDocument"=>&c["document"]["document"],"messageVideo"=>&c["video"]["video"],"messageAudio"=>&c["audio"]["audio"],"messageAnimation"=>&c["animation"]["animation"],"messagePhoto"=>&c["photo"]["sizes"].as_array()?.last()?["photo"],_=>return None};
    if f.is_object(){Some(f)}else{None}
}
