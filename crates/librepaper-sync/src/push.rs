//! `push` and `publish`, and the cheap questions a desktop asks between them.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::client::{refusal, Client};
use crate::diff::{self, Patch};
use crate::files::{sha256_hex, FileRecord, Kind};
use crate::git::Git;
use crate::link::Link;
use crate::pull::settle;
use crate::rules::Rules;
use crate::state::{PendingPush, Pushed, State, StateStore};
use crate::walk::{compare, scan, Local, LocalChanges, Scan};
use crate::Error;

/// How many pushed versions a state remembers until a pull meets them.
const MAX_PUSHED: usize = 200;

#[derive(Clone, Debug)]
pub struct PushOptions {
    /// What the version is called in the project's history.
    pub message: String,
    /// Who made the change, as git would say it. The account behind the
    /// credentials is recorded beside it.
    pub author: Option<String>,
    /// Paths the push leaves out, as if the directory did not hold them: exact
    /// paths, and directories with everything in them, relative to the
    /// directory with `/` between the parts. A file the caller makes in the
    /// directory that is not part of the project, such as a local render's
    /// output, goes here.
    pub exclude: Vec<String>,
    /// In a repository, the branch to sync. An unbound repository needs it on
    /// the first transfer; a bound one accepts only its own.
    pub branch: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushOutcome {
    /// The directory holds what the project last agreed on.
    Nothing,
    Pushed {
        label_sha: String,
        sequence: i64,
        /// How many files the push added, changed or deleted.
        files: usize,
    },
    /// These files changed on the web since they were last pulled.
    Refused { paths: Vec<String> },
    /// A pending suggestion is in the way of this file.
    Suggestion { path: String },
}

/// The change a push sends, before the assets it needs are uploaded.
struct Plan<'a> {
    dependencies: Vec<Value>,
    patches: Vec<Patch>,
    create: Vec<Value>,
    delete: Vec<Value>,
    put_binary: Vec<Value>,
    uploads: Vec<&'a Local>,
}

impl Plan<'_> {
    fn is_empty(&self) -> bool {
        self.patches.is_empty()
            && self.create.is_empty()
            && self.delete.is_empty()
            && self.put_binary.is_empty()
    }

    fn body(&self, operation: &str, sync: Value) -> Value {
        json!({
            "operation": operation,
            "change": {
                "dependencies": self.dependencies,
                "patches": self.patches,
                "create": self.create,
                "delete": self.delete,
                "put_binary": self.put_binary,
            },
            "sync": sync,
        })
    }
}

/// What to send for the paths that differ. `bases` holds the LF text each
/// changed text file was last agreed on.
fn plan_change<'a>(
    recorded: &BTreeMap<String, FileRecord>,
    found: &'a Scan,
    changes: &LocalChanges,
    bases: &BTreeMap<String, String>,
) -> Plan<'a> {
    let mut plan = Plan {
        dependencies: Vec::new(),
        patches: Vec::new(),
        create: Vec::new(),
        delete: Vec::new(),
        put_binary: Vec::new(),
        uploads: Vec::new(),
    };
    for path in &changes.added {
        let file = &found.files[path];
        match file.kind {
            Kind::Text => plan.create.push(json!({
                "path": path,
                "text": file.text.as_deref().unwrap_or_default(),
            })),
            Kind::Asset => {
                plan.uploads.push(file);
                plan.put_binary
                    .push(json!({"path": path, "hash": file.sha, "expect": Value::Null}));
            }
        }
    }
    for path in &changes.changed {
        let file = &found.files[path];
        let Some(before) = recorded.get(path) else {
            continue;
        };
        match file.kind {
            Kind::Text => {
                let Some(base) = bases.get(path) else {
                    continue;
                };
                let edits = diff::patches(path, base, file.text.as_deref().unwrap_or_default());
                if edits.is_empty() {
                    continue;
                }
                plan.dependencies.push(dependency(path, &before.sha));
                plan.patches.extend(edits);
            }
            Kind::Asset => {
                plan.uploads.push(file);
                plan.put_binary
                    .push(json!({"path": path, "hash": file.sha, "expect": before.sha}));
            }
        }
    }
    for path in &changes.deleted {
        let Some(before) = recorded.get(path) else {
            continue;
        };
        if before.kind == Kind::Text {
            plan.dependencies.push(dependency(path, &before.sha));
        }
        plan.delete
            .push(json!({"path": path, "expect": before.sha}));
    }
    plan
}

/// A text file the change reads: it must still hash to what was recorded.
fn dependency(path: &str, hash: &str) -> Value {
    json!({"path": path, "file_hash": hash, "start": 0, "end": 0, "exact": ""})
}

/// What the server said to a source change.
enum Sent {
    Accepted {
        sha: String,
        sequence: i64,
    },
    /// A definitive refusal: the change was not made.
    Refused(PushOutcome),
}

/// Posts a source change and reads the answer. An answer that is neither an
/// acceptance nor a known refusal is an error, and settles nothing.
async fn send(client: &Client, slug: &str, body: &Value) -> Result<Sent, Error> {
    let (status, reply) = client.source_change(slug, body).await?;
    match status {
        200 => {}
        409 => match reply.get("reason").and_then(Value::as_str) {
            Some("changed") => {
                let listed = reply.get("paths").or_else(|| reply.get("files"));
                return Ok(Sent::Refused(PushOutcome::Refused {
                    paths: strings(listed),
                }));
            }
            Some("suggestion") => {
                return Ok(Sent::Refused(PushOutcome::Suggestion {
                    path: reply
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                }));
            }
            _ => return Err(refusal(status, &reply)),
        },
        _ => return Err(refusal(status, &reply)),
    }
    let label = reply.get("label");
    let sha = label
        .and_then(|label| label.get("sha"))
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Invalid("the server did not name the version it recorded".into()))?
        .to_string();
    let sequence = label
        .and_then(|label| label.get("sequence"))
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            Error::Invalid("the server did not number the version it recorded".into())
        })?;
    Ok(Sent::Accepted { sha, sequence })
}

