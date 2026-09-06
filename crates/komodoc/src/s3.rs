//! Any S3-compatible bucket: R2, AWS, MinIO, Backblaze, Ceph.
//!
//! Signed by hand rather than through an SDK. SigV4 is four HMACs over a
//! canonical request plus a digest of the body, which is the code below; the
//! alternative is pulling a dependency tree the size of the rest of this
//! program into a binary that has almost none.

use std::collections::BTreeMap;
use std::time::Duration;

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::{Method, Response};
use sha2::{Digest, Sha256};

use crate::blob::{version_of, BlobError, BlobInfo, BlobResult, BlobStore, BlobVersion};
use crate::clock::{amz_stamps, now_unix};
use crate::http::{client, truncate};
use crate::storage::StorageOptions;

pub struct S3Store {
    endpoint: String, // https://host, without the bucket
    bucket: String,
    region: String,
    prefix: String, // every key komodoc writes lives under this
    access_key: String,
    secret_key: String,
    /// Turns the conditional write off: see StorageOptions.
    single_writer: bool,
}

impl S3Store {
    pub fn new(options: &StorageOptions) -> S3Store {
        let mut prefix = options.prefix.clone();
        if !prefix.is_empty() && !prefix.ends_with('/') {
            prefix.push('/');
        }
        S3Store {
            endpoint: options.endpoint.trim_end_matches('/').to_string(),
            bucket: options.bucket.clone(),
            region: options.region.clone(),
            prefix,
            access_key: options.access_key.clone(),
            secret_key: options.secret_key.clone(),
            single_writer: options.single_writer,
        }
    }

    /// Puts a key under this deployment's prefix, so komodoc can share a bucket
    /// with whatever else the operator keeps there.
    fn scoped(&self, key: &str) -> String {
        format!("{}{}", self.prefix, key)
    }

    /// `escape_path` returns a path that already starts with a slash, so the
    /// bucket is joined to it directly rather than with one of its own.
    fn url(&self, key: &str) -> String {
        format!(
            "{}/{}{}",
            self.endpoint,
            self.bucket,
            escape_path(&self.scoped(key))
        )
    }

    async fn write(
        &self,
        key: &str,
        body: Vec<u8>,
        content_type: &str,
        conditions: &[(&str, String)],
    ) -> BlobResult<BlobVersion> {
        let mut headers = Vec::new();
        if !content_type.is_empty() {
            headers.push(("content-type", content_type.to_string()));
        }
        for (name, value) in conditions {
            headers.push((name, value.clone()));
        }
        let tag = version_of(&body);
        let response = self
            .send(Method::PUT, &self.url(key), &headers, body)
            .await?;
        let status = response.status().as_u16();
        // 412 is the conditional write refusing: the object moved. 409 is
        // what some implementations answer to a lost If-None-Match race.
        if status == 412 || status == 409 {
            return Err(BlobError::Conflict);
        }
        if status >= 300 {
            return Err(problem("PUT", key, response).await);
        }
        // Some implementations do not return an ETag on PUT; the digest is the
        // same answer, and matches what a later GET will report.
        Ok(etag(&response).unwrap_or(tag))
    }

    /// Signs a request and performs it.
    async fn send(
        &self,
        method: Method,
        target: &str,
        headers: &[(&str, String)],
        body: Vec<u8>,
    ) -> BlobResult<Response> {
        let parsed = url::Url::parse(target).map_err(|err| BlobError::Other(err.to_string()))?;
        let host = parsed.host_str().unwrap_or_default().to_string();
        let host = match parsed.port() {
            Some(port) => format!("{host}:{port}"),
            None => host,
        };
        let (stamp, day) = amz_stamps(now_unix());
        let payload_hash = hex::encode(Sha256::digest(&body));

        let mut signed_headers: BTreeMap<String, String> = BTreeMap::new();
        signed_headers.insert("host".into(), host);
        signed_headers.insert("x-amz-content-sha256".into(), payload_hash.clone());
        signed_headers.insert("x-amz-date".into(), stamp.clone());
        for (name, value) in headers {
            signed_headers.insert(name.to_lowercase(), value.trim().to_string());
        }

        let query: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let canonical = [
            method.as_str().to_string(),
            escape_path(parsed.path()),
            canonical_query(&query),
            signed_headers
                .iter()
                .map(|(k, v)| format!("{k}:{v}\n"))
                .collect::<String>(),
            signed_headers.keys().cloned().collect::<Vec<_>>().join(";"),
            payload_hash,
        ]
        .join("\n");
        let scope = format!("{day}/{}/s3/aws4_request", self.region);
        let to_sign = [
            "AWS4-HMAC-SHA256",
            &stamp,
            &scope,
            &hex::encode(Sha256::digest(canonical.as_bytes())),
        ]
        .join("\n");
        let signature = hex::encode(hmac_sha256(&self.signing_key(&day), to_sign.as_bytes()));
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.access_key,
            scope,
            signed_headers.keys().cloned().collect::<Vec<_>>().join(";"),
            signature
        );

