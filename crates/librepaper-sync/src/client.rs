//! The calls sync makes of a deployment, over the shared HTTP client.

use std::collections::BTreeMap;
use std::time::Duration;

use librepaper_base::digest::Digest;
use librepaper_base::http::{decode, detail_of, post_json_as, send, truncate, Credentials};
use librepaper_base::site::SiteUrl;
use librepaper_base::text::LineEnding;
use librepaper_base::util::{token, Encoding};
use librepaper_document::document::projection::{Entry, HeadSnapshot, LabelRow, VersionSnapshot};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

use crate::files::{FileRecord, Kind};
use crate::rules::Rules;
use crate::Error;

/// A request that answers from memory or a small row.
const QUICK: Duration = Duration::from_secs(60);
/// A request that carries or returns file contents.
const BULK: Duration = Duration::from_secs(300);
/// How many times a request that got no answer is sent again, unchanged.
const RETRIES: u32 = 2;

/// One deployment, and who is asking.
pub struct Client {
    site: Url,
    credentials: Credentials,
}

/// Names the site only. The credential never reaches a log.
impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client").field("site", &self.site).finish()
    }
}

/// What a person called the version, or a stand-in when nobody did.
pub(crate) fn version_name(row: &LabelRow) -> String {
    match row.label.as_deref().map(str::trim) {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => format!("LibrePaper version {}", row.sequence),
    }
}

/// One page of a project's version list.
#[derive(Debug)]
pub(crate) struct LabelPage {
    /// Newest first.
    pub labels: Vec<LabelRow>,
    /// Where the next page starts.
    pub next: Option<i64>,
    /// The identity of the project's accepted state when the page was read.
    /// Older servers send none.
    pub head: Option<String>,
}

/// The files of a project at one moment, with pending suggestions undone.
#[derive(Clone, Debug, Default)]
pub(crate) struct Snapshot {
    pub files: BTreeMap<String, FileRecord>,
    /// The LF text of each text file.
    pub texts: BTreeMap<String, String>,
}

impl Snapshot {
    /// Reads a file listing and its texts as the server sends them.
    fn from_wire(
        listing: BTreeMap<String, Entry>,
        bodies: BTreeMap<String, String>,
    ) -> Result<Snapshot, Error> {
        let mut snapshot = Snapshot::default();
        for (path, entry) in listing {
            match entry.kind.as_str() {
                "text" => {
                    let body = bodies.get(&path).ok_or_else(|| {
                        Error::Invalid(format!("the server sent no text for {path}"))
                    })?;
                    let body = LineEnding::normalise(body).into_owned();
                    snapshot.files.insert(
                        path.clone(),
                        FileRecord {
                            kind: Kind::Text,
                            sha: Digest::of(body.as_bytes()).to_hex(),
                        },
                    );
                    snapshot.texts.insert(path, body);
                }
                "asset" => {
                    if entry.digest.len() != 64 {
                        return Err(Error::Invalid(format!(
                            "the server sent no digest for {path}"
                        )));
                    }
                    snapshot.files.insert(
                        path,
                        FileRecord {
                            kind: Kind::Asset,
                            sha: entry.digest,
                        },
                    );
                }
                _ => {}
            }
        }
        Ok(snapshot)
    }

    /// Drops the paths this deployment's rules would not sync.
    pub(crate) fn retain_synced(&mut self, rules: &Rules) {
        self.files.retain(|path, _| rules.check(path).is_ok());
        let files = &self.files;
        self.texts.retain(|path, _| files.contains_key(path));
    }
}

impl Client {
    pub fn new(site: Url, credentials: Credentials) -> Client {
        Client { site, credentials }
    }

    /// A fingerprint of the sign-in that is safe to store: it names the
    /// credential without revealing it.
    pub(crate) fn actor(&self) -> String {
        let secret = format!("{}\n{}", self.credentials.token, self.credentials.key);
        let digest = Digest::of(secret.as_bytes()).to_hex();
        format!("sha256:{digest}")
    }