/// Posts the request a pending push holds and settles the state by what the
/// server said. Only the effects saved with the request are applied. A
/// refusal clears the attempt and leaves the baseline where it was; an error
/// keeps the attempt, key and effects, for the next push.
async fn settle_pending(
    client: &Client,
    store: &dyn StateStore,
    state: &mut State,
    pending: &PendingPush,
) -> Result<PushOutcome, Error> {
    let slug = state.link.slug.clone();
    match send(client, &slug, &pending.request).await? {
        Sent::Refused(outcome) => {
            state.pending = None;
            store.save(state)?;
            Ok(outcome)
        }
        Sent::Accepted { sha, sequence } => {
            state.files.extend(pending.upsert_files.clone());
            for path in &pending.deleted_paths {
                state.files.remove(path);
            }
            state.synced.extend(
                pending
                    .upsert_files
                    .keys()
                    .chain(&pending.deleted_paths)
                    .cloned(),
            );
            state.pushed.push(Pushed {
                label: sha.clone(),
                commit: pending.commit.clone(),
            });
            let excess = state.pushed.len().saturating_sub(MAX_PUSHED);
            state.pushed.drain(..excess);
            if pending.commit.is_some() {
                state.head = pending.commit.clone();
            }
            // The project's head moved with this push, so the next pull reads
            // it once.
            state.web_head = None;
            state.pending = None;
            store.save(state)?;
            Ok(PushOutcome::Pushed {
                label_sha: sha,
                sequence,
                files: pending.upsert_files.len() + pending.deleted_paths.len(),
            })
        }
    }
}

/// Sends the directory's changes to the project as one named version. A push
/// that was posted and never settled is settled first, from the request and
/// effects saved before it was posted; files changed since are new work for
/// the same call.
pub async fn push(
    client: &Client,
    dir: &Path,
    store: &dyn StateStore,
    options: &PushOptions,
) -> Result<PushOutcome, Error> {
    if options.message.trim().is_empty() {
        return Err(Error::Invalid("a push needs a message".into()));
    }
    let mut state = store.load()?.ok_or(Error::NotLinked)?;
    let git = store.repository().map(Git::at);
    // A merge that stopped with conflicts leaves markers in the files.
    if git
        .as_ref()
        .is_some_and(|git| git.succeeds(&["rev-parse", "-q", "--verify", "MERGE_HEAD"]))
    {
        return Err(Error::Merging);
    }
    state.pin_branch(store.repository(), options.branch.as_deref())?;
    let mut settled = None;
    if let Some(pending) = state.pending.clone() {
        if pending.actor != client.actor() {
            return Err(Error::PendingActor);
        }
        match settle_pending(client, store, &mut state, &pending).await? {
            outcome @ PushOutcome::Pushed { .. } => settled = Some(outcome),
            refused => return Ok(refused),
        }
    }
    let rules = client.rules().await?;
    if let Some(git) = &git {
        if settle(git, &mut state) {
            store.save(&state)?;
        }
    }
    let found = scan(dir, &rules, git.as_ref(), &options.exclude)?;
    let changes = compare(dir, &state.files, &found);
    if changes.is_empty() {
        return Ok(settled.unwrap_or(PushOutcome::Nothing));
    }
    if !changes.added.is_empty() && found.files.len() > rules.max_files {
        return Err(too_many(&rules, found.files.len()));
    }
    let slug = state.link.slug.clone();

    // A patch names the text it replaces, so the text the edits were made
    // against is read from the project, and must still be what was recorded.
    let mut bases: BTreeMap<String, String> = BTreeMap::new();
    let edited: Vec<&String> = changes
        .changed
        .iter()
        .filter(|path| found.files[*path].kind == Kind::Text)
        .collect();
    if !edited.is_empty() {
        let head = client.head(&slug).await?;
        let mut moved = Vec::new();
        for path in edited {
            let recorded = &state.files[path];
            match (head.files.get(path), head.texts.get(path)) {
                (Some(now), Some(text)) if now.sha == recorded.sha => {
                    bases.insert(path.clone(), text.clone());
                }
                _ => moved.push(path.clone()),
            }
        }
        if !moved.is_empty() {
            return Ok(PushOutcome::Refused { paths: moved });
        }
    }

    let plan = plan_change(&state.files, &found, &changes, &bases);
    if plan.is_empty() {
        return Ok(settled.unwrap_or(PushOutcome::Nothing));
    }
    for file in &plan.uploads {
        upload(client, &slug, file).await?;
    }
    let mut sync = json!({"provider": "cli", "message": options.message});
    if let Some(author) = options
        .author
        .as_deref()
        .map(str::trim)
        .filter(|author| !author.is_empty())
    {
        sync["author"] = json!(author);
    }
    let request_key = librepaper_base::util::new_request_key();
    let request = plan.body(&request_key, sync);
    let upsert_files = changes
        .added
        .iter()
        .chain(&changes.changed)
        .map(|path| {
            let file = &found.files[path];
            (
                path.clone(),
                FileRecord {
                    kind: file.kind,
                    sha: file.sha.clone(),
                },
            )
        })
        .collect();
    let pending = PendingPush {
        request_key,
        actor: client.actor(),
        request,
        upsert_files,
        deleted_paths: changes.deleted.clone(),
        commit: git.as_ref().and_then(|git| git.rev("HEAD")),
    };
    // Saved before the post, so an answer that never arrives can be settled
    // by the next push. When the save fails nothing has been sent.
    state.pending = Some(pending.clone());
    store.save(&state)?;
    settle_pending(client, store, &mut state, &pending).await
}

