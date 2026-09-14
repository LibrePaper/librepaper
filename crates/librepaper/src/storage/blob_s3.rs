//! S3-compatible immutable blob storage.

use std::ops::Range;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt};
use object_store::aws::AmazonS3Builder;
use object_store::path::Path;
use object_store::{ObjectStore, PutMode, PutOptions};

use super::blob::{validate_object_key, BlobError, BlobInfo, BlobMetadata, BlobResult, BlobStore};

pub struct S3Store {
    inner: Arc<dyn ObjectStore>,
    description: String,
}

impl S3Store {
    pub fn new(
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
        if let Some(endpoint) = endpoint.filter(|v| !v.is_empty()) {
            builder = builder.with_endpoint(endpoint);
        }
        let inner = builder
            .build()
            .map_err(|e| format!("invalid S3 configuration: {e}"))?;
        Ok(Self {
            inner: Arc::new(inner),
            description: format!("s3://{bucket}"),
        })
    }

    fn path(key: &str) -> BlobResult<Path> {
        validate_object_key(key)?;
        Path::parse(key).map_err(|e| BlobError::Other(e.to_string()))
    }
}

fn map_error(error: object_store::Error) -> BlobError {
    match error {
        object_store::Error::NotFound { .. } => BlobError::NotFound,
        object_store::Error::AlreadyExists { .. } | object_store::Error::Precondition { .. } => {
            BlobError::Conflict
        }
        other => BlobError::Other(other.to_string()),
    }
}

#[async_trait]
impl BlobStore for S3Store {
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
        let metadata = self
            .inner
            .head(&Self::path(key)?)
            .await
            .map_err(map_error)?;
        Ok(BlobMetadata {
            size: metadata.size,
            version: metadata.e_tag.or(metadata.version),
        })
    }
    async fn get_range(&self, key: &str, range: Range<u64>) -> BlobResult<Vec<u8>> {
        Ok(self
            .inner
            .get_range(&Self::path(key)?, range)
            .await
            .map_err(map_error)?
            .to_vec())
    }
    async fn exists(&self, key: &str) -> BlobResult<bool> {
        match self.inner.head(&Self::path(key)?).await {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(map_error(e)),
        }
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
                Err(e) => return Err(map_error(e)),
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
        Ok(rows
            .into_iter()
            .map(|r| BlobInfo {
                key: r.location.to_string(),
                size: i64::try_from(r.size).unwrap_or(i64::MAX),
                modified_at: Some(r.last_modified.into()),
            })
            .collect())
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
        let offset = Self::path(after.unwrap_or(prefix.as_ref().map_or("", Path::as_ref)))?;
        let rows = self
            .inner
            .list_with_offset(prefix.as_ref(), &offset)
            .take(limit)
            .try_collect::<Vec<_>>()
            .await
            .map_err(map_error)?;
        Ok(rows
            .into_iter()
            .map(|row| BlobInfo {
                key: row.location.to_string(),
                size: i64::try_from(row.size).unwrap_or(i64::MAX),
                modified_at: Some(row.last_modified.into()),
            })
            .collect())
    }
    fn describe(&self) -> String {
        self.description.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "requires LIBREPAPER_TEST_S3_ENDPOINT"]
    async fn s3_immutable_blob_contract() {
        let endpoint = std::env::var("LIBREPAPER_TEST_S3_ENDPOINT")
            .expect("set LIBREPAPER_TEST_S3_ENDPOINT to run the S3 contract");
        let bucket =
            std::env::var("LIBREPAPER_TEST_S3_BUCKET").unwrap_or_else(|_| "librepaper-test".into());
        let store = S3Store::new(Some(&endpoint), "us-east-1", &bucket, true).unwrap();
        let prefix = format!("contract/{}/", uuid::Uuid::now_v7());
        let key = format!("{prefix}object");
        store
            .put_new(&key, b"immutable-body".to_vec(), "text/plain")
            .await
            .unwrap();
        assert!(matches!(
            store
                .put_new(&key, b"replacement".to_vec(), "text/plain")
                .await,
            Err(BlobError::Conflict)
        ));
        assert_eq!(store.get(&key).await.unwrap(), b"immutable-body");
        assert_eq!(store.get_range(&key, 2..7).await.unwrap(), b"mutab");
        let metadata = store.head(&key).await.unwrap();
        assert_eq!(metadata.size, 14);
        assert!(metadata.version.is_some());
        let listed = store.list_page(&prefix, None, 10).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].key, key);
        store.delete(std::slice::from_ref(&key)).await.unwrap();
        store.delete(std::slice::from_ref(&key)).await.unwrap();
        assert!(!store.exists(&key).await.unwrap());
    }
}
