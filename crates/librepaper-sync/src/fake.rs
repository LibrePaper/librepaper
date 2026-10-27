//! A project server that answers the read calls sync makes, from memory, and
//! remembers what it was asked.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use librepaper_base::digest::Digest;
use librepaper_base::http::Credentials;
use librepaper_document::document::projection::{
    Entry, HeadSnapshot, Projection, VersionSnapshot,
};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

use crate::client::Client;

/// The text of each file, by path.
pub(crate) type Files = BTreeMap<String, String>;

/// Builds a file listing from `(path, text)` pairs.
pub(crate) fn files(entries: &[(&str, &str)]) -> Files {
    entries
        .iter()
        .map(|(path, text)| (path.to_string(), text.to_string()))
        .collect()
}

/// What the fake deployment does with one source change.
pub(crate) enum Answer {
    Reply(u16, Value),
    /// Reads the request and closes the connection without answering.
    Drop,
}

/// A source change accepted as a new named version.
pub(crate) fn accepted(sha: &str, sequence: i64) -> Answer {
    Answer::Reply(200, json!({"label": {"sha": sha, "sequence": sequence}}))
}

#[derive(Default)]
struct Data {
    /// Oldest first: sequence, files.
    versions: Vec<(i64, Files)>,
    current: Files,
    head: Option<String>,
    requests: Vec<String>,
    /// What the project holds once a source change lands.
    after_push: Files,
    /// Answers to source changes, used before the default acceptance.
    script: VecDeque<Answer>,
    /// The body of every source change that came in.
    posted: Vec<Value>,
}

pub(crate) struct Fake {
    site: Url,
    data: Arc<Mutex<Data>>,
}

impl Fake {
    pub(crate) async fn start() -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let site = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let data = Arc::new(Mutex::new(Data::default()));
        let shared = data.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(answer(stream, shared.clone()));
            }
        });
        Fake { site, data }
    }

    pub(crate) fn client(&self) -> Client {
        self.client_as("lp_token")
    }

    /// A client signed in with another token.
    pub(crate) fn client_as(&self, token: &str) -> Client {
        Client::new(self.site.clone(), Credentials::token(token))
    }

    /// The link a directory is linked with.
    pub(crate) fn link(&self) -> crate::link::Link {
        crate::link::Link::at(&self.site, "paper".to_string())
    }

    /// Names a version with these files. The project as it stands is left as
    /// it was.
    pub(crate) fn name(&self, sequence: i64, files: Files) {
        self.data.lock().unwrap().versions.push((sequence, files));
    }

    /// The project as it stands.
    pub(crate) fn set_current(&self, files: Files) {
        self.data.lock().unwrap().current = files;
    }

    /// What the project holds once a source change is accepted. The change
    /// is recorded as the next named version, and the head identity moves.
    pub(crate) fn set_after_push(&self, files: Files) {
        self.data.lock().unwrap().after_push = files;
    }

    /// Queues answers to the next source changes. Once they are used up, a
    /// source change is accepted as `set_after_push` says.
    pub(crate) fn script(&self, answers: Vec<Answer>) {
        self.data.lock().unwrap().script.extend(answers);
    }

    /// The body of every source change that came in, oldest first.
    pub(crate) fn posted(&self) -> Vec<Value> {
        self.data.lock().unwrap().posted.clone()
    }

    /// The head identity the history page carries; `None` is an older server.
    pub(crate) fn set_head(&self, head: Option<&str>) {
        self.data.lock().unwrap().head = head.map(str::to_string);
    }

    /// How many requests whose path ends with `suffix` came in.
    pub(crate) fn count(&self, suffix: &str) -> usize {
        self.data
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|path| path.split('?').next().unwrap_or("").ends_with(suffix))
            .count()
    }
}

fn tree(files: &Files) -> Projection {
    let files = files
        .iter()
        .map(|(path, text)| {
            let entry = Entry {
                kind: "text".into(),
                id: String::new(),
                digest: Digest::of(text.as_bytes()).to_hex(),
                bytes: text.len() as u64,
            };
            (path.clone(), entry)
        })
        .collect();
    Projection {
        files,
        diagnostics: Vec::new(),
    }
}

