//! Adapted from VibeFactory app/logging: log facade, async worker, bounded ring, 4 MiB rotation.
use std::{collections::VecDeque, fs::{self,File,OpenOptions},io::Write,path::{Path,PathBuf},sync::{mpsc,Arc,RwLock},thread,time::{SystemTime,UNIX_EPOCH}};
use log::{Log,Metadata,Record,LevelFilter};

const MAX_ENTRIES:usize=2000;
const MAX_BYTES:u64=4*1024*1024;

#[derive(Clone,Debug)]
#[allow(dead_code)]
pub struct Entry {
    pub timestamp_ms:u64,
    pub level:String,
    pub target:String,
    pub message:String,
}
enum Message {Entry(Entry),Flush(mpsc::SyncSender<()>),}
struct Sink { tx:mpsc::Sender<Message> }
impl Log for Sink {
    fn enabled(&self,_:&Metadata<'_>)->bool {true}
    fn log(&self,rec:&Record<'_>){
        let timestamp_ms=SystemTime::now().duration_since(UNIX_EPOCH)
            .map(|d|d.as_millis().min(u64::MAX as u128) as u64).unwrap_or(0);
        let _=self.tx.send(Message::Entry(Entry{
            timestamp_ms,level:rec.level().to_string(),target:rec.target().to_string(),
            message:rec.args().to_string(),
        }));
    }
    fn flush(&self) {
        let (tx,rx)=mpsc::sync_channel(0);
        if self.tx.send(Message::Flush(tx)).is_ok(){let _=rx.recv();}
    }
}
pub struct Runtime {entries:Arc<RwLock<VecDeque<Entry>>>,path:PathBuf}
impl Runtime {
    #[allow(dead_code)]
    pub fn snapshot(&self)->Vec<Entry> {
        self.entries.read().map(|g|g.iter().cloned().collect()).unwrap_or_default()
    }
    pub fn flush(&self){log::logger().flush();}
    pub fn path(&self)->&Path{&self.path}
}
fn open_log(path:&Path)->Option<File>{
    OpenOptions::new().create(true).append(true).open(path).ok()
}
fn rotate(path:&Path) {
    let previous=path.with_file_name("be200hub.previous.log");
    let _=fs::remove_file(&previous);
    let _=fs::rename(path,previous);
}
fn worker(rx:mpsc::Receiver<Message>,path:PathBuf,entries:Arc<RwLock<VecDeque<Entry>>>){
    let mut bytes=fs::metadata(&path).map(|m|m.len()).unwrap_or(0);
    if bytes>=MAX_BYTES {rotate(&path);bytes=0;}
    let mut file=open_log(&path);
    while let Ok(message)=rx.recv() {
        match message {
            Message::Entry(entry)=>{
                let line=format!("{} {:<5} {} {}\n",
                    entry.timestamp_ms,entry.level,entry.target,entry.message);
                if bytes>0 && bytes.saturating_add(line.len() as u64)>MAX_BYTES {
                    file.take();rotate(&path);file=open_log(&path);bytes=0;
                }
                if let Some(f)=file.as_mut(){
                    if f.write_all(line.as_bytes()).is_ok(){bytes+=line.len() as u64;}
                }
                if let Ok(mut g)=entries.write(){
                    g.push_back(entry);
                    while g.len()>MAX_ENTRIES{g.pop_front();}
                }
            }
            Message::Flush(tx)=>{if let Some(f)=file.as_mut(){let _=f.flush();}let _=tx.send(());}
        }
    }
}
pub fn default_directory()->PathBuf {
    std::env::var_os("LOCALAPPDATA").or_else(||std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(||std::env::current_dir().unwrap_or_else(|_|PathBuf::from(".")))
        .join("BE200AudioHub").join("logs")
}
pub fn install(dir:&Path)->Result<Runtime,String>{
    fs::create_dir_all(dir).map_err(|e|format!("create log directory: {e}"))?;
    let path=dir.join("be200hub.log");
    let entries=Arc::new(RwLock::new(VecDeque::new()));
    let (tx,rx)=mpsc::channel();
    let cloned=Arc::clone(&entries);
    let worker_path=path.clone();
    thread::Builder::new().name("BE200HubLogger".into())
        .spawn(move ||worker(rx,worker_path,cloned)).map_err(|e|e.to_string())?;
    log::set_boxed_logger(Box::new(Sink{tx})).map_err(|e|e.to_string())?;
    let level=match std::env::var("BE200_LOG").unwrap_or_default().to_lowercase().as_str(){
        "trace"=>LevelFilter::Trace,"debug"=>LevelFilter::Debug,
        "warn"=>LevelFilter::Warn,"error"=>LevelFilter::Error,"off"=>LevelFilter::Off,
        _=>LevelFilter::Info,
    };
    log::set_max_level(level);
    Ok(Runtime{entries,path})
}