    pub(crate) fn site(&self) -> &Url {
        &self.site
    }

    fn base(&self) -> Result<SiteUrl, Error> {
        SiteUrl::from_origin_of(&self.site).map_err(Error::Invalid)
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        extra: &[(&str, &str)],
        body: Option<Vec<u8>>,
        timeout: Duration,
    ) -> Result<(u16, Vec<u8>), Error> {
        let owned = self.credentials.headers();
        let mut headers: Vec<(&str, &str)> = owned
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        headers.extend_from_slice(extra);
        send(
            method,
            &self.base()?.join(path),
            &headers,
            body,
            timeout,
        )
        .await
        .map_err(Error::Network)
    }

    /// A GET that must answer with JSON.
    async fn get_json(&self, path: &str) -> Result<Value, Error> {
        let (status, raw) = self.call(Method::GET, path, &[], None, QUICK).await?;
        let value = decode(&raw);
        if (200..300).contains(&status) {
            Ok(value)
        } else {
            Err(refusal(status, &value))
        }
    }

    /// What the deployment accepts. The configuration is public, and an agent
    /// token is admitted only on its document's routes, so this call carries
    /// no credential.
    pub async fn rules(&self) -> Result<Rules, Error> {
        let anonymous = Credentials::default().headers();
        let headers: Vec<(&str, &str)> = anonymous
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let (status, raw) = send(
            Method::GET,
            &self.base()?.join("/api/config"),
            &headers,
            None,
            QUICK,
        )
        .await
        .map_err(Error::Network)?;
        let value = decode(&raw);
        if !(200..300).contains(&status) {
            return Err(refusal(status, &value));
        }
        serde_json::from_value(value).map_err(|error| {
            Error::Invalid(format!("the deployment's rules are unreadable: {error}"))
        })
    }

    /// One page of a project's versions, newest first, where the next page
    /// starts, and the project's head identity. `named` leaves out the labels
    /// nobody named.
    pub(crate) async fn labels(
        &self,
        slug: &str,
        named: bool,
        limit: u32,
        after: Option<i64>,
    ) -> Result<LabelPage, Error> {
        let mut path = format!("/api/documents/{slug}/history?limit={limit}");
        if named {
            path.push_str("&named=1");
        }
        if let Some(after) = after {
            path.push_str(&format!("&after={after}"));
        }
        let value = self.get_json(&path).await?;
        let labels: Vec<LabelRow> = serde_json::from_value(
            value.get("labels").cloned().unwrap_or(Value::Null),
        )
        .map_err(|error| Error::Invalid(format!("the version list is unreadable: {error}")))?;
        let next = value.get("next_cursor").and_then(Value::as_i64);
        let head = value
            .get("head")
            .and_then(Value::as_str)
            .map(str::to_string);
        Ok(LabelPage { labels, next, head })
    }

    /// The files of one version.
    pub(crate) async fn version(&self, slug: &str, sha: &str) -> Result<Snapshot, Error> {
        let path = format!("/api/documents/{slug}/history/{sha}?unsuggested=1");
        let (status, raw) = self.call(Method::GET, &path, &[], None, BULK).await?;
        let value = decode(&raw);
        if status == 410 {
            return Err(Error::Trimmed);
        }
        if status != 200 {
            return Err(refusal(status, &value));
        }
        let version: VersionSnapshot = serde_json::from_value(value)
            .map_err(|error| Error::Invalid(format!("the version {sha} is unreadable: {error}")))?;
        version
            .verify()
            .map_err(|error| Error::Invalid(format!("the version {sha} is damaged: {error}")))?;
        Snapshot::from_wire(version.files, version.texts)
    }

