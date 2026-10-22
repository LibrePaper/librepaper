//! The operations that only touch the private repository: blobs, trees,
//! commits, merges, diffs and the refs that keep an operation's commits
//! reachable.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;

use base64::Engine as _;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::storage::Conn;
use super::validate::{is_id, is_sha256, require_id, require_repo_path};
use super::{parse, At, GitEnv, GitError, GitManager};

/// The most blob content one `blobs` answer carries.
const MAX_BLOB_ANSWER_BYTES: u64 = 24 * 1024 * 1024;
const NULL_ID: &str = "0000000000000000000000000000000000000000";
/// The identity of commits whose ids must repeat: the same inputs always make
/// the same commit.
const PRIVATE_NAME: &str = "LibrePaper";
const PRIVATE_EMAIL: &str = "sync@librepaper.invalid";
const PRIVATE_DATE: &str = "946684800 +0000";
/// Modes a tree entry may have when the page sets one.
const MODES: &[&str] = &["100644", "100755", "120000", "160000"];

fn missing(id: &str) -> GitError {
    GitError::refused(
        "missing",
        format!("The object {id} is not in the private repository."),
    )
}

fn decode_utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Splits `<meta>\t<path>` records separated by NUL, as `ls-tree -z` writes.
/// The path comes back as the raw bytes Git stored, which need not be UTF-8.
fn records(bytes: &[u8]) -> impl Iterator<Item = (String, &[u8])> + '_ {
    bytes.split(|b| *b == 0).filter_map(|record| {
        let tab = record.iter().position(|b| *b == b'\t')?;
        Some((decode_utf8(&record[..tab]), &record[tab + 1..]))
    })
}