        let mut request = client()
            .request(method, target)
            .timeout(Duration::from_secs(300));
        for (name, value) in &signed_headers {
            if name != "host" {
                request = request.header(name, value);
            }
        }
        request = request.header("authorization", authorization).body(body);
        request
            .send()
            .await
            .map_err(|err| BlobError::Other(err.to_string()))
    }

    fn signing_key(&self, day: &str) -> Vec<u8> {
        signing_key_for(&self.secret_key, day, &self.region, "s3")
    }

    /// A URL that fetches one object and expires. It is what lets the bytes of
    /// a document go from the bucket to the reader without passing through
    /// the server. The URL is a bearer token for its lifetime, which is why
    /// the lifetime is short.
    pub fn presign_get(&self, key: &str, lifetime_seconds: u64) -> Option<String> {
        let (stamp, day) = amz_stamps(now_unix());
        let scope = format!("{day}/{}/s3/aws4_request", self.region);
        let mut query = vec![
            (
                "X-Amz-Algorithm".to_string(),
                "AWS4-HMAC-SHA256".to_string(),
            ),
            (
                "X-Amz-Credential".to_string(),
                format!("{}/{scope}", self.access_key),
            ),
            ("X-Amz-Date".to_string(), stamp.clone()),
            ("X-Amz-Expires".to_string(), lifetime_seconds.to_string()),
            ("X-Amz-SignedHeaders".to_string(), "host".to_string()),
        ];
        let target = url::Url::parse(&self.url(key)).ok()?;
        let host = match target.port() {
            Some(port) => format!("{}:{port}", target.host_str()?),
            None => target.host_str()?.to_string(),
        };
        let canonical = [
            "GET".to_string(),
            escape_path(target.path()),
            canonical_query(&query),
            format!("host:{host}\n"),
            "host".to_string(),
            "UNSIGNED-PAYLOAD".to_string(),
        ]
        .join("\n");
        let to_sign = [
            "AWS4-HMAC-SHA256",
            &stamp,
            &scope,
            &hex::encode(Sha256::digest(canonical.as_bytes())),
        ]
        .join("\n");
        let signature = hex::encode(hmac_sha256(&self.signing_key(&day), to_sign.as_bytes()));
        query.push(("X-Amz-Signature".to_string(), signature));
        let mut base = target.clone();
        base.set_query(None);
        Some(format!("{base}?{}", canonical_query(&query)))
    }

    /// What a bucket turned out to support. A deployment must not discover on
    /// its first upload that its index cannot be written safely, so this runs
    /// at startup and is printed.
    pub async fn probe(&self) -> ProbeReport {
        const SCRATCH: &str = ".komodoc-probe";
        let mut report = ProbeReport::default();

        let first = match self
            .write(SCRATCH, b"komodoc".to_vec(), "text/plain", &[])
            .await
        {
            Ok(first) => first,
            Err(err) => {
                report.why = format!("the bucket could not be written to: {err}");
                return report;
            }
        };
        report.reachable = true;

        // A conditional write against the wrong version must be refused...
        let wrong = [(
            "If-Match",
            "\"0000000000000000000000000000000000000000\"".to_string(),
        )];
        match self
            .write(SCRATCH, b"no".to_vec(), "text/plain", &wrong)
            .await
        {
            Ok(_) => {
                report.why = "a write conditional on the wrong version was accepted, so a lost update would go unnoticed.".into();
                let _ = self.delete(&[SCRATCH.to_string()]).await;
                return report;
            }
            Err(BlobError::Conflict) => {}
            Err(err) => {
                report.why = format!("a conditional write answered {err} rather than refusing.");
                let _ = self.delete(&[SCRATCH.to_string()]).await;
                return report;
            }
        }

        // ...and so must a create-only write over something that exists.
        let create = [("If-None-Match", "*".to_string())];
        match self
            .write(SCRATCH, b"no".to_vec(), "text/plain", &create)
            .await
        {
            Ok(_) => {
                report.why = "a create-only write over an existing object was accepted.".into();
                let _ = self.delete(&[SCRATCH.to_string()]).await;
                return report;
            }
            Err(BlobError::Conflict) => {}
            Err(err) => {
                report.why = format!("a create-only write answered {err} rather than refusing.");
                let _ = self.delete(&[SCRATCH.to_string()]).await;
                return report;
            }
        }

        // And the right version must be accepted, or nothing could ever be
        // written.
        let right = [("If-Match", first)];
        if let Err(err) = self
            .write(SCRATCH, b"komodoc".to_vec(), "text/plain", &right)
            .await
        {
            report.why = format!("a write conditional on the current version was refused: {err}");
            let _ = self.delete(&[SCRATCH.to_string()]).await;
            return report;
        }

        let _ = self.delete(&[SCRATCH.to_string()]).await;
        report.conditional_writes = true;
        report
    }

    /// The two things an operator has to set on their bucket, printed rather
    /// than described: a CORS rule, if documents are to be fetched by readers
    /// directly, and a credential scoped to this prefix rather than to the
    /// whole account.
    pub fn advice(&self, origin: &str) -> String {
        format!(
            "\nTo let readers fetch documents straight from {bucket}, allow this origin:\n\n  \
             [{{\"AllowedOrigins\": [\"{origin}\"],\n    \"AllowedMethods\": [\"GET\", \"HEAD\"],\n    \
             \"AllowedHeaders\": [\"*\"],\n    \"ExposeHeaders\": [\"ETag\"],\n    \"MaxAgeSeconds\": 3600}}]\n\n\
             And a credential that can reach these keys and nothing else:\n\n  \
             {{\"Version\": \"2012-10-17\",\n   \"Statement\": [{{\"Effect\": \"Allow\",\n     \
             \"Action\": [\"s3:GetObject\", \"s3:PutObject\", \"s3:DeleteObject\", \"s3:ListBucket\"],\n     \
             \"Resource\": [\"arn:aws:s3:::{bucket}\", \"arn:aws:s3:::{bucket}/{prefix}*\"]}}]}}\n",
            bucket = self.bucket,
            prefix = self.prefix,
        )
    }
}

