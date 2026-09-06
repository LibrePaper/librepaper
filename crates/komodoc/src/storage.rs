//! Where a deployment keeps its bytes, and how it is told.
//!
//! The point of the option is not the option: it is that a small VPS can run
//! `komodoc serve` while holding no durable state of its own. The bytes, the
//! bill and the ownership of the data are the operator's, in a bucket they
//! supply. A directory remains the default, because that is what running it on
//! your own machine should mean.

use std::sync::Arc;

use clap::Args;

use crate::blob::{legacy_source_key, BlobStore, FsStore};
use crate::s3::S3Store;
use crate::util::first_of;

#[derive(Clone, Debug, Default)]
pub struct StorageOptions {
    pub dir: String,

    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub prefix: String,
    pub access_key: String,
    pub secret_key: String,

    /// The operator asserting that exactly one process writes this bucket,
    /// which is true of the VPS shape by construction. It makes the in-process
    /// mutex the authority and writes the index unconditionally -- which is
    /// the only way to run against a bucket whose conditional writes do not
    /// work. It is never inferred: a silently degraded index is a lost
    /// document, so it has to be asked for.
    pub single_writer: bool,
}

impl StorageOptions {
    /// Fills in whatever was not passed as a flag. Credentials come from the
    /// environment by preference, and `open_storage` says so when they do not:
    /// a flag lands in the process table, where every other process on the
    /// machine can read it, and in the shell history of whoever typed it.
    pub fn fill_from_environment(&mut self) {
        let env = |name: &str| std::env::var(name).unwrap_or_default();
        self.endpoint = first_of(&[&self.endpoint, &env("KOMODOC_S3_ENDPOINT")]);
        self.bucket = first_of(&[&self.bucket, &env("KOMODOC_S3_BUCKET")]);
        self.region = first_of(&[&self.region, &env("KOMODOC_S3_REGION"), "auto"]);
        self.prefix = first_of(&[&self.prefix, &env("KOMODOC_S3_PREFIX"), "komodoc/"]);
        self.access_key = first_of(&[&self.access_key, &env("KOMODOC_S3_ACCESS_KEY")]);
        self.secret_key = first_of(&[&self.secret_key, &env("KOMODOC_S3_SECRET_KEY")]);
    }
}

/// The blob store a deployment was configured for, having checked that it
/// actually works. A misconfigured bucket must fail here, at startup, in front
/// of whoever is starting it -- not on the first upload, in front of a user.
pub async fn open_storage(mut options: StorageOptions) -> Result<Arc<dyn BlobStore>, String> {
    options.fill_from_environment();

    if options.bucket.is_empty() {
        let dir = first_of(&[
            &options.dir,
            &std::env::var("KOMODOC_DATA").unwrap_or_default(),
            "komodoc-data",
        ]);
        let absolute =
            std::path::absolute(&dir).map_err(|err| format!("bad --data directory: {err}"))?;
        std::fs::create_dir_all(&absolute)
            .map_err(|err| format!("could not create {}: {err}", absolute.display()))?;
        return Ok(Arc::new(FsStore::new(absolute)));
    }

    // A directory and a bucket are two answers to one question.
    if !options.dir.is_empty() {
        return Err(
            "--data and --s3-bucket are two places to keep the same bytes; pick one".into(),
        );
    }
    if options.endpoint.is_empty() {
        return Err("--s3-endpoint is required with --s3-bucket.\n\n    \
                    Cloudflare R2  https://<account>.r2.cloudflarestorage.com\n    \
                    AWS S3         https://s3.<region>.amazonaws.com\n    \
                    MinIO          https://minio.example.com"
            .into());
    }
    if options.access_key.is_empty() || options.secret_key.is_empty() {
        return Err(format!(
            "this needs credentials for {}.\n\n  \
             Through the environment, so they stay out of the process table\n  \
             and out of your shell history:\n\n    \
             export KOMODOC_S3_ACCESS_KEY=...\n    export KOMODOC_S3_SECRET_KEY=...",
            options.bucket
        ));
    }

    let store = S3Store::new(&options);
    let report = store.probe().await;
    print!("{}", report.describe(&options));
    if !report.conditional_writes && !options.single_writer {
        return Err(format!(
            "{} does not support conditional writes, which is what keeps the\n  \
             index correct when two writes race. {}\n\n  \
             If exactly one komodoc process writes this bucket -- which is true of\n  \
             a single server, and is the ordinary case -- say so and it will use\n  \
             its own lock instead:\n\n    \
             komodoc serve --s3-bucket {} --single-writer\n\n  \
             Do not pass it if anything else writes these keys.",
            options.endpoint, report.why, options.bucket
        ));
    }
    Ok(Arc::new(store))
}