/// Sends an asset the project does not hold yet.
async fn upload(client: &Client, slug: &str, file: &Local) -> Result<(), Error> {
    if client.has_asset(slug, &file.sha).await? {
        return Ok(());
    }
    let bytes = std::fs::read(&file.file).map_err(|error| Error::io(&file.file, error))?;
    if sha256_hex(&bytes) != file.sha {
        return Err(Error::Invalid(format!(
            "{} changed while it was being sent",
            file.file.display()
        )));
    }
    client.upload_asset(slug, bytes, &file.sha).await
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn too_many(rules: &Rules, found: usize) -> Error {
    Error::Rules(format!(
        "a project may hold {} files and this directory has {found}",
        rules.max_files
    ))
}

/// The files a new project is created with: everything the scan found, with
/// its bytes.
fn upload_list(found: &Scan, rules: &Rules) -> Result<Vec<(String, Vec<u8>)>, Error> {
    if found.files.len() > rules.max_files {
        return Err(too_many(rules, found.files.len()));
    }
    let mut sent = Vec::with_capacity(found.files.len());
    for (path, file) in &found.files {
        let bytes = match &file.text {
            Some(text) => text.clone().into_bytes(),
            None => {
                let bytes =
                    std::fs::read(&file.file).map_err(|error| Error::io(&file.file, error))?;
                if sha256_hex(&bytes) != file.sha {
                    return Err(Error::Invalid(format!(
                        "{} changed while it was being sent",
                        file.file.display()
                    )));
                }
                bytes
            }
        };
        sent.push((path.clone(), bytes));
    }
    Ok(sent)
}

/// Creates a web project from the directory and links the directory to it.
/// Paths in `exclude` (see `PushOptions`) are not sent.
pub async fn publish(
    client: &Client,
    dir: &Path,
    store: &dyn StateStore,
    title: &str,
    exclude: &[String],
) -> Result<Link, Error> {
    if store.load()?.is_some() {
        return Err(Error::Invalid(
            "this directory is already linked to a project".into(),
        ));
    }
    let rules = client.rules().await?;
    let git = store.repository().map(Git::at);
    let found = scan(dir, &rules, git.as_ref(), exclude)?;
    let sent = upload_list(&found, &rules)?;
    let slug = client.publish(title, &sent).await?;
    let link = Link::at(client.site(), slug.clone());

    let mut state = State::new(link.clone());
    // The version the project was created with is the one this directory
    // already holds. Not knowing it only means the first pull reads more.
    state.pulled = client
        .labels(&slug, false, 1, None)
        .await
        .ok()
        .and_then(|page| page.labels.first().map(|label| label.sequence));
    state.files = found.records();
    state.synced = state.files.keys().cloned().collect();
    state.head = git.as_ref().and_then(|git| git.rev("HEAD"));
    store.save(&state)?;
    Ok(link)
}

/// How many synced files differ from what the project last agreed on. Reads
/// the directory and nothing else. Paths in `exclude` (see `PushOptions`) are
/// not counted.
pub fn changes_here(
    dir: &Path,
    state: &State,
    rules: &Rules,
    exclude: &[String],
) -> Result<usize, Error> {
    let git = Git::toplevel(dir);
    let found = scan(dir, rules, git.as_ref(), exclude)?;
    Ok(compare(dir, &state.files, &found).count())
}

/// Whether the project has something this directory has not taken in: a named
/// version past `pulled`, or accepted files that differ from the ones the
/// state represents. The project's files are read only when its head identity
/// differs from the recorded one. A read that finds nothing new records the
/// identity in `store`, so the next check is cheap.
pub async fn web_changed(
    client: &Client,
    store: &dyn StateStore,
    state: &State,
) -> Result<bool, Error> {
    let page = client.labels(&state.link.slug, true, 1, None).await?;
    let named = page.labels.first().is_some_and(|label| match state.pulled {
        Some(pulled) => label.sequence > pulled,
        None => true,
    });
    if named {
        return Ok(true);
    }
    if page.head.is_some() && page.head == state.web_head {
        return Ok(false);
    }
    let rules = client.rules().await?;
    let mut current = client.head(&state.link.slug).await?;
    current.retain_synced(&rules);
    // A fetch not yet merged is what the directory is about to agree with.
    let recorded = match &state.fetched {
        Some(fetched) => &fetched.files,
        None => &state.files,
    };
    if current.files != *recorded {
        return Ok(true);
    }
    if page.head.is_some() {
        let mut seen = state.clone();
        seen.web_head = page.head;
        store.save(&seen)?;
    }
    Ok(false)
}

/// The latest commit of a repository, as a push's default message and author.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitSummary {
    /// The subject of `HEAD`, with "and N more" when several commits have
    /// been made since the last push.
    pub message: String,
    pub author: String,
}