    /// The files of the project as it stands.
    pub(crate) async fn head(&self, slug: &str) -> Result<Snapshot, Error> {
        let path = format!("/api/documents/{slug}/project?unsuggested=1");
        let (status, raw) = self.call(Method::GET, &path, &[], None, BULK).await?;
        let value = decode(&raw);
        if status != 200 {
            return Err(refusal(status, &value));
        }
        let head: HeadSnapshot = serde_json::from_value(value)
            .map_err(|error| Error::Invalid(format!("the project is unreadable: {error}")))?;
        head.verify()
            .map_err(|error| Error::Invalid(format!("the project is damaged: {error}")))?;
        Snapshot::from_wire(head.tree.files, head.texts)
    }

    /// An asset's bytes, checked against its digest.
    pub(crate) async fn asset(&self, slug: &str, digest: &str) -> Result<Vec<u8>, Error> {
        let path = format!("/api/documents/{slug}/assets/{digest}");
        let (status, raw) = self.call(Method::GET, &path, &[], None, BULK).await?;
        if status != 200 {
            return Err(refusal(status, &decode(&raw)));
        }
        if Digest::of(&raw).to_hex() != digest {
            return Err(Error::Invalid(format!(
                "the asset {digest} arrived damaged"
            )));
        }
        Ok(raw)
    }

    /// Whether the project already holds an asset, so it is not sent again.
    pub(crate) async fn has_asset(&self, slug: &str, digest: &str) -> Result<bool, Error> {
        let path = format!("/api/documents/{slug}/assets/{digest}");
        let (status, raw) = self.call(Method::HEAD, &path, &[], None, QUICK).await?;
        match status {
            200 => Ok(true),
            404 => Ok(false),
            _ => Err(refusal(status, &decode(&raw))),
        }
    }

    pub(crate) async fn upload_asset(
        &self,
        slug: &str,
        bytes: Vec<u8>,
        digest: &str,
    ) -> Result<(), Error> {
        let path = format!("/api/documents/{slug}/assets");
        let headers = [("content-type", "application/octet-stream")];
        let (status, raw) = self
            .call(Method::PUT, &path, &headers, Some(bytes), BULK)
            .await?;
        let value = decode(&raw);
        if status != 200 {
            return Err(refusal(status, &value));
        }
        if value.get("sha").and_then(Value::as_str) != Some(digest) {
            return Err(Error::Invalid(
                "the server stored a different file than the one sent".into(),
            ));
        }
        Ok(())
    }

    /// Sends a source change. The same body, and so the same operation key,
    /// goes out again when no answer came back, so a change that did land
    /// answers with its first receipt instead of landing twice.
    pub(crate) async fn source_change(
        &self,
        slug: &str,
        body: &Value,
    ) -> Result<(u16, Value), Error> {
        let target = self.base()?.join(&format!("/api/documents/{slug}/source-change"));
        let mut attempt = 0;
        loop {
            match post_json_as(&target, body, &self.credentials, BULK).await {
                Ok((503, reply)) if retryable(&reply) && attempt < RETRIES => {}
                Ok(reply) => return Ok(reply),
                Err(_) if attempt < RETRIES => {}
                Err(error) => return Err(Error::Network(error)),
            }
            attempt += 1;
            tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
        }
    }

    /// Creates a project from files and answers its slug.
    pub(crate) async fn publish(
        &self,
        title: &str,
        files: &[(String, Vec<u8>)],
    ) -> Result<String, Error> {
        let boundary = format!("librepaper-{}", token::<16>(Encoding::Hex));
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\n{title}\r\n"
            )
            .as_bytes(),
        );
        for (path, bytes) in files {
            // Quotes and line breaks in a name are escaped as a browser does.
            let name = path
                .replace('"', "%22")
                .replace('\r', "%0D")
                .replace('\n', "%0A");
            body.extend_from_slice(
                format!(
                    "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
                )
                .as_bytes(),
            );
            body.extend_from_slice(bytes);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let kind = format!("multipart/form-data; boundary={boundary}");
        let (status, raw) = self
            .call(
                Method::POST,
                "/api/documents",
                &[("content-type", kind.as_str())],
                Some(body),
                BULK,
            )
            .await?;
        let value = decode(&raw);
        if !(200..300).contains(&status) {
            return Err(refusal(status, &value));
        }
        value
            .get("slug")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| Error::Invalid("the server did not name the new project".into()))
    }
}

