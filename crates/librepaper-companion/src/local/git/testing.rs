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
        let grant = self
            .manager
            .insert_grant_for_test(ORIGIN, PROJECT, &self.remote);
        self.ok(
            "connect",
            json!({"connection_id": CONNECTION_ID, "remote": {"grant": grant}, "branch": "main"}),
        )
        .await;
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
    assert_eq!(fx.ok("push", json!({"commit": one})).await["pushed"], true);
    let two = fx.commit(&[&one], &[("f.txt", "two")]).await;
    assert_eq!(fx.ok("push", json!({"commit": two})).await["pushed"], true);
    assert_eq!(
        git_in(&fx.remote, &["rev-parse", "refs/heads/main"], ""),
        two
    );
    let fetched = fx.ok("fetch", json!({"also": []})).await;
    assert_eq!(fetched["head"], json!(two));

    let stale = fx.commit(&[], &[("f.txt", "unrelated")]).await;
    let error = fx.call("push", json!({"commit": stale})).await.unwrap_err();
    assert_eq!(error.status, 409);
    assert_eq!(error.kind, Some("moved"));
    assert_eq!(
        git_in(&fx.remote, &["rev-parse", "refs/heads/main"], ""),
        two
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
        fx.ok("push", json!({"commit": commit})).await["pushed"],
        true
    );
    assert!(!marker.exists(), "the sidecar's push ran a user hook");
}