#[derive(Debug, Default)]
pub struct ProbeReport {
    pub reachable: bool,
    pub conditional_writes: bool,
    pub why: String,
}

impl ProbeReport {
    pub fn describe(&self, options: &StorageOptions) -> String {
        let mut out = format!(
            "  storage: {}/{}/{}\n",
            options.endpoint, options.bucket, options.prefix
        );
        out.push_str(if !self.reachable {
            "  bucket: unreachable\n"
        } else if options.single_writer {
            "  bucket: single-writer, asserted; the index is written unconditionally\n"
        } else if self.conditional_writes {
            "  bucket: conditional writes work; the index is safe against a racing writer\n"
        } else {
            "  bucket: no conditional writes\n"
        });
        out
    }
}

fn etag(response: &Response) -> Option<String> {
    response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .filter(|v| !v.is_empty())
}

async fn problem(method: &str, key: &str, response: Response) -> BlobError {
    let status = response.status().as_u16();
    let body = response
        .bytes()
        .await
        .map(|b| b.to_vec())
        .unwrap_or_default();
    BlobError::Other(format!(
        "{method} {key} failed ({status}): {}",
        truncate(&String::from_utf8_lossy(&body[..body.len().min(2048)]), 300)
    ))
}

#[async_trait]
impl BlobStore for S3Store {
    async fn get(&self, key: &str) -> BlobResult<Vec<u8>> {
        Ok(self.get_versioned(key).await?.0)
    }

    async fn get_versioned(&self, key: &str) -> BlobResult<(Vec<u8>, BlobVersion)> {
        let response = self
            .send(Method::GET, &self.url(key), &[], Vec::new())
            .await?;
        let status = response.status().as_u16();
        if status == 404 {
            return Err(BlobError::NotFound);
        }
        if status != 200 {
            return Err(problem("GET", key, response).await);
        }
        let version = etag(&response).unwrap_or_default();
        let body = response
            .bytes()
            .await
            .map_err(|err| BlobError::Other(err.to_string()))?;
        Ok((body.to_vec(), version))
    }

    async fn put(&self, key: &str, body: Vec<u8>, content_type: &str) -> BlobResult<()> {
        self.write(key, body, content_type, &[]).await.map(|_| ())
    }

    async fn delete(&self, keys: &[String]) -> BlobResult<()> {
        for key in keys {
            let response = self
                .send(Method::DELETE, &self.url(key), &[], Vec::new())
                .await?;
            let status = response.status().as_u16();
            // An object that is not there is the outcome asked for.
            if status >= 300 && status != 404 {
                return Err(problem("DELETE", key, response).await);
            }
        }
        Ok(())
    }

