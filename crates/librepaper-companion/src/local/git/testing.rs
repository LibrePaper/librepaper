//! Tests for the Git operations, run against real `git` and temporary
//! directories.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};

use serde_json::{json, Value};

use super::{GitManager, AMBIENT};

const ORIGIN: &str = "https://paper.example";
const PROJECT: &str = "paper";
const CONNECTION_ID: &str = "0b0a7b1c-5d2e-4f3a-8c1d-2e3f4a5b6c7d";

/// A `git` command for test setup, away from any user configuration.
fn git_cmd(dir: &Path) -> StdCommand {
    let mut command = StdCommand::new("git");
    command.arg("-C").arg(dir);
    for name in AMBIENT {
        command.env_remove(name);
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t");
    command
}

/// Runs a setup command and returns its trimmed stdout.
fn git_in(dir: &Path, args: &[&str], stdin: &str) -> String {
    use std::io::Write;
    let mut child = git_cmd(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("git starts");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(stdin.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("git finishes");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

struct Fixture {
    _state: tempfile::TempDir,
    scratch: tempfile::TempDir,
    manager: GitManager,
    remote: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::build(|_| Vec::new())
    }

    /// `env` gets the scratch directory and returns variables to set on every
    /// Git command the manager runs.
    fn build(env: impl FnOnce(&Path) -> Vec<(String, OsString)>) -> Self {
        let state = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        git_in(
            scratch.path(),
            &[
                "init",
                "--bare",
                "--quiet",
                "--initial-branch=main",
                "remote.git",
            ],
            "",
        );
        let remote = scratch.path().join("remote.git").canonicalize().unwrap();
        let manager = GitManager::new(state.path()).with_test_env(env(scratch.path()));
        Fixture {
            _state: state,
            scratch,
            manager,
            remote,
        }
    }

    async fn call(&self, op: &str, mut body: Value) -> Result<Value, super::GitError> {
        body["project"] = json!(PROJECT);
        self.manager
            .call(ORIGIN, op, body.to_string().as_bytes())
            .await
    }

    async fn ok(&self, op: &str, body: Value) -> Value {
        self.call(op, body)
            .await
            .unwrap_or_else(|error| panic!("{op}: {} {}", error.status, error.message))
    }

    async fn connect(&self) {
        self.connect_to(&self.remote).await;
    }

    /// Connects the project to `folder` through a grant.
    async fn connect_to(&self, folder: &Path) {
        let grant = self.manager.insert_grant_for_test(ORIGIN, PROJECT, folder);
        self.ok(
            "connect",
            json!({"connection_id": CONNECTION_ID, "remote": {"grant": grant}, "branch": "main"}),
        )
        .await;
    }

    /// The head of the connected branch, fetched into the private repository.
    async fn fetched_head(&self) -> String {
        let fetched = self.ok("fetch", json!({"also": []})).await;
        fetched["head"].as_str().unwrap().to_string()
    }

    async fn blob(&self, text: &str) -> String {
        let answer = self
            .ok("put-blobs", json!({"blobs": [{"text": text}]}))
            .await;
        answer["ids"][0].as_str().unwrap().to_string()
    }

    async fn tree_of(&self, files: &[(&str, &str)]) -> String {
        let mut edits = Vec::new();
        for (path, text) in files {
            let id = self.blob(text).await;
            edits.push(json!({"path": path, "id": id}));
        }
        let answer = self
            .ok("tree-edit", json!({"base": null, "edits": edits}))
            .await;
        answer["tree"].as_str().unwrap().to_string()
    }

    async fn commit(&self, parents: &[&str], files: &[(&str, &str)]) -> String {
        let tree = self.tree_of(files).await;
        let answer = self
            .ok(
                "commit-tree",
                json!({"tree": tree, "parents": parents, "message": "m\n", "author": null}),
            )
            .await;
        answer["commit"].as_str().unwrap().to_string()
    }

    async fn blob_text(&self, id: &str) -> String {
        use base64::Engine as _;
        let answer = self.ok("blobs", json!({"ids": [id]})).await;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(answer["blobs"][id].as_str().unwrap())
            .unwrap();
        String::from_utf8(bytes).unwrap()
    }
}

#[tokio::test]
async fn the_same_inputs_make_the_same_commit() {
    let fx = Fixture::new();
    fx.connect().await;
    let ids = fx
        .ok(
            "put-blobs",
            json!({"blobs": [{"text": "hello\n"}, {"base64": "AAEC"}]}),
        )
        .await;
    let again = fx
        .ok(
            "put-blobs",
            json!({"blobs": [{"text": "hello\n"}, {"base64": "AAEC"}]}),
        )
        .await;
    assert_eq!(ids, again);
    let (text, binary) = (
        ids["ids"][0].as_str().unwrap(),
        ids["ids"][1].as_str().unwrap(),
    );
    let edits = json!([
        {"path": "a.txt", "id": text},
        {"path": "dir/b.bin", "id": binary, "mode": "100755"},
    ]);
    let first = fx
        .ok("tree-edit", json!({"base": null, "edits": edits}))
        .await;
    let second = fx
        .ok("tree-edit", json!({"base": null, "edits": edits}))
        .await;
    assert_eq!(first, second);
    let tree = first["tree"].as_str().unwrap();
    let make = || json!({"tree": tree, "parents": [], "message": "m\n", "author": null});
    let one = fx.ok("commit-tree", make()).await;
    let two = fx.ok("commit-tree", make()).await;
    assert_eq!(one, two);

    let listing = fx.ok("tree", json!({"commit": one["commit"]})).await;
    assert_eq!(listing["tree"], first["tree"]);
    let entries = listing["entries"].as_array().unwrap();
    let find = |path: &str| entries.iter().find(|e| e["path"] == path).unwrap();
    assert_eq!(find("a.txt")["mode"], "100644");
    assert_eq!(find("a.txt")["size"], 6);
    assert_eq!(find("dir")["type"], "tree");
    assert!(find("dir")["size"].is_null());
    assert_eq!(find("dir/b.bin")["mode"], "100755");
    assert_eq!(fx.blob_text(text).await, "hello\n");
}

#[tokio::test]
async fn a_tree_edit_keeps_the_existing_mode_and_can_delete() {
    let fx = Fixture::new();
    fx.connect().await;
    let one = fx.blob("one").await;
    let two = fx.blob("two").await;
    let base = fx
        .ok(
            "tree-edit",
            json!({"base": null, "edits": [
                {"path": "run.sh", "id": one, "mode": "100755"},
                {"path": "gone.txt", "id": one},
            ]}),
        )
        .await;
    let edited = fx
        .ok(
            "tree-edit",
            json!({"base": base["tree"], "edits": [
                {"path": "run.sh", "id": two},
                {"path": "gone.txt", "delete": true},
            ]}),
        )
        .await;
    let listing = fx.ok("tree", json!({"commit": edited["tree"]})).await;
    let entries = listing["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["path"], "run.sh");
    assert_eq!(entries[0]["mode"], "100755");
    assert_eq!(entries[0]["id"], json!(two));
}

#[tokio::test]
async fn tree_edit_refuses_paths_that_escape_the_repository() {
    let fx = Fixture::new();
    fx.connect().await;
    let id = fx.blob("x").await;
    for path in ["../x", ".git/config", "/etc/passwd", "a/../../b", "a\\b"] {
        let error = fx
            .call(
                "tree-edit",
                json!({"base": null, "edits": [{"path": path, "id": id}]}),
            )
            .await
            .unwrap_err();
        assert_eq!(error.status, 400, "{path}");
    }
    let error = fx
        .call(
            "tree-edit",
            json!({"base": null, "edits": [{"path": "a", "id": "0".repeat(40)}]}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, Some("missing"));
}

#[tokio::test]
async fn a_merge_is_clean_when_the_edits_are_on_different_lines() {
    let fx = Fixture::new();
    fx.connect().await;
    let base = fx.commit(&[], &[("f.txt", "a\nb\nc\n")]).await;
    let ours = fx.commit(&[&base], &[("f.txt", "A\nb\nc\n")]).await;
    let theirs = fx.commit(&[&base], &[("f.txt", "a\nb\nC\n")]).await;
    let merged = fx
        .ok("merge", json!({"ours": ours, "theirs": theirs}))
        .await;
    assert_eq!(merged["clean"], true);
    let listing = fx.ok("tree", json!({"commit": merged["tree"]})).await;
    let id = listing["entries"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(fx.blob_text(&id).await, "A\nb\nC\n");
}

#[tokio::test]
async fn a_merge_reports_each_conflict_with_its_three_stages() {
    let fx = Fixture::new();
    fx.connect().await;
    let base = fx.commit(&[], &[("f.txt", "a\nb\nc\n")]).await;
    let ours = fx.commit(&[&base], &[("f.txt", "A\nb\nc\n")]).await;
    let theirs = fx.commit(&[&base], &[("f.txt", "X\nb\nc\n")]).await;
    let merged = fx
        .ok("merge", json!({"ours": ours, "theirs": theirs}))
        .await;
    assert_eq!(merged["clean"], false);
    let conflicts = merged["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1);
    let conflict = &conflicts[0];
    assert_eq!(conflict["path"], "f.txt");
    assert_eq!(conflict["mode"], "100644");
    assert_eq!(
        fx.blob_text(conflict["base"].as_str().unwrap()).await,
        "a\nb\nc\n"
    );
    assert_eq!(
        fx.blob_text(conflict["ours"].as_str().unwrap()).await,
        "A\nb\nc\n"
    );
    assert_eq!(
        fx.blob_text(conflict["theirs"].as_str().unwrap()).await,
        "X\nb\nc\n"
    );
}

#[tokio::test]
async fn a_rename_with_an_edit_conflicts_with_an_edit_at_the_old_path() {
    let fx = Fixture::new();
    fx.connect().await;
    let base_text = "1\n2\n3\n4\n5\n6\n7\n8\n";
    let base = fx.commit(&[], &[("x.txt", base_text)]).await;
    let ours = fx
        .commit(&[&base], &[("x.txt", "one\n2\n3\n4\n5\n6\n7\n8\n")])
        .await;
    let theirs = fx
        .commit(&[&base], &[("y.txt", "1\n2\n3\n4\n5\n6\n7\neight\n")])
        .await;
    let merged = fx
        .ok("merge", json!({"ours": ours, "theirs": theirs}))
        .await;
    assert_eq!(merged["clean"], false);
    let conflicts = merged["conflicts"].as_array().unwrap();
    let conflict = conflicts.iter().find(|c| c["path"] == "x.txt").unwrap();
    assert!(conflict["base"].is_string());
    assert!(conflict["ours"].is_string());
    assert!(conflict["theirs"].is_null());
    let listing = fx.ok("tree", json!({"commit": merged["tree"]})).await;
    let paths: Vec<&str> = listing["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["path"].as_str())
        .collect();
    assert!(paths.contains(&"y.txt"), "{paths:?}");
}

#[tokio::test]
async fn a_renamed_and_edited_asset_conflicts_with_an_edit_at_the_old_path() {
    let fx = Fixture::new();
    fx.connect().await;
    let base = fx
        .commit(&[], &[("fig.pdf", "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\n")])
        .await;
    let ours = fx
        .commit(&[&base], &[("fig.pdf", "L1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\n")])
        .await;
    let theirs = fx
        .commit(&[&base], &[("img.pdf", "l1\nl2\nl3\nl4\nl5\nl6\nl7\nL8\n")])
        .await;
    let merged = fx
        .ok("merge", json!({"ours": ours, "theirs": theirs}))
        .await;
    assert_eq!(merged["clean"], false);
    assert!(merged["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["path"] == "fig.pdf"));
}

#[tokio::test]
async fn a_delete_against_an_edit_leaves_one_stage_empty() {
    let fx = Fixture::new();
    fx.connect().await;
    let base = fx
        .commit(&[], &[("f.txt", "a\n"), ("keep.txt", "k\n")])
        .await;
    let ours = fx
        .commit(&[&base], &[("f.txt", "changed\n"), ("keep.txt", "k\n")])
        .await;
    let theirs = fx.commit(&[&base], &[("keep.txt", "k\n")]).await;
    let merged = fx
        .ok("merge", json!({"ours": ours, "theirs": theirs}))
        .await;
    assert_eq!(merged["clean"], false);
    let conflict = &merged["conflicts"][0];
    assert_eq!(conflict["path"], "f.txt");
    assert!(conflict["base"].is_string());
    assert!(conflict["ours"].is_string());
    assert!(conflict["theirs"].is_null());
}

#[tokio::test]
async fn a_diff_lists_added_modified_and_deleted_files() {
    let fx = Fixture::new();
    fx.connect().await;
    let from = fx
        .tree_of(&[("a.txt", "1"), ("b.txt", "2"), ("c.txt", "3")])
        .await;
    let to = fx
        .tree_of(&[("a.txt", "1x"), ("b.txt", "2"), ("d.txt", "4")])
        .await;
    let answer = fx.ok("diff", json!({"from": from, "to": to})).await;
    let changes = answer["changes"].as_array().unwrap();
    let by = |path: &str| changes.iter().find(|c| c["path"] == path).unwrap();
    assert_eq!(changes.len(), 3);
    assert_eq!(by("a.txt")["status"], "M");
    assert!(by("a.txt")["from"].is_string() && by("a.txt")["to"].is_string());
    assert_eq!(by("c.txt")["status"], "D");
    assert!(by("c.txt")["to"].is_null());
    assert_eq!(by("d.txt")["status"], "A");
    assert!(by("d.txt")["from"].is_null());
}

#[tokio::test]
async fn ancestry_follows_the_parents() {
    let fx = Fixture::new();
    fx.connect().await;
    let one = fx.commit(&[], &[("f.txt", "1")]).await;
    let two = fx.commit(&[&one], &[("f.txt", "2")]).await;
    let yes = fx
        .ok("ancestor", json!({"ancestor": one, "descendant": two}))
        .await;
    let no = fx
        .ok("ancestor", json!({"ancestor": two, "descendant": one}))
        .await;
    assert_eq!(yes["is_ancestor"], true);
    assert_eq!(no["is_ancestor"], false);
}

#[tokio::test]
async fn a_given_author_is_kept_and_the_committer_falls_back_to_it() {
    let fx = Fixture::new();
    fx.connect().await;
    let tree = fx.tree_of(&[("f.txt", "1")]).await;
    let made = fx
        .ok(
            "commit-tree",
            json!({"tree": tree, "parents": [], "message": "hi\n", "author": {"name": "Ann", "email": "ann@example.com"}}),
        )
        .await;
    let commit = made["commit"].as_str().unwrap();
    let repo = super::storage::conn_for(&fx.manager.root, ORIGIN, PROJECT).repo;
    let shown = git_in(&repo, &["cat-file", "-p", commit], "");
    assert!(shown.contains("author Ann <ann@example.com>"), "{shown}");
}

#[tokio::test]
async fn refs_keep_and_release_an_operations_commits() {
    let fx = Fixture::new();
    fx.connect().await;
    let one = fx.commit(&[], &[("f.txt", "1")]).await;
    let operation = "v2.1700000000000.00112233445566778899aabbccddeeff";
    fx.ok(
        "keep",
        json!({"operation": operation, "refs": {"base": one, "local": one}}),
    )
    .await;
    let repo = super::storage::conn_for(&fx.manager.root, ORIGIN, PROJECT).repo;
    let listing = || {
        git_in(
            &repo,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/librepaper/ops/",
            ],
            "",
        )
    };
    let kept = listing();
    assert!(
        kept.contains(&format!("refs/librepaper/ops/{operation}/base")),
        "{kept}"
    );
    assert!(
        kept.contains(&format!("refs/librepaper/ops/{operation}/local")),
        "{kept}"
    );
    fx.ok("release", json!({"operation": operation})).await;
    assert_eq!(listing(), "");
    let error = fx
        .call("keep", json!({"operation": "../x", "refs": {}}))
        .await
        .unwrap_err();
    assert_eq!(error.status, 400);
}

#[tokio::test]
async fn known_assets_come_from_the_table_and_must_still_exist() {
    use sha2::{Digest, Sha256};
    let fx = Fixture::new();
    fx.connect().await;
    let digest = hex::encode(Sha256::digest(b"hello\n"));
    let wrong = "0".repeat(64);
    let error = fx
        .call(
            "put-blobs",
            json!({"blobs": [{"text": "hello\n", "sha256": wrong}]}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.status, 400);
    let put = fx
        .ok(
            "put-blobs",
            json!({"blobs": [{"text": "hello\n", "sha256": digest}]}),
        )
        .await;
    let known = fx
        .ok("known-assets", json!({"sha256": [digest, "1".repeat(64)]}))
        .await;
    assert_eq!(known["known"][&digest], put["ids"][0]);
    assert!(known["known"].get("1".repeat(64)).is_none());
}

#[tokio::test]
async fn the_journal_round_trips_and_forget_keeps_the_repository() {
    let fx = Fixture::new();
    fx.connect().await;
    assert_eq!(fx.ok("journal", json!({})).await["entries"], json!([]));
    fx.ok(
        "journal",
        json!({"upsert": {"id": "a", "op": "x", "phase": "planned"}}),
    )
    .await;
    assert_eq!(
        fx.ok("journal", json!({})).await["entries"][0]["phase"],
        "planned"
    );
    let status = fx.ok("status", json!({})).await;
    assert_eq!(status["connected"], true);
    assert_eq!(status["connection_id"], CONNECTION_ID);
    assert_eq!(status["branch"], "main");
    fx.ok("forget", json!({})).await;
    assert_eq!(fx.ok("status", json!({})).await["connected"], false);
    assert_eq!(
        fx.ok("journal", json!({})).await["entries"][0]["phase"],
        "planned"
    );
    assert_eq!(
        fx.call("tree", json!({"commit": "0".repeat(40)}))
            .await
            .unwrap_err()
            .status,
        404
    );
}

#[tokio::test]
async fn journal_changes_are_atomic_per_entry() {
    let fx = Fixture::new();
    fx.connect().await;
    let (one, two) = tokio::join!(
        fx.ok("journal", json!({"upsert": {"id": "a", "phase": "one"}})),
        fx.ok("journal", json!({"upsert": {"id": "b", "phase": "two"}})),
    );
    assert_eq!(one["ok"], true);
    assert_eq!(two["ok"], true);
    let entries = fx.ok("journal", json!({})).await["entries"].clone();
    assert_eq!(entries.as_array().unwrap().len(), 2);

    fx.ok("journal", json!({"upsert": {"id": "a", "phase": "done"}}))
        .await;
    let entries = fx.ok("journal", json!({})).await["entries"].clone();
    assert_eq!(entries.as_array().unwrap().len(), 2);
    let a = entries
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "a")
        .unwrap();
    assert_eq!(a["phase"], "done");

    fx.ok("journal", json!({"remove": "a"})).await;
    fx.ok("journal", json!({"remove": "missing"})).await;
    let entries = fx.ok("journal", json!({})).await["entries"].clone();
    assert_eq!(entries, json!([{"id": "b", "phase": "two"}]));

    for body in [
        json!({"upsert": {"phase": "no id"}}),
        json!({"upsert": {"id": "x".repeat(129)}}),
        json!({"upsert": {"id": "a"}, "remove": "a"}),
        json!({"entries": []}),
    ] {
        assert_eq!(fx.call("journal", body).await.unwrap_err().status, 400);
    }
    let huge = json!({"upsert": {"id": "big", "text": "x".repeat(1024 * 1024)}});
    assert_eq!(fx.call("journal", huge).await.unwrap_err().status, 413);
}

#[tokio::test]
async fn a_push_that_is_behind_the_remote_is_moved() {
    let fx = Fixture::new();
    fx.connect().await;
    let one = fx.commit(&[], &[("f.txt", "one")]).await;
    assert_eq!(
        fx.ok("push", push_body(&one, None, CONNECTION_ID)).await["pushed"],
        true
    );
    let two = fx.commit(&[&one], &[("f.txt", "two")]).await;
    assert_eq!(
        fx.ok("push", push_body(&two, Some(&one), CONNECTION_ID))
            .await["pushed"],
        true
    );
    assert_eq!(
        git_in(&fx.remote, &["rev-parse", "refs/heads/main"], ""),
        two
    );
    let fetched = fx.ok("fetch", json!({"also": []})).await;
    assert_eq!(fetched["head"], json!(two));

    let stale = fx.commit(&[&one], &[("f.txt", "unrelated")]).await;
    let error = fx
        .call("push", push_body(&stale, Some(&one), CONNECTION_ID))
        .await
        .unwrap_err();
    assert_eq!(error.status, 409);
    assert_eq!(error.kind, Some("moved"));
    assert_eq!(
        git_in(&fx.remote, &["rev-parse", "refs/heads/main"], ""),
        two
    );
}

fn push_body(commit: &str, expected: Option<&str>, connection_id: &str) -> Value {
    json!({"commit": commit, "expected": expected, "connection_id": connection_id})
}

/// An ordinary repository in `scratch` with `a.txt` committed on main.
fn ordinary_folder(scratch: &Path, name: &str, text: &str) -> PathBuf {
    let folder = scratch.join(name);
    std::fs::create_dir(&folder).unwrap();
    git_in(&folder, &["init", "--quiet", "--initial-branch=main"], "");
    std::fs::write(folder.join("a.txt"), text).unwrap();
    git_in(&folder, &["add", "a.txt"], "");
    git_in(&folder, &["commit", "--quiet", "-m", "one"], "");
    folder
}

#[tokio::test]
async fn a_push_updates_a_local_folder_with_the_branch_checked_out() {
    let fx = Fixture::new();
    let folder = ordinary_folder(fx.scratch.path(), "work", "one\n");
    fx.connect_to(&folder).await;
    let head = fx.fetched_head().await;
    let next = fx.commit(&[&head], &[("a.txt", "two\n")]).await;
    assert_eq!(
        fx.ok("push", push_body(&next, Some(&head), CONNECTION_ID))
            .await["pushed"],
        true
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("a.txt")).unwrap(),
        "two\n"
    );
    assert_eq!(git_in(&folder, &["rev-parse", "refs/heads/main"], ""), next);
}

#[tokio::test]
async fn a_push_into_an_empty_local_folder_succeeds() {
    let fx = Fixture::new();
    let folder = fx.scratch.path().join("empty");
    std::fs::create_dir(&folder).unwrap();
    git_in(&folder, &["init", "--quiet", "--initial-branch=main"], "");
    fx.connect_to(&folder).await;
    let one = fx.commit(&[], &[("a.txt", "one\n")]).await;
    assert_eq!(
        fx.ok("push", push_body(&one, None, CONNECTION_ID)).await["pushed"],
        true
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("a.txt")).unwrap(),
        "one\n"
    );
}

#[tokio::test]
async fn a_push_into_a_folder_with_uncommitted_changes_is_refused() {
    let fx = Fixture::new();
    let folder = ordinary_folder(fx.scratch.path(), "dirty", "one\n");
    fx.connect_to(&folder).await;
    let head = fx.fetched_head().await;
    let next = fx.commit(&[&head], &[("a.txt", "two\n")]).await;
    std::fs::write(folder.join("a.txt"), "edited\n").unwrap();
    let error = fx
        .call("push", push_body(&next, Some(&head), CONNECTION_ID))
        .await
        .unwrap_err();
    assert_eq!(error.kind, Some("dirty"), "{}", error.message);
    assert_eq!(
        std::fs::read_to_string(folder.join("a.txt")).unwrap(),
        "edited\n"
    );
}

#[tokio::test]
async fn a_push_with_a_stale_lease_is_moved() {
    let fx = Fixture::new();
    fx.connect().await;
    let one = fx.commit(&[], &[("f.txt", "one")]).await;
    fx.ok("push", push_body(&one, None, CONNECTION_ID)).await;
    let two = fx.commit(&[&one], &[("f.txt", "two")]).await;
    let three = fx.commit(&[&one], &[("f.txt", "three")]).await;

    // The right expected head succeeds.
    assert_eq!(
        fx.ok("push", push_body(&two, Some(&one), CONNECTION_ID))
            .await["pushed"],
        true
    );
    // Somebody pushed meanwhile: the lease names `one`, the remote is at `two`.
    let error = fx
        .call("push", push_body(&three, Some(&one), CONNECTION_ID))
        .await
        .unwrap_err();
    assert_eq!((error.status, error.kind), (409, Some("moved")));
    assert_eq!(
        git_in(&fx.remote, &["rev-parse", "refs/heads/main"], ""),
        two
    );

    // A null expected means the branch must not exist, and it does.
    let error = fx
        .call("push", push_body(&three, None, CONNECTION_ID))
        .await
        .unwrap_err();
    assert_eq!((error.status, error.kind), (409, Some("moved")));
}

#[tokio::test]
async fn a_push_does_not_recreate_a_deleted_branch() {
    let fx = Fixture::new();
    fx.connect().await;
    let one = fx.commit(&[], &[("f.txt", "one")]).await;
    fx.ok("push", push_body(&one, None, CONNECTION_ID)).await;
    let two = fx.commit(&[&one], &[("f.txt", "two")]).await;
    git_in(&fx.remote, &["update-ref", "-d", "refs/heads/main"], "");

    let error = fx
        .call("push", push_body(&two, Some(&one), CONNECTION_ID))
        .await
        .unwrap_err();
    assert_eq!((error.status, error.kind), (409, Some("moved")));
    let missing = git_cmd(&fx.remote)
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/main"])
        .output()
        .unwrap();
    assert!(!missing.status.success(), "the branch was recreated");
}

#[tokio::test]
async fn a_push_from_another_connection_is_stale() {
    let fx = Fixture::new();
    fx.connect().await;
    let one = fx.commit(&[], &[("f.txt", "one")]).await;
    let error = fx
        .call(
            "push",
            push_body(&one, None, "1c1c1c1c-5d2e-4f3a-8c1d-2e3f4a5b6c7d"),
        )
        .await
        .unwrap_err();
    assert_eq!((error.status, error.kind), (409, Some("stale-connection")));
    let missing = git_cmd(&fx.remote)
        .args(["rev-parse", "--verify", "--quiet", "refs/heads/main"])
        .output()
        .unwrap();
    assert!(!missing.status.success(), "the push went ahead");
}

#[tokio::test]
async fn a_push_that_does_not_descend_from_expected_is_refused() {
    let fx = Fixture::new();
    fx.connect().await;
    let one = fx.commit(&[], &[("f.txt", "one")]).await;
    fx.ok("push", push_body(&one, None, CONNECTION_ID)).await;
    let unrelated = fx.commit(&[], &[("f.txt", "unrelated")]).await;
    let error = fx
        .call("push", push_body(&unrelated, Some(&one), CONNECTION_ID))
        .await
        .unwrap_err();
    assert_eq!(error.status, 400);
    assert_eq!(
        git_in(&fx.remote, &["rev-parse", "refs/heads/main"], ""),
        one
    );
}

#[tokio::test]
async fn a_fetch_reports_the_extra_commits_it_could_not_find() {
    let fx = Fixture::new();
    fx.connect().await;
    let nothing = fx.ok("fetch", json!({"also": []})).await;
    assert!(nothing["head"].is_null());

    let empty = git_in(&fx.remote, &["mktree"], "");
    let head = git_in(&fx.remote, &["commit-tree", &empty, "-m", "remote"], "");
    git_in(&fx.remote, &["update-ref", "refs/heads/main", &head], "");
    let unknown = "1".repeat(40);
    let fetched = fx.ok("fetch", json!({"also": [head, unknown]})).await;
    assert_eq!(fetched["head"], json!(head));
    assert_eq!(fetched["missing"], json!([unknown]));
}

#[tokio::test]
async fn branches_lists_the_remote_and_its_default() {
    let fx = Fixture::new();
    let empty = git_in(&fx.remote, &["mktree"], "");
    let head = git_in(&fx.remote, &["commit-tree", &empty, "-m", "remote"], "");
    git_in(&fx.remote, &["update-ref", "refs/heads/main", &head], "");
    git_in(&fx.remote, &["update-ref", "refs/heads/dev", &head], "");
    let grant = fx
        .manager
        .insert_grant_for_test(ORIGIN, PROJECT, &fx.remote);
    let answer = fx.ok("branches", json!({"remote": {"grant": grant}})).await;
    assert_eq!(answer["branches"], json!(["dev", "main"]));
    assert_eq!(answer["default"], "main");
    assert!(answer["label"].as_str().unwrap().ends_with("remote.git"));
}

#[tokio::test]
async fn a_url_with_a_password_is_refused_before_anything_runs() {
    let fx = Fixture::new();
    let remote = json!({"url": "https://user:secret@example.invalid/o/r.git"});
    let error = fx
        .call("branches", json!({"remote": remote}))
        .await
        .unwrap_err();
    assert_eq!(error.status, 400);
    assert!(!error.message.contains("secret"));
    let error = fx
        .call(
            "connect",
            json!({"connection_id": CONNECTION_ID, "remote": remote, "branch": "main"}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.status, 400);
}

#[tokio::test]
async fn a_grant_belongs_to_one_page_and_project() {
    let fx = Fixture::new();
    let grant = fx
        .manager
        .insert_grant_for_test(ORIGIN, "other-project", &fx.remote);
    let error = fx
        .call(
            "connect",
            json!({"connection_id": CONNECTION_ID, "remote": {"grant": grant}, "branch": "main"}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.status, 404);
}

#[tokio::test]
async fn a_branch_name_that_looks_like_an_option_is_refused() {
    let fx = Fixture::new();
    let grant = fx
        .manager
        .insert_grant_for_test(ORIGIN, PROJECT, &fx.remote);
    let error = fx
        .call(
            "connect",
            json!({"connection_id": CONNECTION_ID, "remote": {"grant": grant}, "branch": "--upload-pack=x"}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.status, 400);
}

#[cfg(unix)]
#[tokio::test]
async fn a_pre_push_hook_from_the_users_configuration_does_not_run() {
    use std::os::unix::fs::PermissionsExt;
    let mut marker = PathBuf::new();
    let mut config = PathBuf::new();
    let fx = Fixture::build(|scratch| {
        let hooks = scratch.join("user-hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        marker = scratch.join("hook-ran");
        let hook = hooks.join("pre-push");
        std::fs::write(
            &hook,
            format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        config = scratch.join("user.gitconfig");
        std::fs::write(
            &config,
            format!("[core]\n\thooksPath = {}\n", hooks.display()),
        )
        .unwrap();
        vec![(
            "GIT_CONFIG_GLOBAL".to_string(),
            config.clone().into_os_string(),
        )]
    });

    // The control: plain Git with that configuration does run the hook.
    let control = fx.scratch.path().join("control");
    std::fs::create_dir_all(&control).unwrap();
    git_in(&control, &["init", "--quiet"], "");
    git_in(
        &control,
        &["commit", "--quiet", "--allow-empty", "-m", "x"],
        "",
    );
    let status = git_cmd(&control)
        .env("GIT_CONFIG_GLOBAL", &config)
        .args([
            "push",
            "--quiet",
            fx.remote.to_str().unwrap(),
            "HEAD:refs/heads/control",
        ])
        .status()
        .unwrap();
    assert!(status.success());
    assert!(marker.exists(), "the control push should have run the hook");
    std::fs::remove_file(&marker).unwrap();

    fx.connect().await;
    let commit = fx.commit(&[], &[("f.txt", "one")]).await;
    assert_eq!(
        fx.ok("push", push_body(&commit, None, CONNECTION_ID)).await["pushed"],
        true
    );
    assert!(!marker.exists(), "the sidecar's push ran a user hook");
}

#[tokio::test]
async fn the_users_global_attributes_do_not_change_a_private_merge() {
    let mut dirs = (PathBuf::new(), PathBuf::new());
    let fx = Fixture::build(|scratch| {
        let xdg = scratch.join("xdg");
        let home = scratch.join("home");
        std::fs::create_dir_all(xdg.join("git")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(xdg.join("git").join("attributes"), "* merge=union\n").unwrap();
        dirs = (xdg.clone(), home.clone());
        vec![
            ("XDG_CONFIG_HOME".to_string(), xdg.into_os_string()),
            ("HOME".to_string(), home.into_os_string()),
        ]
    });
    fx.connect().await;
    let base = fx.commit(&[], &[("f.txt", "a\nb\nc\n")]).await;
    let ours = fx.commit(&[&base], &[("f.txt", "A\nb\nc\n")]).await;
    let theirs = fx.commit(&[&base], &[("f.txt", "X\nb\nc\n")]).await;

    // The control: plain Git reads the same attributes file and merges cleanly,
    // so the file really does apply to merges.
    let repo = super::storage::conn_for(&fx.manager.root, ORIGIN, PROJECT).repo;
    let control = git_cmd(&repo)
        .env("XDG_CONFIG_HOME", &dirs.0)
        .env("HOME", &dirs.1)
        .args(["merge-tree", "--write-tree", &ours, &theirs])
        .output()
        .unwrap();
    assert_eq!(control.status.code(), Some(0), "the control should merge");

    let merged = fx
        .ok("merge", json!({"ours": ours, "theirs": theirs}))
        .await;
    assert_eq!(merged["clean"], false);
    assert_eq!(merged["conflicts"][0]["path"], "f.txt");
}

#[test]
fn the_private_config_is_rewritten_when_it_is_out_of_date() {
    let state = tempfile::tempdir().unwrap();
    let manager = GitManager::new(state.path());
    std::fs::create_dir_all(&manager.root).unwrap();
    let config = manager.root.join("private.gitconfig");
    std::fs::write(&config, b"").unwrap();

    let (written, hooks) = manager.runtime.support().unwrap();
    assert_eq!(written, config);
    assert!(hooks.is_dir());
    let attributes = manager.root.join("empty.gitattributes");
    assert_eq!(std::fs::read(&attributes).unwrap(), b"");
    let text = std::fs::read_to_string(&config).unwrap();
    assert_eq!(text, super::private_config(&attributes).unwrap());
    assert!(text.contains("tree = 4b825dc642cb6eb9a060e54bf8d69288fbee4904"));
}

/// A tree whose paths are raw bytes, which the JSON edits cannot carry.
fn raw_tree(repo: &Path, files: &[(&[u8], &str)]) -> String {
    use std::io::Write;
    let mut listing = Vec::new();
    for (path, text) in files {
        let id = git_in(repo, &["hash-object", "-w", "--stdin"], text);
        listing.extend_from_slice(format!("100644 blob {id}\t").as_bytes());
        listing.extend_from_slice(path);
        listing.push(0);
    }
    let mut child = git_cmd(repo)
        .args(["mktree", "-z"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("git starts");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(&listing)
        .expect("write stdin");
    let output = child.wait_with_output().expect("git finishes");
    assert!(
        output.status.success(),
        "mktree: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[tokio::test]
async fn a_path_that_is_not_utf8_is_flagged_in_the_listing() {
    let fx = Fixture::new();
    fx.connect().await;
    let repo = super::storage::conn_for(&fx.manager.root, ORIGIN, PROJECT).repo;
    let tree = raw_tree(&repo, &[(b"draft\xff.md", "x\n"), (b"good.md", "y\n")]);
    let listing = fx.ok("tree", json!({"commit": tree})).await;
    let entries = listing["entries"].as_array().unwrap();
    let bad = entries
        .iter()
        .find(|e| e["path"] == "draft\u{FFFD}.md")
        .unwrap();
    assert_eq!(bad["invalid_name"], true);
    let good = entries.iter().find(|e| e["path"] == "good.md").unwrap();
    assert!(good.get("invalid_name").is_none(), "{good}");
}

/// A non-bare repository with one commit, plus a linked worktree of it.
fn repository_with_worktree(scratch: &Path) -> (PathBuf, PathBuf) {
    let main = scratch.join("main-repo");
    std::fs::create_dir(&main).unwrap();
    git_in(&main, &["init", "--quiet", "--initial-branch=main"], "");
    git_in(
        &main,
        &["commit", "--allow-empty", "--quiet", "-m", "one"],
        "",
    );
    let linked = scratch.join("linked-tree");
    git_in(
        &main,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "side",
            linked.to_str().unwrap(),
        ],
        "",
    );
    (main.canonicalize().unwrap(), linked.canonicalize().unwrap())
}

#[tokio::test]
async fn the_chooser_accepts_a_linked_worktree() {
    let fx = Fixture::new();
    let (main, linked) = repository_with_worktree(fx.scratch.path());
    let answer = fx.manager.add_grant(ORIGIN, PROJECT, linked).await.unwrap();
    assert_eq!(answer["label"], "linked-tree");
    fx.manager.add_grant(ORIGIN, PROJECT, main).await.unwrap();
}

#[tokio::test]
async fn the_chooser_refuses_a_subfolder_of_a_working_tree() {
    let fx = Fixture::new();
    let (main, _) = repository_with_worktree(fx.scratch.path());
    let sub = main.join("sub");
    std::fs::create_dir(&sub).unwrap();
    let error = fx
        .manager
        .add_grant(ORIGIN, PROJECT, sub)
        .await
        .unwrap_err();
    assert_eq!(error.status, 400);
}

#[tokio::test]
async fn the_chooser_accepts_a_bare_repository_and_a_git_folder() {
    let fx = Fixture::new();
    let (main, _) = repository_with_worktree(fx.scratch.path());
    fx.manager
        .add_grant(ORIGIN, PROJECT, fx.remote.clone())
        .await
        .unwrap();
    fx.manager
        .add_grant(ORIGIN, PROJECT, main.join(".git"))
        .await
        .unwrap();
}
