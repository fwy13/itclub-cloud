//! Minimal TDLib JSON bridge. Exactly one thread calls td_receive in this process.
//! All raw pointers remain within the FFI boundary; JSON is copied before the next receive.
use anyhow::{Context,Result};
use libloading::Library;
use serde_json::{json,Value};
use std::{collections::HashMap,ffi::{c_char,CStr,CString},sync::{Arc,Mutex,atomic::{AtomicU64,Ordering}},time::Duration};
use tokio::sync::{broadcast,oneshot};

type Create=unsafe extern "C" fn()->i32;
type Send=unsafe extern "C" fn(i32,*const c_char);
type Receive=unsafe extern "C" fn(f64)->*const c_char;
type Execute=unsafe extern "C" fn(*const c_char)->*const c_char;
struct Native {_library:Library,create:Create,send:Send,receive:Receive,execute:Execute}
type Reply=std::result::Result<Value,String>;
pub struct Hub {
    native:Native,
    pending:Mutex<HashMap<u64,oneshot::Sender<Reply>>>,
    next:AtomicU64,
    pub events:broadcast::Sender<Value>,
}
impl Hub {
    pub fn open(path:&str)->Result<Arc<Self>> {
        // SAFETY: symbols are TDLib's documented C ABI. Library is retained for the
        // lifetime of every function pointer and of the dedicated receive thread.
        let native=unsafe {
            let library=Library::new(path).with_context(||format!("Không mở được TDLib: {path}. Xem README để cài libtdjson"))?;
            let create=*library.get::<Create>(b"td_create_client_id\0")?;
            let send=*library.get::<Send>(b"td_send\0")?;
            let receive=*library.get::<Receive>(b"td_receive\0")?;
            let execute=*library.get::<Execute>(b"td_execute\0")?;
            Native{_library:library,create,send,receive,execute}
        };
        let (events,_)=broadcast::channel(2048);
        let hub=Arc::new(Self{native,pending:Mutex::new(HashMap::new()),next:AtomicU64::new(1),events});
        // execute is only used before starting td_receive; returned buffers never cross threads.
        let verbosity=CString::new(r#"{"@type":"setLogVerbosityLevel","new_verbosity_level":1}"#)?;
        unsafe {(hub.native.execute)(verbosity.as_ptr());}
        let receiver=hub.clone();
        std::thread::Builder::new().name("tdlib-receiver".into()).spawn(move||loop {
            let ptr=unsafe {(receiver.native.receive)(0.1)};
            if ptr.is_null(){continue;}
            let raw=unsafe {CStr::from_ptr(ptr)}.to_bytes();
            let Ok(value)=serde_json::from_slice::<Value>(raw) else{continue;};
            if let Some(id)=value["@extra"].as_u64() {
                if let Some(tx)=receiver.pending.lock().unwrap().remove(&id) {
                    let result=if value["@type"]=="error" {Err(format!("TDLib {}: {}",value["code"],value["message"].as_str().unwrap_or("Unknown error")))} else{Ok(value)};
                    let _=tx.send(result);
                }
            } else {let _=receiver.events.send(value);}
        })?;
        Ok(hub)
    }
    pub fn create(&self)->i32 {unsafe{(self.native.create)()}}
    pub async fn call(&self,client:i32,mut request:Value)->Result<Value> {
        let id=self.next.fetch_add(1,Ordering::Relaxed);
        request["@extra"]=json!(id);
        let serialized=CString::new(serde_json::to_string(&request)?)?;
        let (tx,rx)=oneshot::channel();
        self.pending.lock().unwrap().insert(id,tx);
        struct Guard<'a>(&'a Hub,u64);
        impl Drop for Guard<'_>{fn drop(&mut self){self.0.pending.lock().unwrap().remove(&self.1);}}
        let _guard=Guard(self,id);
        // td_send may be called concurrently, according to TDLib's API contract.
        unsafe{(self.native.send)(client,serialized.as_ptr());}
        tokio::time::timeout(Duration::from_secs(180),rx).await.context("TDLib request timed out")?.context("TDLib response channel closed")?.map_err(anyhow::Error::msg)
    }
}
