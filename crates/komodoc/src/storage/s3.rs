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

use crate::http::{client, truncate};
use crate::storage::blob::{version_of, BlobError, BlobInfo, BlobResult, BlobStore, BlobVersion};
use crate::storage::StorageOptions;
use crate::util::{amz_stamps, now_unix};

pub struct S3Store {
    endpoint: String, // https://host, without the bucket
    bucket: String,
    region: String,
    prefix: String, // every key komodoc writes lives under this
    access_key: String,
    secret_key: String,
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
    ///
    /// `pub(crate)` rather than private so a test can build the exact URL a
    /// request goes out as and check it against what gets signed -- see
    /// `canonical_path` below and `tests/review_misc.rs`.
    pub(crate) fn url(&self, key: &str) -> String {
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
            // `target` was built by `url()`, which already ran the scoped key
            // through `escape_path` once. `url::Url` keeps a percent-escape it
            // is given rather than re-encoding it -- our escape set is a
            // subset of characters `url` would encode anyway -- so
            // `parsed.path()` is exactly that one encoding, not the raw key.
            // Encoding it again here is what used to turn a space into
            // `%2520` in the canonical request while the wire request only
            // ever sent `%20`; using the same already-encoded path for both
            // is what keeps the signature over what was actually sent.
            canonical_path(&parsed),
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
            let body = String::from_utf8(body.to_vec())
                .map_err(|err| BlobError::Other(format!("listing {prefix} is not UTF-8: {err}")))?;
            if status != 200 {
                return Err(BlobError::Other(format!(
                    "listing {prefix} failed ({status}): {}",
                    truncate(&body, 200)
                )));
            }
            let page = parse_listing(&body)?;
            for (key, size, version) in page.contents {
                // Keys come back scoped; callers speak in unscoped keys.
                let key = key
                    .strip_prefix(&self.prefix)
                    .ok_or_else(|| BlobError::Other("listing returned an unscoped key".into()))?
                    .to_string();
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

    async fn list_page(
        &self,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
    ) -> BlobResult<Vec<BlobInfo>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut found = Vec::with_capacity(limit.min(1000));
        let mut token: Option<String> = None;
        let mut first_request = true;
        let mut seen_tokens = std::collections::HashSet::new();
        loop {
            let remaining = limit.saturating_sub(found.len());
            if remaining == 0 {
                break;
            }
            let mut query = vec![
                ("list-type".to_string(), "2".to_string()),
                // S3 ListObjectsV2 caps this parameter at 1000.
                ("max-keys".to_string(), remaining.min(1000).to_string()),
                ("prefix".to_string(), self.scoped(prefix)),
            ];
            if first_request {
                if let Some(after) = after {
                    query.push(("start-after".to_string(), self.scoped(after)));
                }
                first_request = false;
            }
            if let Some(token) = &token {
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
            let body = String::from_utf8(body.to_vec())
                .map_err(|err| BlobError::Other(format!("listing {prefix} is not UTF-8: {err}")))?;
            if status != 200 {
                return Err(BlobError::Other(format!(
                    "listing {prefix} failed ({status}): {}",
                    truncate(&body, 200)
                )));
            }
            let page = parse_listing(&body)?;
            for (key, size, version) in page.contents {
                let key = key
                    .strip_prefix(&self.prefix)
                    .ok_or_else(|| BlobError::Other("listing returned an unscoped key".into()))?;
                let item = BlobInfo {
                    key: key.to_owned(),
                    size,
                    version,
                };
                if item.key.starts_with(prefix)
                    && after.is_none_or(|cursor| item.key.as_str() > cursor)
                {
                    found.push(item);
                    if found.len() == limit {
                        break;
                    }
                }
            }
            if found.len() == limit || !page.truncated {
                break;
            }
            let next = page.next_token;
            if !seen_tokens.insert(next.clone()) {
                return Err(BlobError::Other(
                    "S3 listing repeated a continuation token".into(),
                ));
            }
            token = Some(next);
        }
        found.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(found)
    }

    async fn swap(&self, key: &str, body: Vec<u8>, expect: &str) -> BlobResult<BlobVersion> {
        // Even a deployment configured for one writer must retain create-only
        // semantics: backup publication uses an empty expectation to make a
        // recovery point immutable if two jobs are accidentally launched.
        if self.single_writer && !expect.is_empty() {
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

fn parse_listing(body: &str) -> BlobResult<Listing> {
    if !body.contains("<ListBucketResult") || !body.contains("</ListBucketResult>") {
        return Err(BlobError::Other("malformed S3 listing root".into()));
    }
    let truncated = match element(body, "IsTruncated")
        .ok_or_else(|| BlobError::Other("S3 listing omitted IsTruncated".into()))?
        .trim()
    {
        "true" => true,
        "false" => false,
        value => {
            return Err(BlobError::Other(format!(
                "invalid S3 IsTruncated value {value:?}"
            )))
        }
    };
    let next_token = element(body, "NextContinuationToken")
        .map(|value| unescape_xml(&value))
        .transpose()?
        .unwrap_or_default();
    if truncated && next_token.is_empty() {
        return Err(BlobError::Other(
            "truncated S3 listing omitted continuation token".into(),
        ));
    }
    let mut listing = Listing {
        truncated,
        next_token,
        contents: Vec::new(),
    };
    let mut rest = body;
    while let Some(start) = rest.find("<Contents>") {
        let after = &rest[start..];
        let end = after
            .find("</Contents>")
            .ok_or_else(|| BlobError::Other("malformed S3 Contents element".into()))?;
        let block = &after[..end];
        let key = unescape_xml(
            &element(block, "Key")
                .ok_or_else(|| BlobError::Other("S3 object omitted Key".into()))?,
        )?;
        if key.is_empty() {
            return Err(BlobError::Other("S3 object has an empty Key".into()));
        }
        let size_text = element(block, "Size")
            .ok_or_else(|| BlobError::Other("S3 object omitted Size".into()))?;
        let size = size_text
            .parse::<i64>()
            .map_err(|_| BlobError::Other(format!("invalid S3 object size {size_text:?}")))?;
        if size < 0 {
            return Err(BlobError::Other("S3 object has a negative Size".into()));
        }
        let version = element(block, "ETag")
            .map(|value| unescape_xml(&value))
            .transpose()?
            .unwrap_or_default();
        listing.contents.push((key, size, version));
        rest = &after[end + "</Contents>".len()..];
    }
    Ok(listing)
}

fn element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

fn unescape_xml(text: &str) -> BlobResult<String> {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('&') {
        output.push_str(&rest[..start]);
        let entity_end = rest[start..]
            .find(';')
            .ok_or_else(|| BlobError::Other("malformed XML entity in S3 listing".into()))?
            + start;
        let entity = &rest[start..=entity_end];
        output.push_str(match entity {
            "&quot;" => "\"",
            "&lt;" => "<",
            "&gt;" => ">",
            "&apos;" => "'",
            "&amp;" => "&",
            _ => {
                return Err(BlobError::Other(format!(
                    "unknown XML entity in S3 listing: {entity}"
                )))
            }
        });
        rest = &rest[entity_end + 1..];
    }
    output.push_str(rest);
    Ok(output)
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

/// The path SigV4 signs, for a request whose URL was built by `url()`.
///
/// `url()` runs the scoped key through `escape_path` once. `url::Url`
/// preserves an escape it is given rather than re-encoding it -- `RESERVED`
/// only leaves unreserved characters unescaped, a strict subset of what
/// `url` itself would leave alone -- so `parsed.path()` here *is* that one
/// encoding, byte for byte, not the raw key. Escaping it again is exactly
/// the bug this function exists to not repeat: it would turn the `%20` the
/// request actually carries into `%2520` in the string that gets signed,
/// so the signature would cover a path nobody sent. `send` routes through
/// this one function so there is a single place that can make that mistake.
pub(crate) fn canonical_path(parsed: &url::Url) -> String {
    parsed.path().to_string()
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::response::Response;
    use axum::routing::any;
    use axum::Router;

    #[test]
    fn listing_decodes_escaped_keys_and_continuation_tokens() {
        let listing = parse_listing(
            "<ListBucketResult>\
             <IsTruncated>true</IsTruncated>\
             <NextContinuationToken>next&amp;page</NextContinuationToken>\
             <Contents><Key>objects/a&amp;b</Key><Size>7</Size><ETag>&quot;tag&quot;</ETag></Contents>\
             </ListBucketResult>",
        )
        .expect("valid listing");
        assert!(listing.truncated);
        assert_eq!(listing.next_token, "next&page");
        assert_eq!(
            listing.contents,
            vec![("objects/a&b".into(), 7, "\"tag\"".into())]
        );
    }

    #[test]
    fn malformed_or_truncated_listing_fails_closed() {
        for body in [
            "<ListBucketResult><IsTruncated>true</IsTruncated></ListBucketResult>",
            "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>x</Key><Size>bad</Size></Contents></ListBucketResult>",
            "<ListBucketResult><IsTruncated>false</IsTruncated><Contents><Key>x&broken;</Key><Size>1</Size></Contents></ListBucketResult>",
        ] {
            assert!(parse_listing(body).is_err(), "accepted malformed listing: {body}");
        }
    }

    #[tokio::test]
    async fn list_page_consumes_s3_pages_past_the_1000_object_limit() {
        async fn bucket(request: Request<Body>) -> Response<Body> {
            let query = request.uri().query().unwrap_or_default();
            let continuation = url::form_urlencoded::parse(query.as_bytes())
                .find(|(key, _)| key == "continuation-token")
                .map(|(_, value)| value.into_owned());
            let mut body = String::from("<ListBucketResult><IsTruncated>");
            if continuation.is_none() {
                body.push_str(
                    "true</IsTruncated><NextContinuationToken>page&amp;2</NextContinuationToken>",
                );
                for index in 0..1000 {
                    body.push_str(&format!(
                        "<Contents><Key>scope/objects/{index:04}</Key><Size>1</Size></Contents>"
                    ));
                }
            } else {
                body.push_str("false</IsTruncated>");
                body.push_str("<Contents><Key>scope/objects/1000</Key><Size>1</Size></Contents>");
            }
            body.push_str("</ListBucketResult>");
            Response::new(Body::from(body))
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            axum::serve(listener, Router::new().fallback(any(bucket)))
                .await
                .expect("server");
        });
        let options = StorageOptions {
            endpoint: format!("http://{address}"),
            bucket: "bucket".into(),
            prefix: "scope/".into(),
            access_key: "key".into(),
            secret_key: "secret".into(),
            ..StorageOptions::default()
        };
        let store = S3Store::new(&options);
        let page = store
            .list_page("objects/", None, 1001)
            .await
            .expect("bounded listing");
        assert_eq!(page.len(), 1001);
        assert_eq!(
            page.first().map(|item| item.key.as_str()),
            Some("objects/0000")
        );
        assert_eq!(
            page.last().map(|item| item.key.as_str()),
            Some("objects/1000")
        );
    }
}
