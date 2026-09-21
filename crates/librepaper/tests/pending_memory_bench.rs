//! Ignored pending-buffer experiment. Run only against a disposable database.
use std::sync::Arc;
use std::time::Instant;
use librepaper::config::Configuration;
use librepaper::log::{Ingested, Registry};
use librepaper::postgres::{NewAccount, NewDocument, PostgresCatalog, PostgresOptions};
use librepaper::{BlobStore, FsStore};
use loro::{ExportMode, LoroDoc, VersionVector};
use serde_json::json;
use uuid::Uuid;

fn rss() -> u64 {
    std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find(|l| l.starts_with("VmRSS:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|n| n.parse().ok())).unwrap_or(0)
}
fn blobs() -> Arc<dyn BlobStore> { let d=tempfile::tempdir().unwrap(); let p=d.path().to_path_buf(); std::mem::forget(d); Arc::new(FsStore::new(p,false)) }
struct Outbox { doc:LoroDoc, at:VersionVector }
impl Outbox { fn new()->Self { Self{doc:LoroDoc::new(),at:VersionVector::default()} } fn update(&mut self)->Vec<u8>{ let t=self.doc.get_text("t"); t.insert_utf16(t.len_utf16(), &"x".repeat(1024)).unwrap(); self.doc.commit(); let b=self.doc.export(ExportMode::Updates{from:std::borrow::Cow::Borrowed(&self.at)}).unwrap(); self.at=self.doc.oplog_vv(); b } }

#[tokio::test]
#[ignore]
async fn pending_memory_shapes() {
 let url=std::env::var("PENDING_MEMORY_BENCH_POSTGRES_URL").expect("database URL");
 let n:usize=std::env::var("PENDING_MEMORY_BENCH_DOCUMENTS").unwrap_or("1,10,50,100".into()).split(',').next().unwrap().parse().unwrap();
 let cat=Arc::new(PostgresCatalog::connect(PostgresOptions::new(url)).await.unwrap()); cat.migrate().await.unwrap();
 let a=cat.create_account(NewAccount{kind:"registered".into(),provider:Some("bench".into()),provider_subject:Some(Uuid::new_v4().to_string()),handle:format!("pending-{}",Uuid::new_v4()),display_name:"bench".into(),email:None}).await.unwrap();
 let reg=Registry::new(cat.clone(),blobs(),Arc::new(Configuration::default()),"pending-memory".into()); let mut docs=Vec::new(); let mut boxes=Vec::new();
 for i in 0..n { let id=cat.create_document(NewDocument{slug:format!("pending-{}-{}",Uuid::new_v4(),i),owner_id:a.id,ownership_mode:"owned".into(),title:"bench".into(),source_format:"markdown".into(),main_path:"paper.md".into(),settings:json!({"version":1})}).await.unwrap().id; docs.push(reg.get(id,"bench").await.unwrap()); boxes.push(Outbox::new()); }
 let base=rss(); let started=Instant::now(); let mut accepted=0usize; let mut pending=0usize;
 for (d,o) in docs.iter().zip(boxes.iter_mut()) { loop { let b=o.update(); let sz=b.len(); match d.ingest(1,"bench","bench",accepted as i64,b).await { Ingested::Accepted=>{accepted+=1;pending+=sz}, Ingested::Retryable(_)=>break, x=>panic!("unexpected {x:?}") } } }
 println!("documents={n} rss_kib_base={} rss_kib_pending={} accepted={} pending_bytes={} elapsed_ms={}",base,rss(),accepted,pending,started.elapsed().as_millis());
}