    async fn list(&self, prefix: &str) -> BlobResult<Vec<BlobInfo>> {
        let mut found = Vec::new();
        let mut token = String::new();
        loop {
            let mut query = vec![
                ("list-type".to_string(), "2".to_string()),
                ("prefix".to_string(), self.scoped(prefix)),
            ];
            if !token.is_empty() {
                query.push(("continuation-token".to_string(), token.clone()));
            }
            let address = format!(
                "{}/{}?{}",
                self.endpoint,
                self.bucket,
                canonical_query(&query)
            );
            let response = self.send(Method::GET, &address, &[], Vec::new()).await?;
            let status = response.status().as_u16();
            let body = response
                .bytes()
                .await
                .map_err(|err| BlobError::Other(err.to_string()))?;
            let body = String::from_utf8_lossy(&body).to_string();
            if status != 200 {
                return Err(BlobError::Other(format!(
                    "listing {prefix} failed ({status}): {}",
                    truncate(&body, 200)
                )));
            }
            let page = parse_listing(&body);
            for (key, size, version) in page.contents {
                // Keys come back scoped; callers speak in unscoped keys.
                let key = key.strip_prefix(&self.prefix).unwrap_or(&key).to_string();
                found.push(BlobInfo { key, size, version });
            }
            if !page.truncated || page.next_token.is_empty() {
                break;
            }
            token = page.next_token;
        }
        found.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(found)
    }

    async fn swap(&self, key: &str, body: Vec<u8>, expect: &str) -> BlobResult<BlobVersion> {
        // With one writer asserted, the caller's mutex is the coordination and
        // the bucket is asked for nothing it may not support.
        if self.single_writer {
            return self.write(key, body, "application/json", &[]).await;
        }
        let condition = if expect.is_empty() {
            ("If-None-Match", "*".to_string())
        } else {
            ("If-Match", expect.to_string())
        };
        self.write(key, body, "application/json", &[condition])
            .await
    }

    fn describe(&self) -> String {
        format!("{}/{}/{}", self.endpoint, self.bucket, self.prefix)
    }

    fn presigned_get(&self, key: &str, lifetime_seconds: u64) -> Option<String> {
        self.presign_get(key, lifetime_seconds)
    }
}

/// ListObjectsV2's answer, in the fields this needs. Read with string
/// searches rather than an XML parser: the document is flat and the four
/// elements are unambiguous.
#[derive(Default)]
struct Listing {
    truncated: bool,
    next_token: String,
    contents: Vec<(String, i64, String)>,
}

fn parse_listing(body: &str) -> Listing {
    let mut listing = Listing {
        truncated: element(body, "IsTruncated").as_deref() == Some("true"),
        next_token: element(body, "NextContinuationToken").unwrap_or_default(),
        contents: Vec::new(),
    };
    let mut rest = body;
    while let Some(start) = rest.find("<Contents>") {
        let after = &rest[start..];
        let Some(end) = after.find("</Contents>") else {
            break;
        };
        let block = &after[..end];
        let key = unescape_xml(&element(block, "Key").unwrap_or_default());
        let size = element(block, "Size")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let version = unescape_xml(&element(block, "ETag").unwrap_or_default());
        listing.contents.push((key, size, version));
        rest = &after[end..];
    }
    listing
}

fn element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

fn unescape_xml(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/* ------------------------------------------------------------- signing */

/// The four-HMAC derivation, with the service named rather than assumed --
/// which is what lets a test check it against the vector AWS publishes, and
/// that vector is for a different service.
pub fn signing_key_for(secret: &str, day: &str, region: &str, service: &str) -> Vec<u8> {
    let key = hmac_sha256(format!("AWS4{secret}").as_bytes(), day.as_bytes());
    let key = hmac_sha256(&key, region.as_bytes());
    let key = hmac_sha256(&key, service.as_bytes());
    hmac_sha256(&key, b"aws4_request")
}

pub fn hmac_sha256(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

/// RFC 3986 unreserved characters stay; everything else is percent-encoded,
/// a space as %20 rather than a plus.
const RESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

pub fn escape(value: &str) -> String {
    utf8_percent_encode(value, RESERVED).to_string()
}

/// Encodes each segment of a path, keeping the slashes between them, and
/// returns it with a leading slash.
pub fn escape_path(path: &str) -> String {
    let segments: Vec<String> = path
        .trim_start_matches('/')
        .split('/')
        .map(escape)
        .collect();
    format!("/{}", segments.join("/"))
}

pub fn canonical_query(query: &[(String, String)]) -> String {
    let mut sorted: Vec<&(String, String)> = query.iter().collect();
    sorted.sort();
    sorted
        .into_iter()
        .map(|(key, value)| format!("{}={}", escape(key), escape(value)))
        .collect::<Vec<_>>()
        .join("&")
}