fn query<'a>(target: &'a str, key: &str) -> Option<&'a str> {
    let query = target.split_once('?')?.1;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='))
}

/// The answer to one request, or `None` to close the connection without one.
fn reply(data: &mut Data, method: &str, target: &str, sent: &[u8]) -> Option<(u16, Value)> {
    let path = target.split('?').next().unwrap_or("");
    if method == "POST" && path.ends_with("/source-change") {
        data.posted
            .push(serde_json::from_slice(sent).unwrap_or(Value::Null));
        if let Some(answer) = data.script.pop_front() {
            return match answer {
                Answer::Reply(status, body) => Some((status, body)),
                Answer::Drop => None,
            };
        }
        let sequence = data.versions.last().map_or(1, |(sequence, _)| sequence + 1);
        data.versions.push((sequence, data.after_push.clone()));
        data.current = data.after_push.clone();
        if let Some(head) = &mut data.head {
            head.push('+');
        }
        let sha = format!("sha-{sequence}");
        return Some((200, json!({"label": {"sha": sha, "sequence": sequence}})));
    }
    if path == "/api/config" {
        return Some((200, json!({})));
    }
    if path.ends_with("/history") {
        let limit = query(target, "limit")
            .and_then(|limit| limit.parse::<usize>().ok())
            .unwrap_or(usize::MAX);
        let after = query(target, "after").and_then(|after| after.parse::<i64>().ok());
        let labels: Vec<Value> = data
            .versions
            .iter()
            .rev()
            .filter(|(sequence, _)| after.is_none_or(|after| *sequence < after))
            .take(limit)
            .map(|(sequence, _)| {
                json!({
                    "sha": format!("sha-{sequence}"),
                    "sequence": sequence,
                    "by": "Bob",
                    "label": format!("Version {sequence}"),
                })
            })
            .collect();
        let mut body = json!({"labels": labels, "next_cursor": null});
        if let Some(head) = &data.head {
            body["head"] = json!(head);
        }
        return Some((200, body));
    }
    if let Some((_, sha)) = path.split_once("/history/") {
        let found = data
            .versions
            .iter()
            .find(|(sequence, _)| format!("sha-{sequence}") == sha);
        return match found {
            Some((_, files)) => {
                let tree = tree(files);
                let version = VersionSnapshot {
                    sha: sha.to_string(),
                    tree_sha: tree.digest().to_hex(),
                    storage_id: String::new(),
                    at: String::new(),
                    by: String::new(),
                    label: None,
                    reason: String::new(),
                    source_format: String::new(),
                    files: tree.files,
                    texts: files.clone(),
                };
                Some((200, serde_json::to_value(version).expect("serializes")))
            }
            None => Some((404, json!({"error": "no such version"}))),
        };
    }
    if path.ends_with("/project") {
        let tree = tree(&data.current);
        let head = HeadSnapshot {
            project_digest: tree.digest().to_hex(),
            tree,
            texts: data.current.clone(),
        };
        return Some((200, serde_json::to_value(head).expect("serializes")));
    }
    Some((404, json!({"error": "no such route"})))
}

async fn answer(mut stream: TcpStream, data: Arc<Mutex<Data>>) {
    let mut request = Vec::new();
    let mut chunk = [0u8; 2048];
    // The head is read first, then as much body as it announces.
    let mut end = None;
    while end.is_none() {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => request.extend_from_slice(&chunk[..read]),
        }
        end = request.windows(4).position(|window| window == b"\r\n\r\n");
    }
    let end = end.unwrap_or(0) + 4;
    let text = String::from_utf8_lossy(&request[..end]).into_owned();
    let announced = text
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while request.len() < end + announced {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => request.extend_from_slice(&chunk[..read]),
        }
    }
    let mut parts = text.lines().next().unwrap_or("").split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let answer = {
        let mut data = data.lock().unwrap();
        data.requests.push(target.clone());
        reply(&mut data, &method, &target, &request[end..])
    };
    let Some((status, body)) = answer else {
        return;
    };
    let body = body.to_string();
    let head = format!(
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
    let _ = stream.shutdown().await;
}
