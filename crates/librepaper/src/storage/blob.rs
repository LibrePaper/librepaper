//! Immutable deployment bytes backed by `object_store`.

use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt};
use object_store::{
    aws::AmazonS3Builder, local::LocalFileSystem, path::Path as ObjectPath, ObjectStore,
    ObjectStoreExt, PutMode, PutOptions,
};
use std::{
    ops::Range,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

pub fn validate_object_key(key: &str) -> BlobResult<()> {
    if key.is_empty() || key.contains('\0') || key.contains('\\') {
        return Err(BlobError::Other("invalid object key".into()));
    }
    let mut any = false;
    for component in Path::new(key).components() {
        match component {
            Component::Normal(_) => any = true,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(BlobError::Other("invalid object key".into()))
            }
        }
    }
    if !any {
        return Err(BlobError::Other("empty object key".into()));
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct BlobInfo {
    pub key: String,
    pub size: i64,
    pub modified_at: Option<std::time::SystemTime>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobMetadata {
    pub size: u64,
    pub version: Option<String>,
}
#[derive(Debug)]
pub enum BlobError {
    NotFound,
    Conflict,
    Other(String),
}
impl std::fmt::Display for BlobError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("no such object"),
            Self::Conflict => f.write_str("the object was written by someone else"),
            Self::Other(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for BlobError {}
pub type BlobResult<T> = Result<T, BlobError>;

#[async_trait]
pub trait BlobStore: Send + Sync {
    async fn get(&self, key: &str) -> BlobResult<Vec<u8>>;
    async fn head(&self, key: &str) -> BlobResult<BlobMetadata> {
        let bytes = self.get(key).await?;
        Ok(BlobMetadata {
            size: bytes.len() as u64,
            version: None,
        })
    }
    async fn length(&self, key: &str) -> BlobResult<u64> {
        Ok(self.head(key).await?.size)
    }
    async fn get_range(&self, key: &str, range: Range<u64>) -> BlobResult<Vec<u8>> {
        let bytes = self.get(key).await?;
        if range.start > range.end || range.end > bytes.len() as u64 {
            return Err(BlobError::Other("invalid object byte range".into()));
        }
        Ok(bytes[range.start as usize..range.end as usize].to_vec())
    }
    async fn exists(&self, key: &str) -> BlobResult<bool> {
        match self.head(key).await {
            Ok(_) => Ok(true),
            Err(BlobError::NotFound) => Ok(false),
            Err(error) => Err(error),
        }
    }
    async fn put_new(&self, key: &str, body: Vec<u8>, content_type: &str) -> BlobResult<()>;
    async fn delete(&self, keys: &[String]) -> BlobResult<()>;
    async fn list(&self, prefix: &str) -> BlobResult<Vec<BlobInfo>>;
    async fn list_page(
        &self,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
    ) -> BlobResult<Vec<BlobInfo>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut rows = self.list(prefix).await?;
        rows.retain(|row| after.is_none_or(|cursor| row.key.as_str() > cursor));
        rows.sort_by(|a, b| a.key.cmp(&b.key));
        rows.truncate(limit);
        Ok(rows)
    }
    fn describe(&self) -> String;
    fn is_local(&self) -> bool {
        false
    }
    async fn capacity_snapshot(&self) -> Option<serde_json::Value> {
        None
    }
}

pub struct ObjectBlobStore {
    inner: Arc<dyn ObjectStore>,
    description: String,
    local_root: Option<PathBuf>,
}
impl ObjectBlobStore {
    pub fn filesystem(root: impl Into<PathBuf>, durable: bool) -> Result<Self, String> {
        let root = root.into();
        std::fs::create_dir_all(&root)
            .map_err(|error| format!("could not create {}: {error}", root.display()))?;
        let store = LocalFileSystem::new_with_prefix(&root)
            .map_err(|error| error.to_string())?
            .with_automatic_cleanup(true)
            .with_fsync(durable);
        Ok(Self {
            inner: Arc::new(store),
            description: root.display().to_string(),
            local_root: Some(root),
        })
    }
    pub fn s3(
        endpoint: Option<&str>,
        region: &str,
        bucket: &str,
        allow_http: bool,
    ) -> Result<Self, String> {
        if region.is_empty() || bucket.is_empty() {
            return Err("S3 region and bucket are required".into());
        }
        let mut builder = AmazonS3Builder::from_env()
            .with_region(region)
            .with_bucket_name(bucket)
            .with_allow_http(allow_http);
        if let Some(endpoint) = endpoint.filter(|value| !value.is_empty()) {
            builder = builder.with_endpoint(endpoint);
        }
        Ok(Self {
            inner: Arc::new(builder.build().map_err(|error| error.to_string())?),
            description: format!("s3://{bucket}"),
            local_root: None,
        })
    }
    fn path(key: &str) -> BlobResult<ObjectPath> {
        validate_object_key(key)?;
        ObjectPath::parse(key).map_err(|error| BlobError::Other(error.to_string()))
    }
}
fn map_error(error: object_store::Error) -> BlobError {
    match error {
        object_store::Error::NotFound { .. } => BlobError::NotFound,
        object_store::Error::AlreadyExists { .. } | object_store::Error::Precondition { .. } => {
            BlobError::Conflict
        }
        error => BlobError::Other(error.to_string()),
    }
}

#[async_trait]
impl BlobStore for ObjectBlobStore {
    async fn get(&self, key: &str) -> BlobResult<Vec<u8>> {
        Ok(self
            .inner
            .get(&Self::path(key)?)
            .await
            .map_err(map_error)?
            .bytes()
            .await
            .map_err(map_error)?
            .to_vec())
    }
    async fn head(&self, key: &str) -> BlobResult<BlobMetadata> {
        let row = self
            .inner
            .head(&Self::path(key)?)
            .await
            .map_err(map_error)?;
        Ok(BlobMetadata {
            size: row.size,
            version: row.e_tag.or(row.version),
        })
    }
    async fn get_range(&self, key: &str, range: Range<u64>) -> BlobResult<Vec<u8>> {
        if range.start > range.end || range.end - range.start > 1024 * 1024 {
            return Err(BlobError::Other(
                "object range exceeds the 1 MiB read bound".into(),
            ));
        }
        Ok(self
            .inner
            .get_range(&Self::path(key)?, range)
            .await
            .map_err(map_error)?
            .to_vec())
    }
    async fn put_new(&self, key: &str, body: Vec<u8>, _content_type: &str) -> BlobResult<()> {
        self.inner
            .put_opts(
                &Self::path(key)?,
                body.into(),
                PutOptions {
                    mode: PutMode::Create,
                    ..Default::default()
                },
            )
            .await
            .map_err(map_error)?;
        Ok(())
    }
    async fn delete(&self, keys: &[String]) -> BlobResult<()> {
        for key in keys {
            match self.inner.delete(&Self::path(key)?).await {
                Ok(()) | Err(object_store::Error::NotFound { .. }) => {}
                Err(error) => return Err(map_error(error)),
            }
        }
        Ok(())
    }
    async fn list(&self, prefix: &str) -> BlobResult<Vec<BlobInfo>> {
        let prefix = if prefix.is_empty() {
            None
        } else {
            Some(Self::path(prefix)?)
        };
        let rows = self
            .inner
            .list(prefix.as_ref())
            .try_collect::<Vec<_>>()
            .await
            .map_err(map_error)?;
        Ok(rows.into_iter().map(info).collect())
    }
    async fn list_page(
        &self,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
    ) -> BlobResult<Vec<BlobInfo>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let prefix = if prefix.is_empty() {
            None
        } else {
            Some(Self::path(prefix)?)
        };
        let offset = Self::path(after.unwrap_or(prefix.as_ref().map_or("", ObjectPath::as_ref)))?;
        let mut stream = self.inner.list_with_offset(prefix.as_ref(), &offset);
        let mut remaining = limit;
        let mut rows = Vec::new();
        while remaining > 0 {
            let maybe_row = stream.next().await;
            let next = match maybe_row {
                Some(row) => row.map_err(map_error)?,
                None => break,
            };
            rows.push(next);
            remaining -= 1;
        }
        Ok(rows.into_iter().map(info).collect())
    }
    fn describe(&self) -> String {
        self.description.clone()
    }
    fn is_local(&self) -> bool {
        self.local_root.is_some()
    }
    async fn capacity_snapshot(&self) -> Option<serde_json::Value> {
        let root = self.local_root.clone()?;
        tokio::task::spawn_blocking(move || { let total = fs2::total_space(&root).ok()?; let available = fs2::available_space(&root).ok()?;
            Some(serde_json::json!({"kind":"filesystem", "total_bytes":total, "available_bytes":available,
                "allocated_bytes":total.saturating_sub(available), "reserved_bytes":0, "is_primary":true})) }).await.ok().flatten()
    }
}
fn info(row: object_store::ObjectMeta) -> BlobInfo {
    BlobInfo {
        key: row.location.to_string(),
        size: i64::try_from(row.size).unwrap_or(i64::MAX),
        modified_at: Some(row.last_modified.into()),
    }
}

/// Compatibility wrapper used by focused tests and the font cache.
pub struct FsStore(ObjectBlobStore);
impl FsStore {
    pub fn new(root: impl Into<PathBuf>, durable: bool) -> Self {
        Self(ObjectBlobStore::filesystem(root, durable).expect("filesystem object store"))
    }
}
#[async_trait]
impl BlobStore for FsStore {
    async fn get(&self, key: &str) -> BlobResult<Vec<u8>> {
        self.0.get(key).await
    }
    async fn head(&self, key: &str) -> BlobResult<BlobMetadata> {
        self.0.head(key).await
    }
    async fn get_range(&self, key: &str, range: Range<u64>) -> BlobResult<Vec<u8>> {
        self.0.get_range(key, range).await
    }
    async fn exists(&self, key: &str) -> BlobResult<bool> {
        self.0.exists(key).await
    }
    async fn put_new(&self, key: &str, body: Vec<u8>, content_type: &str) -> BlobResult<()> {
        self.0.put_new(key, body, content_type).await
    }
    async fn delete(&self, keys: &[String]) -> BlobResult<()> {
        self.0.delete(keys).await
    }
    async fn list(&self, prefix: &str) -> BlobResult<Vec<BlobInfo>> {
        self.0.list(prefix).await
    }
    async fn list_page(
        &self,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
    ) -> BlobResult<Vec<BlobInfo>> {
        self.0.list_page(prefix, after, limit).await
    }
    fn describe(&self) -> String {
        self.0.describe()
    }
    fn is_local(&self) -> bool {
        true
    }
    async fn capacity_snapshot(&self) -> Option<serde_json::Value> {
        self.0.capacity_snapshot().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unsafe_keys() {
        for key in ["", "../escape", "/absolute", "a\\b", "\0"] {
            assert!(validate_object_key(key).is_err());
        }
    }
    #[tokio::test]
    async fn filesystem_contract() {
        let root = tempfile::tempdir().expect("temporary object directory");
        let store = FsStore::new(root.path(), false);
        let key = "documents/one/file".to_string();
        store
            .put_new(&key, b"hello".to_vec(), "text/plain")
            .await
            .expect("put");
        assert!(matches!(
            store.put_new(&key, b"again".to_vec(), "text/plain").await,
            Err(BlobError::Conflict)
        ));
        assert_eq!(store.get_range(&key, 1..4).await.expect("range"), b"ell");
        assert_eq!(store.list("documents/").await.expect("list").len(), 1);
        store
            .delete(std::slice::from_ref(&key))
            .await
            .expect("delete");
        store
            .delete(std::slice::from_ref(&key))
            .await
            .expect("delete twice");
        assert!(!store.exists(&key).await.expect("exists"));
    }
}
