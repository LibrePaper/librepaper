//! Default resident-capacity stress check.
//!
//! This is a release check rather than a per-commit test. It deliberately
//! keeps admitted rooms dirty, so admission cannot discard them to make room.

use std::fs;
use std::sync::Arc;

use crate::config::Configuration;
use crate::document::store;
use crate::room::{Room, RoomAdmissionError, RoomSet};
use crate::storage::blob::{self, BlobStore};
use crate::storage::catalog::Catalog;
use crate::storage::journal::{self, CoordinatorLimits};

fn rss_bytes() -> Option<u64> {
    fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmHWM:")?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
        })
        .map(|kb| kb * 1024)
}

fn write_report(report: &serde_json::Value) {
    let encoded = serde_json::to_vec_pretty(report).expect("encode stress report");
    println!(
        "{}",
        String::from_utf8(encoded.clone()).expect("report is utf8")
    );
    if let Ok(path) = std::env::var("LIBREPAPER_DEFAULTS_REPORT") {
        fs::write(path, encoded).expect("write stress report");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "release default-capacity workload; run with --ignored --nocapture"]
async fn persistence_default_limits_refuse_without_discarding_dirty_rooms() {
    let config = Arc::new(Configuration::default());
    let root = tempfile::tempdir().expect("temporary deployment");
    let objects: Arc<dyn BlobStore> =
        Arc::new(blob::FsStore::new(root.path().join("objects"), true));
    let catalog = Arc::new(
        Catalog::open_with(root.path().join("catalog.db"), true).expect("durable catalog opens"),
    );
    let store = Arc::new(
        store::Store::open_with_catalog(objects.clone(), config.clone(), catalog.clone())
            .await
            .expect("store opens"),
    );
    let rooms = RoomSet::new(objects.clone(), config.clone());
    rooms.attach_deployment_lock(
        crate::server::serve::acquire_writer_lock(&root.path().join("writer.lock"))
            .expect("exclusive deployment lock"),
    );
    rooms.attach_store(store.clone());
    journal::JournalStore::new(catalog.clone())
        .initialize_local("defaults-stress")
        .expect("journal initializes");
    let limits = config.persistence();
    let journal = journal::JournalRuntime::new_with_policy(
        catalog,
        objects,
        "defaults-stress",
        CoordinatorLimits::from_persistence(&limits),
        limits,
        -1,
        -1,
    )
    .expect("journal runtime opens");
    rooms.attach_journal(journal);

    let source_bytes = config.max_document.saturating_sub(32);
    let mut admitted: Vec<(String, Arc<Room>, String)> = Vec::new();
    let mut refusal = None;
    for index in 0..=config.session.rooms_max {
        let slug = format!("defaults-capacity-{index}");
        let marker = format!("default capacity document {index}\n");
        let source = format!(
            "{}{}",
            marker,
            "x".repeat(source_bytes.saturating_sub(marker.len()))
        );
        store
            .put(store::Publication {
                slug: slug.clone(),
                source: source.clone(),
                source_format: "markdown".into(),
                owner: format!("defaults-owner-{index}"),
                ..Default::default()
            })
            .await
            .expect("publish capacity document");
        match rooms.try_get(&slug).await {
            Ok(room) => {
                room.set_main_file(&source, "markdown", "main.md")
                    .await
                    .expect("dirty admitted room");
                admitted.push((slug, room, marker));
            }
            Err(
                error @ (RoomAdmissionError::AtCapacity { .. }
                | RoomAdmissionError::UnreadableState),
            ) => {
                refusal = Some(error);
                break;
            }
        }
    }
    let error = refusal.expect("default resident limits must refuse admission");
    assert!(admitted.len() <= config.session.rooms_max);
    assert_eq!(
        rooms.open_count().await,
        admitted.len(),
        "dirty rooms remain resident"
    );
    for (slug, room, marker) in &admitted {
        let recovered = room.source().await;
        assert_eq!(recovered.len(), source_bytes, "room was discarded: {slug}");
        assert!(
            recovered.starts_with(marker),
            "room content changed: {slug}"
        );
    }
    let report = serde_json::json!({
        "rooms_admitted": admitted.len(),
        "estimated_source_bytes": admitted.len().saturating_mul(source_bytes),
        "rooms_max": config.session.rooms_max,
        "rooms_bytes_max": config.session.rooms_bytes_max,
        "rss_high_water_bytes": rss_bytes(),
        "refusal": error.to_string(),
    });
    write_report(&report);
}