pub fn commit_summary(dir: &Path, state: &State) -> Option<CommitSummary> {
    let git = Git::toplevel(dir)?;
    let subject = git.line(&["log", "-1", "--format=%s"]).ok()?;
    if subject.is_empty() {
        return None;
    }
    let author = git.line(&["log", "-1", "--format=%an"]).ok()?;
    let since = state.head.as_deref().and_then(|head| {
        let range = format!("{head}..HEAD");
        git.line(&["rev-list", "--count", range.as_str()])
            .ok()?
            .parse::<usize>()
            .ok()
    });
    let message = match since {
        Some(count) if count > 1 => format!("{subject} and {} more", count - 1),
        _ => subject,
    };
    Some(CommitSummary { message, author })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::testing::{commit_all, identify, repository};
    use crate::link::Link;
    use crate::state::GitRefStore;

    fn write(dir: &Path, path: &str, bytes: &[u8]) {
        let target = dir.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, bytes).unwrap();
    }

    fn record(kind: Kind, bytes: &[u8]) -> FileRecord {
        FileRecord {
            kind,
            sha: sha256_hex(bytes),
        }
    }

    #[test]
    fn a_push_is_patches_creates_deletes_and_assets_with_the_hashes_they_expect() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "edited.tex", b"alpha\nBETA\ngamma\n");
        write(dir.path(), "new.tex", b"fresh\n");
        write(dir.path(), "fig/new.png", b"new-figure");
        write(dir.path(), "fig/changed.png", b"after");
        write(dir.path(), "same.tex", b"same\n");
        let found = scan(dir.path(), &Rules::default(), None, &[]).unwrap();
        let recorded = BTreeMap::from([
            (
                "edited.tex".to_string(),
                record(Kind::Text, b"alpha\nbeta\ngamma\n"),
            ),
            ("same.tex".to_string(), record(Kind::Text, b"same\n")),
            ("gone.tex".to_string(), record(Kind::Text, b"gone\n")),
            (
                "fig/changed.png".to_string(),
                record(Kind::Asset, b"before"),
            ),
            (
                "fig/gone.png".to_string(),
                record(Kind::Asset, b"old-figure"),
            ),
        ]);
        let changes = compare(dir.path(), &recorded, &found);
        let bases =
            BTreeMap::from([("edited.tex".to_string(), "alpha\nbeta\ngamma\n".to_string())]);

        let plan = plan_change(&recorded, &found, &changes, &bases);
        assert_eq!(plan.patches.len(), 1);
        assert_eq!(plan.patches[0].exact, "beta");
        assert_eq!(plan.patches[0].replacement, "BETA");
        assert_eq!(plan.patches[0].start, 6);
        assert_eq!(plan.create, [json!({"path": "new.tex", "text": "fresh\n"})]);

        let edited = sha256_hex(b"alpha\nbeta\ngamma\n");
        let gone = sha256_hex(b"gone\n");
        assert_eq!(
            plan.dependencies,
            [
                dependency("edited.tex", &edited),
                dependency("gone.tex", &gone)
            ]
        );
        assert_eq!(
            plan.delete,
            [
                json!({"path": "fig/gone.png", "expect": sha256_hex(b"old-figure")}),
                json!({"path": "gone.tex", "expect": gone}),
            ]
        );
        assert_eq!(
            plan.put_binary,
            [
                json!({"path": "fig/new.png", "hash": sha256_hex(b"new-figure"), "expect": null}),
                json!({"path": "fig/changed.png", "hash": sha256_hex(b"after"), "expect": sha256_hex(b"before")}),
            ]
        );
        assert_eq!(plan.uploads.len(), 2);

        let body = plan.body("v2.1.0", json!({"provider": "cli", "message": "Edit"}));
        assert_eq!(body["operation"], "v2.1.0");
        assert_eq!(body["sync"]["provider"], "cli");
        assert_eq!(body["change"]["patches"][0]["path"], "edited.tex");
        assert_eq!(body["change"]["dependencies"][0]["start"], 0);
        assert_eq!(body["change"]["dependencies"][0]["exact"], "");
        let mut keys: Vec<&str> = body["change"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            ["create", "delete", "dependencies", "patches", "put_binary"],
            "exactly the fields the route reads"
        );
    }

    #[test]
    fn a_line_ending_only_edit_sends_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.tex", b"one\r\ntwo\r\n");
        let found = scan(dir.path(), &Rules::default(), None, &[]).unwrap();
        let recorded = BTreeMap::from([("a.tex".to_string(), record(Kind::Text, b"one\ntwo\n"))]);
        let changes = compare(dir.path(), &recorded, &found);
        assert!(changes.is_empty(), "the hashes are of the LF text");
        let plan = plan_change(&recorded, &found, &changes, &BTreeMap::new());
        assert!(plan.is_empty());
    }

    #[test]
    fn a_deleted_asset_needs_no_dependency() {
        let dir = tempfile::tempdir().unwrap();
        let found = scan(dir.path(), &Rules::default(), None, &[]).unwrap();
        let recorded = BTreeMap::from([("fig/a.png".to_string(), record(Kind::Asset, b"x"))]);
        let changes = compare(dir.path(), &recorded, &found);
        let plan = plan_change(&recorded, &found, &changes, &BTreeMap::new());
        assert!(plan.dependencies.is_empty());
        assert_eq!(plan.delete.len(), 1);
    }

    #[test]
    fn a_recorded_file_that_became_ignored_is_not_deleted_but_one_that_is_gone_is() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), ".gitignore", b"refs.bib\n");
        write(dir.path(), "main.tex", b"main\n");
        write(dir.path(), "refs.bib", b"@book{}\n");
        let found = scan(dir.path(), &Rules::default(), None, &[]).unwrap();
        let mut recorded = BTreeMap::from([
            ("main.tex".to_string(), record(Kind::Text, b"main\n")),
            ("refs.bib".to_string(), record(Kind::Text, b"@book{}\n")),
        ]);
        let changes = compare(dir.path(), &recorded, &found);
        assert!(
            changes.is_empty(),
            "the file left the sync, it was not deleted"
        );
        assert!(plan_change(&recorded, &found, &changes, &BTreeMap::new()).is_empty());

        recorded.insert("gone.tex".to_string(), record(Kind::Text, b"gone\n"));
        let changes = compare(dir.path(), &recorded, &found);
        let plan = plan_change(&recorded, &found, &changes, &BTreeMap::new());
        let gone = sha256_hex(b"gone\n");
        assert_eq!(plan.delete, [json!({"path": "gone.tex", "expect": gone})]);
        assert_eq!(plan.dependencies, [dependency("gone.tex", &gone)]);
    }

    #[test]
    fn a_push_does_not_send_a_file_a_render_wrote() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "main.tex", b"main\n");
        write(dir.path(), "main.pdf", b"%PDF rendered");
        write(dir.path(), "main_files/libs/x.css", b"body {}\n");
        write(dir.path(), "fig/new.png", b"new-figure");
        let recorded = BTreeMap::from([("main.tex".to_string(), record(Kind::Text, b"main\n"))]);

        let listed = scan(dir.path(), &Rules::default(), None, &[]).unwrap();
        let changes = compare(dir.path(), &recorded, &listed);
        assert_eq!(
            changes.added,
            ["fig/new.png", "main.pdf", "main_files/libs/x.css"],
            "without the exclusion the render's files are a push"
        );

        let exclude = ["main.pdf".to_string(), "main_files".to_string()];
        let found = scan(dir.path(), &Rules::default(), None, &exclude).unwrap();
        let changes = compare(dir.path(), &recorded, &found);
        assert_eq!(changes.added, ["fig/new.png"]);
        let plan = plan_change(&recorded, &found, &changes, &BTreeMap::new());
        assert!(plan.create.is_empty());
        assert_eq!(
            plan.put_binary,
            [json!({"path": "fig/new.png", "hash": sha256_hex(b"new-figure"), "expect": null})]
        );
        assert_eq!(plan.uploads.len(), 1);
    }

    #[test]
    fn a_new_project_is_not_created_with_a_file_a_render_wrote() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "main.tex", b"main\n");
        write(dir.path(), "chapters/one.tex", b"one\n");
        write(dir.path(), "main.pdf", b"%PDF rendered");
        write(dir.path(), "main_files/libs/x.css", b"body {}\n");
        let rules = Rules::default();
        let paths = |exclude: &[String]| -> Vec<String> {
            let found = scan(dir.path(), &rules, None, exclude).unwrap();
            upload_list(&found, &rules)
                .unwrap()
                .into_iter()
                .map(|(path, _)| path)
                .collect()
        };
        assert_eq!(
            paths(&[]),
            [
                "chapters/one.tex",
                "main.pdf",
                "main.tex",
                "main_files/libs/x.css"
            ]
        );
        let exclude = ["main.pdf".to_string(), "main_files".to_string()];
        assert_eq!(paths(&exclude), ["chapters/one.tex", "main.tex"]);

        // The bytes sent are the files' own.
        let found = scan(dir.path(), &rules, None, &exclude).unwrap();
        let sent = upload_list(&found, &rules).unwrap();
        assert_eq!(sent[1], ("main.tex".to_string(), b"main\n".to_vec()));
    }

    #[test]
    fn the_changes_here_leave_out_what_the_caller_excludes() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "main.tex", b"main\n");
        write(dir.path(), "main.pdf", b"%PDF rendered");
        write(dir.path(), "main_files/libs/x.css", b"body {}\n");
        let mut state = State::new(Link::parse("https://app.librepaper.org/docs/paper").unwrap());
        state.files = BTreeMap::from([("main.tex".to_string(), record(Kind::Text, b"main\n"))]);
        let rules = Rules::default();

        assert_eq!(changes_here(dir.path(), &state, &rules, &[]).unwrap(), 2);
        let exclude = ["main.pdf".to_string(), "main_files".to_string()];
        assert_eq!(
            changes_here(dir.path(), &state, &rules, &exclude).unwrap(),
            0
        );

        // A real edit is still counted beside them.
        write(dir.path(), "main.tex", b"edited\n");
        assert_eq!(
            changes_here(dir.path(), &state, &rules, &exclude).unwrap(),
            1
        );
    }

    #[test]
    fn conflict_paths_are_read_from_either_field() {
        let listed = json!(["a.tex", "b.tex"]);
        assert_eq!(strings(Some(&listed)), ["a.tex", "b.tex"]);
        assert!(strings(None).is_empty());
        assert!(strings(Some(&json!("a.tex"))).is_empty());
    }

    #[test]
    fn the_default_message_is_the_latest_commit_and_how_many_came_with_it() {
        let Some((dir, git)) = repository() else {
            return;
        };
        write(dir.path(), "a.tex", b"1\n");
        let first = commit_all(&git, "First");
        write(dir.path(), "a.tex", b"2\n");
        commit_all(&git, "Second");
        write(dir.path(), "a.tex", b"3\n");
        commit_all(&git, "Third");

        let mut state = State::new(Link::parse("https://app.librepaper.org/docs/paper").unwrap());
        let summary = commit_summary(git.dir(), &state).unwrap();
        assert_eq!(summary.message, "Third");
        assert_eq!(summary.author, "Test");

        state.head = Some(first);
        let summary = commit_summary(git.dir(), &state).unwrap();
        assert_eq!(summary.message, "Third and 1 more");
    }

    #[test]
    fn the_default_message_needs_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        let state = State::new(Link::parse("https://app.librepaper.org/docs/paper").unwrap());
        assert_eq!(commit_summary(dir.path(), &state), None);
    }

    #[tokio::test]
    async fn a_push_is_refused_while_a_merge_is_in_progress() {
        let Some((dir, git)) = repository() else {
            return;
        };
        identify(&git);
        write(dir.path(), "a.tex", b"base\n");
        commit_all(&git, "Base");
        git.run(&["branch", "theirs"], None, &[]).unwrap();
        write(dir.path(), "a.tex", b"mine\n");
        commit_all(&git, "Mine");
        git.run(&["checkout", "-q", "theirs"], None, &[]).unwrap();
        write(dir.path(), "a.tex", b"theirs\n");
        commit_all(&git, "Theirs");
        git.run(&["checkout", "-q", "-"], None, &[]).unwrap();
        // The merge stops with a conflict in a.tex, and MERGE_HEAD is left.
        assert!(!git
            .output(&["merge", "--no-edit", "theirs"], None, &[])
            .unwrap()
            .status
            .success());
        assert!(git.succeeds(&["rev-parse", "-q", "--verify", "MERGE_HEAD"]));

        let store = GitRefStore::open(dir.path()).unwrap();
        let state = State::new(Link::parse("https://app.librepaper.org/docs/paper").unwrap());
        store.save(&state).unwrap();
        // Nothing listens on this port, so a request would fail as a network
        // error rather than as a merge.
        let site = url::Url::parse("http://127.0.0.1:9").unwrap();
        let client = Client::new(site, librepaper_base::http::Credentials::token("lp_token"));
        let options = PushOptions {
            message: "Edits".into(),
            author: None,
            exclude: Vec::new(),
            branch: None,
        };
        let outcome = push(&client, dir.path(), &store, &options).await;
        assert!(matches!(outcome, Err(Error::Merging)), "{outcome:?}");
    }

    // ----------------------------------------------------- branch and pending

    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// What the fake deployment does with one source change.
    enum Answer {
        Reply(u16, Value),
        /// Reads the request and closes the connection without answering.
        Drop,
    }

    /// A deployment that answers `/api/config` and plays back scripted
    /// answers to source changes, keeping every body it was sent.
    struct Fake {
        site: url::Url,
        posted: Arc<Mutex<Vec<Value>>>,
    }

    impl Fake {
        fn link(&self) -> Link {
            Link::parse(&format!("{}docs/paper", self.site)).unwrap()
        }

        fn client(&self, token: &str) -> Client {
            Client::new(
                self.site.clone(),
                librepaper_base::http::Credentials::token(token),
            )
        }

        fn posted(&self) -> Vec<Value> {
            self.posted.lock().unwrap().clone()
        }
    }

    async fn fake(answers: Vec<Answer>) -> Fake {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let site = url::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let posted = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(Mutex::new(VecDeque::from(answers)));
        let (log, queue) = (posted.clone(), script);
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let (log, queue) = (log.clone(), queue.clone());
                tokio::spawn(async move {
                    let mut raw = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let body_at = loop {
                        let read = socket.read(&mut chunk).await.unwrap_or(0);
                        if read == 0 {
                            return;
                        }
                        raw.extend_from_slice(&chunk[..read]);
                        if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            break at + 4;
                        }
                    };
                    let head = String::from_utf8_lossy(&raw[..body_at]).to_string();
                    let length = head
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    while raw.len() < body_at + length {
                        let read = socket.read(&mut chunk).await.unwrap_or(0);
                        if read == 0 {
                            return;
                        }
                        raw.extend_from_slice(&chunk[..read]);
                    }
                    let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
                    let (status, body) = if path.ends_with("/api/config") {
                        (200, json!({}))
                    } else if path.ends_with("/source-change") {
                        let sent = serde_json::from_slice(&raw[body_at..]).unwrap();
                        log.lock().unwrap().push(sent);
                        let next = queue.lock().unwrap().pop_front();
                        match next {
                            Some(Answer::Reply(status, body)) => (status, body),
                            Some(Answer::Drop) | None => return,
                        }
                    } else {
                        (404, json!({}))
                    };
                    let text = body.to_string();
                    let reply = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                        text.len()
                    );
                    let _ = socket.write_all(reply.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        Fake { site, posted }
    }

    fn accepted(sha: &str, sequence: i64) -> Answer {
        Answer::Reply(200, json!({"label": {"sha": sha, "sequence": sequence}}))
    }

    fn options(branch: Option<&str>) -> PushOptions {
        PushOptions {
            message: "Edits".into(),
            author: None,
            exclude: Vec::new(),
            branch: branch.map(str::to_string),
        }
    }

    /// A repository on `main` holding a.tex, with its state saved and bound to
    /// `bound`.
    fn on_main(link: Link, bound: Option<&str>) -> Option<(tempfile::TempDir, Git, GitRefStore)> {
        let (dir, git) = repository()?;
        identify(&git);
        git.run(&["symbolic-ref", "HEAD", "refs/heads/main"], None, &[])
            .unwrap();
        write(dir.path(), "a.tex", b"one\n");
        commit_all(&git, "First");
        let store = GitRefStore::open(dir.path()).unwrap();
        let mut state = State::new(link);
        state.branch = bound.map(str::to_string);
        store.save(&state).unwrap();
        Some((dir, git, store))
    }

    #[tokio::test]
    async fn an_unbound_repository_needs_a_branch_and_the_push_binds_it() {
        let server = fake(vec![accepted("l1", 1)]).await;
        let Some((dir, _git, store)) = on_main(server.link(), None) else {
            return;
        };
        let client = server.client("lp_token");
        let refused = push(&client, dir.path(), &store, &options(None)).await;
        assert!(
            matches!(&refused, Err(Error::BranchUnbound { checked_out }) if checked_out == "main"),
            "{refused:?}"
        );
        assert!(server.posted().is_empty());
        assert_eq!(store.load().unwrap().unwrap().branch, None);

        let sent = push(&client, dir.path(), &store, &options(Some("main"))).await;
        assert!(matches!(sent, Ok(PushOutcome::Pushed { .. })), "{sent:?}");
        assert_eq!(
            store.load().unwrap().unwrap().branch.as_deref(),
            Some("main")
        );
    }

    #[tokio::test]
    async fn a_detached_head_is_refused() {
        let server = fake(Vec::new()).await;
        let Some((dir, git, store)) = on_main(server.link(), Some("main")) else {
            return;
        };
        git.run(&["checkout", "-q", "--detach"], None, &[]).unwrap();
        let client = server.client("lp_token");
        let outcome = push(&client, dir.path(), &store, &options(None)).await;
        assert!(matches!(outcome, Err(Error::DetachedHead)), "{outcome:?}");
        assert!(server.posted().is_empty());
    }

    #[tokio::test]
    async fn another_branch_is_refused_and_named() {
        let server = fake(Vec::new()).await;
        let Some((dir, git, store)) = on_main(server.link(), Some("main")) else {
            return;
        };
        git.run(&["checkout", "-q", "-b", "other"], None, &[])
            .unwrap();
        let client = server.client("lp_token");
        let outcome = push(&client, dir.path(), &store, &options(None)).await;
        let Err(error) = outcome else {
            panic!("a push from the wrong branch went through");
        };
        assert!(
            matches!(&error, Error::BranchMismatch { expected, checked_out }
                if expected == "main" && checked_out == "other"),
            "{error:?}"
        );
        assert!(error.to_string().contains("git switch main"), "{error}");

        // Asking for the checked-out branch does not rebind a bound state.
        let outcome = push(&client, dir.path(), &store, &options(Some("other"))).await;
        assert!(
            matches!(outcome, Err(Error::BranchRebind { .. })),
            "{outcome:?}"
        );
        assert!(server.posted().is_empty());
    }

    #[tokio::test]
    async fn a_lost_answer_is_settled_by_the_next_push_under_the_same_key() {
        let server = fake(vec![
            Answer::Drop,
            Answer::Drop,
            Answer::Drop,
            accepted("l1", 1),
            accepted("l2", 2),
        ])
        .await;
        let Some((dir, _git, store)) = on_main(server.link(), Some("main")) else {
            return;
        };
        let client = server.client("lp_token");
        let lost = push(&client, dir.path(), &store, &options(None)).await;
        assert!(matches!(lost, Err(Error::Network(_))), "{lost:?}");
        let key = store.load().unwrap().unwrap().pending.unwrap().request_key;

        // An edit made after the request was saved is not part of its replay.
        write(dir.path(), "b.tex", b"two\n");
        let outcome = push(&client, dir.path(), &store, &options(None)).await;
        assert!(
            matches!(&outcome, Ok(PushOutcome::Pushed { label_sha, .. }) if label_sha == "l2"),
            "{outcome:?}"
        );

        let posted = server.posted();
        assert_eq!(posted.len(), 5);
        assert!(posted[..4]
            .iter()
            .all(|body| body["operation"] == key.as_str()));
        assert_eq!(posted[3], posted[0]);
        assert_eq!(posted[3]["change"]["create"].as_array().unwrap().len(), 1);
        assert_eq!(posted[3]["change"]["create"][0]["path"], "a.tex");
        assert_ne!(posted[4]["operation"], key.as_str());
        assert_eq!(posted[4]["change"]["create"].as_array().unwrap().len(), 1);
        assert_eq!(posted[4]["change"]["create"][0]["path"], "b.tex");

        let state = store.load().unwrap().unwrap();
        assert!(state.pending.is_none());
        let labels: Vec<&str> = state.pushed.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["l1", "l2"]);
        assert!(state.files.contains_key("a.tex") && state.files.contains_key("b.tex"));
    }

    #[tokio::test]
    async fn a_refusal_clears_the_attempt_without_moving_the_baseline() {
        let server = fake(vec![Answer::Reply(
            409,
            json!({"reason": "changed", "paths": ["a.tex"]}),
        )])
        .await;
        let Some((dir, _git, store)) = on_main(server.link(), Some("main")) else {
            return;
        };
        let client = server.client("lp_token");
        let outcome = push(&client, dir.path(), &store, &options(None)).await;
        assert_eq!(
            outcome.unwrap(),
            PushOutcome::Refused {
                paths: vec!["a.tex".into()]
            }
        );
        let state = store.load().unwrap().unwrap();
        assert!(state.pending.is_none());
        assert!(state.files.is_empty() && state.pushed.is_empty());
    }

    #[tokio::test]
    async fn a_failed_attempt_keeps_its_key_and_only_the_same_sign_in_may_retry() {
        let server = fake(vec![
            Answer::Reply(500, json!({"error": "boom"})),
            accepted("l1", 1),
        ])
        .await;
        let Some((dir, _git, store)) = on_main(server.link(), Some("main")) else {
            return;
        };
        let client = server.client("lp_token");
        let failed = push(&client, dir.path(), &store, &options(None)).await;
        assert!(
            matches!(failed, Err(Error::Http { status: 500, .. })),
            "{failed:?}"
        );
        let pending = store.load().unwrap().unwrap().pending.unwrap();
        assert!(!pending.actor.contains("lp_token"));

        let other = server.client("lp_other");
        let outcome = push(&other, dir.path(), &store, &options(None)).await;
        assert!(matches!(outcome, Err(Error::PendingActor)), "{outcome:?}");
        assert_eq!(server.posted().len(), 1);

        let outcome = push(&client, dir.path(), &store, &options(None)).await;
        assert!(
            matches!(outcome, Ok(PushOutcome::Pushed { .. })),
            "{outcome:?}"
        );
        let posted = server.posted();
        assert_eq!(posted[0]["operation"], pending.request_key.as_str());
        assert_eq!(posted[1], posted[0]);
        assert!(store.load().unwrap().unwrap().pending.is_none());
    }

    /// A store that reads one state and cannot write.
    struct Unwritable(State);

    impl StateStore for Unwritable {
        fn load(&self) -> Result<Option<State>, Error> {
            Ok(Some(self.0.clone()))
        }
        fn save(&self, _: &State) -> Result<(), Error> {
            Err(Error::State("the disk is full".into()))
        }
        fn clear(&self) -> Result<(), Error> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn nothing_is_sent_when_the_pending_record_cannot_be_saved() {
        let server = fake(vec![accepted("l1", 1)]).await;
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.tex", b"one\n");
        let store = Unwritable(State::new(server.link()));
        let client = server.client("lp_token");
        let outcome = push(&client, dir.path(), &store, &options(None)).await;
        assert!(matches!(outcome, Err(Error::State(_))), "{outcome:?}");
        assert!(server.posted().is_empty());
    }

    #[tokio::test]
    async fn a_pull_waits_for_the_pending_push() {
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = crate::state::FileStore::for_dir(home.path(), dir.path());
        let mut state = State::new(Link::parse("https://app.librepaper.org/docs/paper").unwrap());
        state.pending = Some(PendingPush {
            request_key: "v2.1.0".into(),
            actor: "sha256:0".into(),
            request: json!({}),
            upsert_files: BTreeMap::new(),
            deleted_paths: Vec::new(),
            commit: None,
        });
        store.save(&state).unwrap();
        // Nothing listens here, so a request would be a network error.
        let site = url::Url::parse("http://127.0.0.1:9").unwrap();
        let client = Client::new(site, librepaper_base::http::Credentials::token("lp_token"));
        let outcome = crate::pull::pull(
            &client,
            dir.path(),
            &store,
            &crate::pull::PullOptions::default(),
        )
        .await;
        assert!(matches!(outcome, Err(Error::PendingPush)), "{outcome:?}");
        assert_eq!(store.load().unwrap(), Some(state));
    }

    mod web_changed_tests {
        use super::*;
        // ------------------------------------------------------- web_changed

        use crate::fake::{self, Fake};
        use crate::pull::{pull, PullOptions, PullOutcome};
        use crate::state::FileStore;

        fn records(entries: &[(&str, &str)]) -> BTreeMap<String, FileRecord> {
            entries
                .iter()
                .map(|(path, text)| (path.to_string(), record(Kind::Text, text.as_bytes())))
                .collect()
        }

        #[tokio::test]
        async fn an_unchanged_head_answers_without_reading_the_project() {
            let home = tempfile::tempdir().unwrap();
            let store = FileStore::for_dir(home.path(), home.path());
            let fake = Fake::start().await;
            fake.name(1, fake::files(&[("a.tex", "one\n")]));
            fake.set_current(fake::files(&[("a.tex", "unnamed\n")]));
            fake.set_head(Some("h1"));
            let mut state = State::new(fake.link());
            state.pulled = Some(1);
            state.files = records(&[("a.tex", "one\n")]);
            state.web_head = Some("h1".into());

            assert!(!web_changed(&fake.client(), &store, &state).await.unwrap());
            assert_eq!(fake.count("/project"), 0);
        }

        #[tokio::test]
        async fn an_unnamed_accepted_edit_is_a_web_change() {
            let home = tempfile::tempdir().unwrap();
            let store = FileStore::for_dir(home.path(), home.path());
            let fake = Fake::start().await;
            fake.name(1, fake::files(&[("a.tex", "one\n")]));
            fake.set_current(fake::files(&[("a.tex", "unnamed\n")]));
            fake.set_head(Some("h2"));
            let mut state = State::new(fake.link());
            state.pulled = Some(1);
            state.files = records(&[("a.tex", "one\n")]);
            state.web_head = Some("h1".into());

            assert!(web_changed(&fake.client(), &store, &state).await.unwrap());
            assert_eq!(store.load().unwrap(), None, "nothing is recorded as seen");
        }

        #[tokio::test]
        async fn a_moved_head_over_the_same_files_is_recorded_and_not_read_again() {
            let home = tempfile::tempdir().unwrap();
            let store = FileStore::for_dir(home.path(), home.path());
            let fake = Fake::start().await;
            fake.name(1, fake::files(&[("a.tex", "one\r\n")]));
            fake.set_current(fake::files(&[("a.tex", "one\r\n")]));
            fake.set_head(Some("h2"));
            let mut state = State::new(fake.link());
            state.pulled = Some(1);
            state.files = records(&[("a.tex", "one\n")]);
            state.web_head = None;
            store.save(&state).unwrap();

            assert!(!web_changed(&fake.client(), &store, &state).await.unwrap());
            assert_eq!(fake.count("/project"), 1);
            let state = store.load().unwrap().unwrap();
            assert_eq!(state.web_head.as_deref(), Some("h2"));

            assert!(!web_changed(&fake.client(), &store, &state).await.unwrap());
            assert_eq!(fake.count("/project"), 1);
        }

        #[tokio::test]
        async fn a_pending_fetch_is_what_the_current_files_are_compared_with() {
            let home = tempfile::tempdir().unwrap();
            let store = FileStore::for_dir(home.path(), home.path());
            let fake = Fake::start().await;
            fake.name(1, fake::files(&[("a.tex", "two\n")]));
            fake.set_current(fake::files(&[("a.tex", "two\n")]));
            fake.set_head(Some("h2"));
            let mut state = State::new(fake.link());
            state.pulled = Some(1);
            state.files = records(&[("a.tex", "one\n")]);
            state.fetched = Some(crate::state::Fetched {
                commit: "c".into(),
                files: records(&[("a.tex", "two\n")]),
            });

            assert!(!web_changed(&fake.client(), &store, &state).await.unwrap());
        }

        #[tokio::test]
        async fn a_push_clears_the_head_and_the_pull_after_it_finds_nothing_to_write() {
            let dir = tempfile::tempdir().unwrap();
            let home = tempfile::tempdir().unwrap();
            let store = FileStore::for_dir(home.path(), dir.path());
            let fake = Fake::start().await;
            let mut state = State::new(fake.link());
            state.web_head = Some("h1".into());
            store.save(&state).unwrap();
            fake.set_head(Some("h1"));
            write(dir.path(), "new.tex", b"mine\n");
            fake.set_after_push(fake::files(&[("new.tex", "mine\n")]));

            let options = PushOptions {
                message: "Edits".into(),
                author: None,
                exclude: Vec::new(),
                branch: None,
            };
            let outcome = push(&fake.client(), dir.path(), &store, &options)
                .await
                .unwrap();
            assert!(matches!(outcome, PushOutcome::Pushed { .. }), "{outcome:?}");
            assert_eq!(store.load().unwrap().unwrap().web_head, None);

            let outcome = pull(&fake.client(), dir.path(), &store, &PullOptions::default())
                .await
                .unwrap();
            match outcome {
                PullOutcome::Folder {
                    written,
                    trashed,
                    conflicts,
                    ..
                } => {
                    assert!(written.is_empty() && trashed.is_empty() && conflicts.is_empty());
                }
                other => panic!("{other:?}"),
            }
            let state = store.load().unwrap().unwrap();
            assert_eq!(state.web_head.as_deref(), Some("h1+"));
            assert_eq!(fake.count("/project"), 1);
        }
    }
}
