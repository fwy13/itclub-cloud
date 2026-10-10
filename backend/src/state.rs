use crate::{config::Config,db::Job,telegram::Telegram};
use bytes::Bytes;
use moka::future::Cache;
use serde_json::{json,Value};
use sqlx::SqlitePool;
use std::{collections::HashMap,sync::{Arc,Mutex,Weak}};
use tokio::sync::{broadcast,Mutex as AsyncMutex,Semaphore,Notify};
use tokio_util::sync::CancellationToken;

pub type App=Arc<State>;
pub struct State {
    pub cfg:Config,pub db:SqlitePool,pub tg:Arc<Telegram>,
    pub events:broadcast::Sender<Value>,pub uploads:Arc<Semaphore>,pub downloads:Arc<Semaphore>,pub remote_slots:Arc<Semaphore>,
    pub cancel:AsyncMutex<HashMap<String,CancellationToken>>,
    pub cache:Cache<String,Bytes>,locks:Mutex<HashMap<String,Weak<AsyncMutex<()>>>>,
    pub throttle:AsyncMutex<HashMap<String,(u32,i64)>>,
    pub modules:crate::modules::Registry,
    pub shutdown:Notify,pub stopping:CancellationToken,
}
impl State {
    pub fn new(cfg:Config,db:SqlitePool,tg:Arc<Telegram>)->anyhow::Result<App> {
        let (events,_)=broadcast::channel(512);
        Ok(Arc::new(Self{uploads:Arc::new(Semaphore::new(cfg.max_parallel)),downloads:Arc::new(Semaphore::new(16)),remote_slots:Arc::new(Semaphore::new(cfg.remote_parallel)),
            cache:Cache::builder().max_capacity(cfg.cache_bytes).weigher(|_:&String,v:&Bytes|v.len() as u32).time_to_idle(std::time::Duration::from_secs(300)).build(),
            cfg,db,tg,events,cancel:AsyncMutex::new(HashMap::new()),locks:Mutex::new(HashMap::new()),throttle:AsyncMutex::new(HashMap::new()),modules:crate::modules::Registry::new(),shutdown:Notify::new(),stopping:CancellationToken::new()}))
    }
    pub fn lock(&self,key:impl Into<String>)->Arc<AsyncMutex<()>> {
        let key=key.into();let mut locks=self.locks.lock().unwrap();
        if locks.len()>8192{locks.retain(|_,v|v.strong_count()>0);}
        if let Some(lock)=locks.get(&key).and_then(Weak::upgrade){return lock;}
        let lock=Arc::new(AsyncMutex::new(()));locks.insert(key,Arc::downgrade(&lock));lock
    }
    pub async fn progress(&self,id:&str,status:&str,done:u64,total:u64,error:Option<&str>) {
        let lock=self.lock(format!("progress:{id}"));
        let _guard=lock.lock().await;
        let percent=if status=="done"{100}else if total==0{0}else{(done.saturating_mul(100)/total).min(100)};
        let _=sqlx::query("UPDATE jobs SET status=?,progress=?,bytes_done=?,error=?,updated_at=? WHERE id=? AND (status NOT IN ('done','cancelled','error','interrupted') OR status=? OR (?='cancelled' AND status IN ('error','interrupted')))").bind(status).bind(percent as i64).bind(done as i64).bind(error).bind(crate::db::now()).bind(id).bind(status).bind(status).execute(&self.db).await;
        if let Ok(job)=sqlx::query_as::<_,Job>("SELECT * FROM jobs WHERE id=?").bind(id).fetch_one(&self.db).await {let _=self.events.send(json!({"type":"job","owner":job.owner,"job":job}));}
    }
    pub fn changed(&self,owner:&str){let _=self.events.send(json!({"type":"files_changed","owner":owner}));}
}