/// The flags that say where the bytes go. `serve` and `seed` both take them,
/// and they are described in one place so the two cannot drift.
#[derive(Args, Clone, Debug, Default)]
pub struct StorageFlags {
    /// Directory for documents and comments (default komodoc-data)
    #[arg(long, value_name = "DIR")]
    pub data: Option<String>,
    /// S3-compatible endpoint URL; or $KOMODOC_S3_ENDPOINT
    #[arg(long, value_name = "URL")]
    pub s3_endpoint: Option<String>,
    /// Bucket to keep documents in, instead of a directory; or $KOMODOC_S3_BUCKET
    #[arg(long, value_name = "BUCKET")]
    pub s3_bucket: Option<String>,
    /// Bucket region (default auto); or $KOMODOC_S3_REGION
    #[arg(long, value_name = "REGION")]
    pub s3_region: Option<String>,
    /// Key prefix, so a bucket can be shared (default komodoc/); or $KOMODOC_S3_PREFIX
    #[arg(long, value_name = "PREFIX")]
    pub s3_prefix: Option<String>,
    /// Prefer $KOMODOC_S3_ACCESS_KEY: a flag is visible in the process table
    #[arg(long, value_name = "KEY")]
    pub s3_access_key: Option<String>,
    /// Prefer $KOMODOC_S3_SECRET_KEY: a flag is visible in the process table
    #[arg(long, value_name = "SECRET")]
    pub s3_secret_key: Option<String>,
    /// Assert that only this process writes the bucket, when it has no conditional writes
    #[arg(long)]
    pub single_writer: bool,
}

impl StorageFlags {
    pub fn options(&self) -> StorageOptions {
        if self.s3_access_key.as_deref().is_some_and(|v| !v.is_empty()) {
            eprintln!(
                "warning: --s3-access-key is visible to every process on this machine\n  \
                 and lands in your shell history. Prefer KOMODOC_S3_ACCESS_KEY."
            );
        }
        if self.s3_secret_key.as_deref().is_some_and(|v| !v.is_empty()) {
            eprintln!(
                "warning: --s3-secret-key is visible to every process on this machine\n  \
                 and lands in your shell history. Prefer KOMODOC_S3_SECRET_KEY."
            );
        }
        let value = |flag: &Option<String>| flag.clone().unwrap_or_default();
        StorageOptions {
            dir: value(&self.data),
            endpoint: value(&self.s3_endpoint),
            bucket: value(&self.s3_bucket),
            region: value(&self.s3_region),
            prefix: value(&self.s3_prefix),
            access_key: value(&self.s3_access_key),
            secret_key: value(&self.s3_secret_key),
            single_writer: self.single_writer,
        }
    }
}

/// Moves a document's source to the key layout now shared. `serve` used to
/// keep it beside the rendered versions, as documents/<slug>/source.txt; it
/// lives at sources/<slug> now. One pass at startup, and nothing to do on a
/// store that never had the old layout.
pub async fn migrate_legacy_source(blobs: &dyn BlobStore) -> usize {
    let Ok(found) = blobs.list("documents/").await else {
        return 0;
    };
    let mut moved = 0;
    for object in found {
        let Some(rest) = object.key.strip_prefix("documents/") else {
            continue;
        };
        let Some(slug) = rest.strip_suffix("/source.txt") else {
            continue;
        };
        if blobs.get(&legacy_source_key(slug)).await.is_ok() {
            continue; // already moved
        }
        let Ok(body) = blobs.get(&object.key).await else {
            continue;
        };
        if blobs
            .put(&legacy_source_key(slug), body, "text/plain; charset=utf-8")
            .await
            .is_err()
        {
            continue;
        }
        let _ = blobs.delete(std::slice::from_ref(&object.key)).await;
        moved += 1;
    }
    moved
}