impl GitManager {
    /// Type and size of each of `ids` that exists. The ids must be validated.
    async fn check_objects(
        &self,
        conn: &Conn,
        ids: &[String],
    ) -> Result<HashMap<String, (String, u64)>, GitError> {
        let mut found = HashMap::new();
        if ids.is_empty() {
            return Ok(found);
        }
        let mut input = ids.join("\n");
        input.push('\n');
        let output = self
            .local(
                conn,
                &[
                    "cat-file",
                    "--batch-check=%(objectname) %(objecttype) %(objectsize)",
                ],
                Some(input.into_bytes()),
            )
            .await?;
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let mut parts = line.split(' ');
            let (Some(id), Some(kind)) = (parts.next(), parts.next()) else {
                continue;
            };
            if kind == "missing" {
                continue;
            }
            let size = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            found.insert(id.to_string(), (kind.to_string(), size));
        }
        Ok(found)
    }

    /// The tree id that `id` (a tree or a commit) names.
    async fn resolve_tree(&self, conn: &Conn, id: &str) -> Result<String, GitError> {
        let spec = format!("{id}^{{tree}}");
        let output = self
            .local_raw(conn, &["rev-parse", "--verify", "-q", &spec], None, &[])
            .await?;
        let text = output.text();
        if output.code != 0 || !is_id(&text) {
            return Err(GitError::refused(
                "missing",
                format!("{id} is not a tree or commit in the private repository."),
            ));
        }
        Ok(text)
    }

    pub(super) async fn tree(&self, conn: &Conn, body: &[u8]) -> Result<Value, GitError> {
        #[derive(Deserialize)]
        struct Request {
            commit: String,
        }
        let request: Request = parse(body)?;
        let id = require_id(&request.commit, "commit")?;
        let tree = self.resolve_tree(conn, id).await?;
        let output = self
            .local(conn, &["ls-tree", "-r", "-t", "-l", "-z", &tree], None)
            .await?;
        let mut entries = Vec::new();
        for (meta, raw) in records(&output.stdout) {
            let mut fields = meta.split_whitespace();
            let (Some(mode), Some(kind), Some(entry_id), Some(size)) =
                (fields.next(), fields.next(), fields.next(), fields.next())
            else {
                return Err(GitError::internal(
                    "Git's tree listing had an unexpected shape.",
                ));
            };
            let size = if kind == "blob" {
                size.parse::<u64>().ok()
            } else {
                None
            };
            let mut entry = json!({
                "path": decode_utf8(raw),
                "mode": mode,
                "type": kind,
                "id": entry_id,
                "size": size,
            });
            // A name that is not UTF-8 comes back lossy and flagged, so the
            // page never treats the lossy form as the real name.
            if std::str::from_utf8(raw).is_err() {
                entry["invalid_name"] = json!(true);
            }
            entries.push(entry);
        }
        Ok(json!({"tree": tree, "entries": entries}))
    }

    pub(super) async fn blobs(&self, conn: &Conn, body: &[u8]) -> Result<Value, GitError> {
        #[derive(Deserialize)]
        struct Request {
            ids: Vec<String>,
        }
        let request: Request = parse(body)?;
        let mut ids: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        for id in &request.ids {
            require_id(id, "A blob id")?;
            if seen.insert(id.clone()) {
                ids.push(id.clone());
            }
        }
        if ids.is_empty() {
            return Ok(json!({"blobs": {}}));
        }
        let found = self.check_objects(conn, &ids).await?;
        let mut total = 0u64;
        for id in &ids {
            match found.get(id) {
                None => return Err(missing(id)),
                Some((kind, size)) => {
                    if kind != "blob" {
                        return Err(GitError::refused(
                            "not-a-blob",
                            format!("The object {id} is a {kind}, not a blob."),
                        ));
                    }
                    total += size;
                }
            }
        }
        if total > MAX_BLOB_ANSWER_BYTES {
            return Err(GitError::new(
                413,
                None,
                "Those blobs are too large for one request. Ask for fewer at a time.",
            ));
        }
        let mut input = ids.join("\n");
        input.push('\n');
        let output = self
            .local(conn, &["cat-file", "--batch"], Some(input.into_bytes()))
            .await?;
        let data = &output.stdout;
        let short = || GitError::internal("Git's blob listing ended early.");
        let mut position = 0usize;
        let mut blobs = Map::new();
        for id in &ids {
            let end = data[position..]
                .iter()
                .position(|b| *b == b'\n')
                .ok_or_else(short)?
                + position;
            let header = decode_utf8(&data[position..end]);
            let size: usize = header
                .split(' ')
                .nth(2)
                .and_then(|s| s.parse().ok())
                .ok_or_else(short)?;
            let start = end + 1;
            let stop = start + size;
            if stop > data.len() {
                return Err(short());
            }
            blobs.insert(
                id.clone(),
                Value::String(base64::engine::general_purpose::STANDARD.encode(&data[start..stop])),
            );
            position = stop + 1;
        }
        Ok(json!({"blobs": Value::Object(blobs)}))
    }

    pub(super) async fn known_assets(&self, conn: &Conn, body: &[u8]) -> Result<Value, GitError> {
        #[derive(Deserialize)]
        struct Request {
            sha256: Vec<String>,
        }
        let request: Request = parse(body)?;
        let table = self.read_assets(conn);
        let mut candidates: Vec<(String, String)> = Vec::new();
        for digest in &request.sha256 {
            if !is_sha256(digest) {
                return Err(GitError::bad("A sha256 must be 64 lowercase hex digits."));
            }
            if let Some(blob) = table.get(digest) {
                if is_id(blob) {
                    candidates.push((digest.clone(), blob.clone()));
                }
            }
        }
        let ids: Vec<String> = candidates.iter().map(|(_, blob)| blob.clone()).collect();
        let found = self.check_objects(conn, &ids).await?;
        let mut known = Map::new();
        for (digest, blob) in candidates {
            if found.get(&blob).is_some_and(|(kind, _)| kind == "blob") {
                known.insert(digest, Value::String(blob));
            }
        }
        Ok(json!({"known": Value::Object(known)}))
    }

    pub(super) async fn put_blobs(&self, conn: &Conn, body: &[u8]) -> Result<Value, GitError> {
        #[derive(Deserialize)]
        struct Item {
            text: Option<String>,
            base64: Option<String>,
            sha256: Option<String>,
        }
        #[derive(Deserialize)]
        struct Request {
            blobs: Vec<Item>,
        }
        let request: Request = parse(body)?;
        if request.blobs.is_empty() {
            return Ok(json!({"ids": []}));
        }
        let mut contents: Vec<Vec<u8>> = Vec::with_capacity(request.blobs.len());
        let mut digests: Vec<Option<String>> = Vec::with_capacity(request.blobs.len());
        for item in &request.blobs {
            let bytes = match (&item.text, &item.base64) {
                (Some(text), None) => text.clone().into_bytes(),
                (None, Some(encoded)) => base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|_| GitError::bad("A blob's base64 is not valid."))?,
                _ => {
                    return Err(GitError::bad(
                        "Each blob needs exactly one of text or base64.",
                    ))
                }
            };
            let digest = match &item.sha256 {
                Some(claimed) => {
                    let claimed = claimed.to_ascii_lowercase();
                    if !is_sha256(&claimed) {
                        return Err(GitError::bad("A sha256 must be 64 hex digits."));
                    }
                    if hex::encode(Sha256::digest(&bytes)) != claimed {
                        return Err(GitError::bad("A blob does not match its sha256."));
                    }
                    Some(claimed)
                }
                None => None,
            };
            contents.push(bytes);
            digests.push(digest);
        }
        self.ensure_dir(conn)?;
        let scratch = tempfile::Builder::new()
            .prefix("tmp-blobs-")
            .tempdir_in(&conn.dir)
            .map_err(|error| {
                GitError::internal(format!("could not make a scratch directory: {error}"))
            })?;
        let mut names = String::new();
        for (index, bytes) in contents.iter().enumerate() {
            let path = scratch.path().join(index.to_string());
            std::fs::write(&path, bytes)
                .map_err(|error| GitError::internal(format!("could not stage a blob: {error}")))?;
            let text = path
                .to_str()
                .filter(|text| !text.contains('\n'))
                .ok_or_else(|| GitError::internal("The scratch path is not usable."))?;
            names.push_str(text);
            names.push('\n');
        }
        let output = self
            .local(
                conn,
                &["hash-object", "-w", "--no-filters", "--stdin-paths"],
                Some(names.into_bytes()),
            )
            .await?;
        let ids: Vec<String> = output
            .text()
            .lines()
            .map(|line| line.trim().to_string())
            .collect();
        if ids.len() != contents.len() || !ids.iter().all(|id| is_id(id)) {
            return Err(GitError::internal(
                "Git hashed a different number of blobs than were sent.",
            ));
        }
        let mut table = self.read_assets(conn);
        let mut changed = false;
        for (digest, id) in digests.iter().zip(&ids) {
            if let Some(digest) = digest {
                if table.get(digest) != Some(id) {
                    table.insert(digest.clone(), id.clone());
                    changed = true;
                }
            }
        }
        if changed {
            self.write_assets(conn, &table)?;
        }
        Ok(json!({"ids": ids}))
    }

    pub(super) async fn tree_edit(&self, conn: &Conn, body: &[u8]) -> Result<Value, GitError> {
        #[derive(Deserialize)]
        struct Edit {
            path: String,
            id: Option<String>,
            mode: Option<String>,
            #[serde(default)]
            delete: bool,
        }
        #[derive(Deserialize)]
        struct Request {
            base: Option<String>,
            edits: Vec<Edit>,
        }
        let request: Request = parse(body)?;
        for edit in &request.edits {
            require_repo_path(&edit.path)?;
            if edit.delete {
                continue;
            }
            let id = edit
                .id
                .as_deref()
                .ok_or_else(|| GitError::bad("An edit needs an id or delete."))?;
            require_id(id, "An edit id")?;
            if let Some(mode) = &edit.mode {
                if !MODES.contains(&mode.as_str()) {
                    return Err(GitError::bad(format!("The mode {mode} is not allowed.")));
                }
            }
        }
        let base = match &request.base {
            Some(base) => {
                require_id(base, "base")?;
                Some(self.resolve_tree(conn, base).await?)
            }
            None => None,
        };
        let mut existing: HashMap<String, String> = HashMap::new();
        let needs_modes = request.edits.iter().any(|e| !e.delete && e.mode.is_none());
        if let (Some(base), true) = (&base, needs_modes) {
            let output = self
                .local(conn, &["ls-tree", "-r", "-z", base], None)
                .await?;
            for (meta, raw) in records(&output.stdout) {
                // Edits name UTF-8 paths, so a path that is not UTF-8 has no
                // mode to keep.
                let Ok(path) = std::str::from_utf8(raw) else {
                    continue;
                };
                if let Some(mode) = meta.split_whitespace().next() {
                    existing.insert(path.to_string(), mode.to_string());
                }
            }
        }
        let mut ids: Vec<String> = request
            .edits
            .iter()
            .filter(|e| !e.delete)
            .filter_map(|e| e.id.clone())
            .collect();
        ids.sort();
        ids.dedup();
        let found = self.check_objects(conn, &ids).await?;
        let mut stream: Vec<u8> = Vec::new();
        for edit in request.edits.iter().filter(|e| e.delete) {
            stream.extend_from_slice(format!("0 {NULL_ID}\t{}", edit.path).as_bytes());
            stream.push(0);
        }
        for edit in request.edits.iter().filter(|e| !e.delete) {
            let id = edit.id.as_deref().unwrap_or_default();
            let mode = edit
                .mode
                .clone()
                .or_else(|| existing.get(&edit.path).cloned())
                .unwrap_or_else(|| "100644".to_string());
            let Some((kind, _)) = found.get(id) else {
                return Err(missing(id));
            };
            if mode != "160000" && kind != "blob" {
                return Err(GitError::refused(
                    "not-a-blob",
                    format!("The object {id} is a {kind}, not a blob."),
                ));
            }
            stream.extend_from_slice(format!("{mode} {id}\t{}", edit.path).as_bytes());
            stream.push(0);
        }
        self.ensure_dir(conn)?;
        let scratch = tempfile::Builder::new()
            .prefix("tmp-index-")
            .tempdir_in(&conn.dir)
            .map_err(|error| {
                GitError::internal(format!("could not make a scratch directory: {error}"))
            })?;
        let index = scratch.path().join("index");
        let envs: [(&str, &OsStr); 1] = [("GIT_INDEX_FILE", index.as_os_str())];
        if let Some(base) = &base {
            self.local_raw(conn, &["read-tree", base], None, &envs)
                .await?
                .ok()?;
        }
        if !stream.is_empty() {
            self.local_raw(
                conn,
                &["update-index", "-z", "--index-info"],
                Some(stream),
                &envs,
            )
            .await?
            .ok()?;
        }
        let written = self
            .local_raw(conn, &["write-tree"], None, &envs)
            .await?
            .ok()?;
        let tree = written.text();
        if !is_id(&tree) {
            return Err(GitError::internal("Git did not return a tree id."));
        }
        Ok(json!({"tree": tree}))
    }

    /// The machine's Git identity, when it has both a name and an email.
    async fn machine_identity(&self, conn: &Conn) -> Option<(String, String)> {
        let mut found = Vec::new();
        for key in ["user.name", "user.email"] {
            let output = self
                .runtime
                .run(
                    GitEnv::Transport,
                    At::Repo(&conn.repo),
                    &["config", "--get", key],
                    None,
                    &[],
                )
                .await
                .ok()?;
            let text = output.text();
            if output.code != 0 || text.is_empty() {
                return None;
            }
            found.push(text);
        }
        let email = found.pop()?;
        let name = found.pop()?;
        Some((name, email))
    }

    pub(super) async fn commit_tree(&self, conn: &Conn, body: &[u8]) -> Result<Value, GitError> {
        #[derive(Deserialize)]
        struct Author {
            name: String,
            email: String,
        }
        #[derive(Deserialize)]
        struct Request {
            tree: String,
            parents: Vec<String>,
            message: String,
            author: Option<Author>,
        }
        let request: Request = parse(body)?;
        require_id(&request.tree, "tree")?;
        for parent in &request.parents {
            require_id(parent, "A parent")?;
        }
        let mut variables: Vec<(&str, String)> = Vec::new();
        match &request.author {
            None => {
                for (name, value) in [
                    ("GIT_AUTHOR_NAME", PRIVATE_NAME),
                    ("GIT_AUTHOR_EMAIL", PRIVATE_EMAIL),
                    ("GIT_AUTHOR_DATE", PRIVATE_DATE),
                    ("GIT_COMMITTER_NAME", PRIVATE_NAME),
                    ("GIT_COMMITTER_EMAIL", PRIVATE_EMAIL),
                    ("GIT_COMMITTER_DATE", PRIVATE_DATE),
                ] {
                    variables.push((name, value.to_string()));
                }
            }
            Some(author) => {
                let name = author.name.trim();
                let email = author.email.trim();
                let usable =
                    |text: &str| !text.is_empty() && text.len() <= 256 && !text.contains('\0');
                if !usable(name) || !usable(email) {
                    return Err(GitError::bad("The author needs a name and an email."));
                }
                let (committer_name, committer_email) = self
                    .machine_identity(conn)
                    .await
                    .unwrap_or_else(|| (name.to_string(), email.to_string()));
                variables.push(("GIT_AUTHOR_NAME", name.to_string()));
                variables.push(("GIT_AUTHOR_EMAIL", email.to_string()));
                variables.push(("GIT_COMMITTER_NAME", committer_name));
                variables.push(("GIT_COMMITTER_EMAIL", committer_email));
            }
        }
        let envs: Vec<(&str, &OsStr)> = variables
            .iter()
            .map(|(name, value)| (*name, OsStr::new(value.as_str())))
            .collect();
        let mut args: Vec<&str> = vec!["commit-tree", request.tree.as_str()];
        for parent in &request.parents {
            args.push("-p");
            args.push(parent.as_str());
        }
        args.extend(["-F", "-"]);
        let output = self
            .local_raw(
                conn,
                &args,
                Some(request.message.clone().into_bytes()),
                &envs,
            )
            .await?
            .ok()?;
        let commit = output.text();
        if !is_id(&commit) {
            return Err(GitError::internal("Git did not return a commit id."));
        }
        Ok(json!({"commit": commit}))
    }

    pub(super) async fn merge(&self, conn: &Conn, body: &[u8]) -> Result<Value, GitError> {
        #[derive(Deserialize)]
        struct Request {
            ours: String,
            theirs: String,
        }
        let request: Request = parse(body)?;
        require_id(&request.ours, "ours")?;
        require_id(&request.theirs, "theirs")?;
        // Rename detection stays off so a rename arrives as a deletion plus
        // an addition. With it on, Git merges an edit at the old path into
        // the renamed file, and the page's per-path binary check misses it.
        let mut output = self
            .local_raw(
                conn,
                &[
                    "-c",
                    "merge.renames=false",
                    "merge-tree",
                    "--write-tree",
                    "-z",
                    &request.ours,
                    &request.theirs,
                ],
                None,
                &[],
            )
            .await?;
        // Histories with no common ancestor merge from an empty base. The
        // flag is newer than the first Git that has `--write-tree`, so it is
        // only tried once Git has said the histories are unrelated.
        if output.code != 0 && output.code != 1 && output.stderr.contains("unrelated histories") {
            output = self
                .local_raw(
                    conn,
                    &[
                        "-c",
                        "merge.renames=false",
                        "merge-tree",
                        "--write-tree",
                        "-z",
                        "--allow-unrelated-histories",
                        &request.ours,
                        &request.theirs,
                    ],
                    None,
                    &[],
                )
                .await?;
        }
        if output.code != 0 && output.code != 1 {
            return Err(GitError::refused(
                "git",
                format!(
                    "Git could not merge: {}",
                    super::validate::redact(&output.stderr)
                ),
            ));
        }
        let mut fields = output.stdout.split(|b| *b == 0);
        let tree = decode_utf8(fields.next().unwrap_or_default());
        if !is_id(&tree) {
            return Err(GitError::internal("Git did not return a merged tree id."));
        }
        if output.code == 0 {
            return Ok(json!({"clean": true, "tree": tree}));
        }
        // After the tree come `<mode> <object> <stage>\t<path>` records, then
        // an empty field, then Git's informational messages.
        let mut order: Vec<String> = Vec::new();
        let mut stages: HashMap<String, [Option<(String, String)>; 3]> = HashMap::new();
        for field in fields {
            if field.is_empty() {
                break;
            }
            let text = decode_utf8(field);
            let Some((meta, path)) = text.split_once('\t') else {
                continue;
            };
            let mut parts = meta.split(' ');
            let (Some(mode), Some(id), Some(stage)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let slot = match stage {
                "1" => 0,
                "2" => 1,
                "3" => 2,
                _ => continue,
            };
            if !stages.contains_key(path) {
                order.push(path.to_string());
            }
            stages.entry(path.to_string()).or_default()[slot] =
                Some((mode.to_string(), id.to_string()));
        }
        let mut conflicts = Vec::new();
        for path in order {
            let [base, ours, theirs] = stages.remove(&path).unwrap_or_default();
            let mode = ours
                .as_ref()
                .or(theirs.as_ref())
                .or(base.as_ref())
                .map(|(mode, _)| mode.clone());
            conflicts.push(json!({
                "path": path,
                "base": base.map(|(_, id)| id),
                "ours": ours.map(|(_, id)| id),
                "theirs": theirs.map(|(_, id)| id),
                "mode": mode,
            }));
        }
        Ok(json!({"clean": false, "tree": tree, "conflicts": conflicts}))
    }

    pub(super) async fn diff(&self, conn: &Conn, body: &[u8]) -> Result<Value, GitError> {
        #[derive(Deserialize)]
        struct Request {
            from: String,
            to: String,
        }
        let request: Request = parse(body)?;
        require_id(&request.from, "from")?;
        require_id(&request.to, "to")?;
        let output = self
            .local(
                conn,
                &[
                    "diff-tree",
                    "-r",
                    "-z",
                    "--no-renames",
                    &request.from,
                    &request.to,
                ],
                None,
            )
            .await?;
        let mut changes = Vec::new();
        let mut parts = output.stdout.split(|b| *b == 0).filter(|s| !s.is_empty());
        while let Some(meta) = parts.next() {
            let Some(path) = parts.next() else {
                break;
            };
            let path = std::str::from_utf8(path).map_err(|_| {
                GitError::internal("Git reported a changed path that is not UTF-8.")
            })?;
            let meta = decode_utf8(meta);
            let mut fields = meta.trim_start_matches(':').split(' ');
            let (_, _, Some(from), Some(to), Some(status)) = (
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
                fields.next(),
            ) else {
                return Err(GitError::internal(
                    "Git's diff listing had an unexpected shape.",
                ));
            };
            let status = match status.chars().next() {
                Some('A') => "A",
                Some('D') => "D",
                _ => "M",
            };
            let id = |id: &str| (id != NULL_ID).then(|| id.to_string());
            changes.push(json!({
                "path": path,
                "status": status,
                "from": id(from),
                "to": id(to),
            }));
        }
        Ok(json!({"changes": changes}))
    }

    pub(super) async fn ancestor(&self, conn: &Conn, body: &[u8]) -> Result<Value, GitError> {
        #[derive(Deserialize)]
        struct Request {
            ancestor: String,
            descendant: String,
        }
        let request: Request = parse(body)?;
        require_id(&request.ancestor, "ancestor")?;
        require_id(&request.descendant, "descendant")?;
        let output = self
            .local_raw(
                conn,
                &[
                    "merge-base",
                    "--is-ancestor",
                    &request.ancestor,
                    &request.descendant,
                ],
                None,
                &[],
            )
            .await?;
        match output.code {
            0 => Ok(json!({"is_ancestor": true})),
            1 => Ok(json!({"is_ancestor": false})),
            _ => Err(GitError::refused(
                "git",
                format!("Git failed: {}", super::validate::redact(&output.stderr)),
            )),
        }
    }

    pub(super) async fn keep(&self, conn: &Conn, body: &[u8]) -> Result<Value, GitError> {
        #[derive(Deserialize)]
        struct Request {
            operation: String,
            refs: std::collections::BTreeMap<String, String>,
        }
        let request: Request = parse(body)?;
        let prefix = operation_prefix(&request.operation)?;
        let mut script = String::new();
        for (name, commit) in &request.refs {
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            {
                return Err(GitError::bad(
                    "A ref name may use letters, digits, - and _ only.",
                ));
            }
            require_id(commit, "A ref target")?;
            script.push_str(&format!("update {prefix}{name} {commit}\n"));
        }
        if !script.is_empty() {
            self.local(conn, &["update-ref", "--stdin"], Some(script.into_bytes()))
                .await?;
        }
        Ok(json!({"ok": true}))
    }

    pub(super) async fn release(&self, conn: &Conn, body: &[u8]) -> Result<Value, GitError> {
        #[derive(Deserialize)]
        struct Request {
            operation: String,
        }
        let request: Request = parse(body)?;
        let prefix = operation_prefix(&request.operation)?;
        let listed = self
            .local(
                conn,
                &["for-each-ref", "--format=%(refname)", &prefix],
                None,
            )
            .await?;
        let mut script = String::new();
        for name in listed.text().lines() {
            let name = name.trim();
            if name.starts_with(prefix.as_str()) {
                script.push_str(&format!("delete {name}\n"));
            }
        }
        if !script.is_empty() {
            self.local(conn, &["update-ref", "--stdin"], Some(script.into_bytes()))
                .await?;
        }
        Ok(json!({"ok": true}))
    }
}

/// `refs/librepaper/ops/<operation>/` for an operation id such as
/// `v2.1700000000000.<32 hex>`, after checking it cannot escape that prefix.
fn operation_prefix(operation: &str) -> Result<String, GitError> {
    let ok = !operation.is_empty()
        && operation.len() <= 128
        && !operation.starts_with('.')
        && !operation.ends_with('.')
        && !operation.contains("..")
        && !operation.ends_with(".lock")
        && operation
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if !ok {
        return Err(GitError::bad("The operation id is not valid."));
    }
    Ok(format!("refs/librepaper/ops/{operation}/"))
}