fn retryable(reply: &Value) -> bool {
    reply.get("retryable").and_then(Value::as_bool) == Some(true)
}

/// The error for a reply that was not a success.
pub(crate) fn refusal(status: u16, reply: &Value) -> Error {
    let mut detail = detail_of(reply);
    if detail.trim().is_empty() || detail == "null" {
        detail = format!("the server answered {status}");
    }
    Error::Http { status, detail }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(kind: &str, digest: &str) -> Entry {
        Entry {
            kind: kind.into(),
            id: String::new(),
            digest: digest.into(),
            bytes: 0,
        }
    }

    fn bodies(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(path, body)| (path.to_string(), body.to_string()))
            .collect()
    }

    #[test]
    fn a_version_listing_becomes_records_and_lf_texts() {
        let files = BTreeMap::from([
            ("main.tex".to_string(), entry("text", "ignored")),
            ("fig/a.png".to_string(), entry("asset", &"a".repeat(64))),
            ("odd".to_string(), entry("folder", "")),
        ]);
        let snapshot =
            Snapshot::from_wire(files, bodies(&[("main.tex", "one\r\ntwo\r\n")])).unwrap();
        assert_eq!(snapshot.files.len(), 2);
        assert_eq!(snapshot.texts["main.tex"], "one\ntwo\n");
        assert_eq!(
            snapshot.files["main.tex"].sha,
            Digest::of(b"one\ntwo\n").to_hex()
        );
        assert_eq!(snapshot.files["fig/a.png"].sha, "a".repeat(64));
        assert_eq!(snapshot.files["fig/a.png"].kind, Kind::Asset);
    }

    #[test]
    fn a_text_without_a_body_is_an_error() {
        let files = BTreeMap::from([("main.tex".to_string(), entry("text", "x"))]);
        assert!(Snapshot::from_wire(files, BTreeMap::new()).is_err());
    }

    #[test]
    fn paths_the_rules_refuse_are_dropped() {
        let paths = ["main.tex", ".git/config.txt", "notes.docx", "C:/outside/paper.tex"];
        let files = paths
            .iter()
            .map(|path| (path.to_string(), entry("text", "x")))
            .collect();
        let texts = bodies(&[
            ("main.tex", "a\n"),
            (".git/config.txt", "b\n"),
            ("notes.docx", "c\n"),
            ("C:/outside/paper.tex", "d\n"),
        ]);
        let mut snapshot = Snapshot::from_wire(files, texts).unwrap();
        snapshot.retain_synced(&Rules::default());
        assert_eq!(snapshot.files.keys().collect::<Vec<_>>(), ["main.tex"]);
        assert_eq!(snapshot.texts.len(), 1);
    }

    #[test]
    fn an_empty_error_body_still_says_what_happened() {
        let error = refusal(502, &decode(b""));
        assert!(error.to_string().contains("502"), "{error}");
        let error = refusal(409, &json!({"error": "files changed"}));
        assert_eq!(error.to_string(), "files changed (409)");
    }

    #[test]
    fn an_unnamed_version_still_has_a_name() {
        let label = |name: Option<&str>| LabelRow {
            sha: "s".into(),
            sequence: 7,
            at: String::new(),
            by: String::new(),
            label: name.map(str::to_string),
            reason: String::new(),
            tree_sha: None,
            frontier: String::new(),
            archive_status: String::new(),
        };
        assert_eq!(version_name(&label(Some(" Draft 2 "))), "Draft 2");
        assert_eq!(version_name(&label(None)), "LibrePaper version 7");
        assert_eq!(version_name(&label(Some("  "))), "LibrePaper version 7");
    }
}
