//! Candidate preview requests exchanged with the browser-side renderer.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::peer::AutomationPeer;
use super::runner_lifecycle;

const MAX_FILES: usize = 200;
const MAX_CONTEXT_BYTES: usize = 16 * 1024;
const PREVIEW_TIMEOUT_SECS: u64 = 300;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    id: String,
    sha: String,
    size: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Settings {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    engine: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    release: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Tree {
    main: String,
    files: BTreeMap<String, Entry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    settings: Option<Settings>,
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn tree_from_snapshot(snapshot: &super::peer::Snapshot) -> Result<Tree, String> {
    if snapshot.tree.is_null() || !snapshot.tree.is_object() {
        return Err("the server snapshot has no canonical source tree".into());
    }
    serde_json::from_value(snapshot.tree.clone())
        .map_err(|err| format!("invalid canonical source tree: {err}"))
}

fn candidate_tree(mut tree: Tree, files: &Value) -> Result<Tree, String> {
    let files = files
        .as_object()
        .ok_or_else(|| "preview files must be a JSON object".to_string())?;
    if files.is_empty() || files.len() > MAX_FILES {
        return Err("preview must contain between one and 200 changed files".into());
    }
    let encoded = serde_json::to_vec(files)
        .map_err(|err| format!("could not encode preview files: {err}"))?;
    if encoded.len() > MAX_CONTEXT_BYTES {
        return Err("preview files exceed the 16 KiB relay context limit".into());
    }
    for (path, source) in files {
        if !valid_path(path) {
            return Err(format!("invalid preview file path {path:?}"));
        }
        let source = source
            .as_str()
            .ok_or_else(|| format!("preview source for {path:?} is not text"))?;
        let entry = tree
            .files
            .get_mut(path)
            .ok_or_else(|| format!("preview file {path:?} does not exist in the snapshot"))?;
        if entry.kind != "text" {
            return Err(format!("preview file {path:?} is not a text file"));
        }
        entry.sha = super::peer::source_sha(source);
        entry.size = source.len() as i64;
    }
    Ok(tree)
}

fn revision(tree: &Tree) -> Result<String, String> {
    let bytes =
        serde_json::to_vec(tree).map_err(|err| format!("could not encode preview tree: {err}"))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn preview_dir(
    peer: &AutomationPeer,
    conversation: &str,
    state_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    let inherited = std::env::var_os("LIBREPAPER_ASSISTANT_STATE_DIR").map(PathBuf::from);
    Ok(runner_lifecycle::location(
        peer.link(),
        conversation,
        state_dir.or(inherited.as_deref()),
    )?
    .directory
    .join("preview"))
}

/// Submit an isolated candidate and wait for browser diagnostics.
pub async fn preview(
    peer: &AutomationPeer,
    conversation: &str,
    state_dir: Option<&Path>,
    base_revision: &str,
    task_id: &str,
    files: Value,
) -> Result<Value, String> {
    if !valid_identifier(base_revision) || !valid_identifier(task_id) {
        return Err("invalid preview revision or task id".into());
    }
    let snapshot = peer.snapshot().await?;
    if snapshot.sha != base_revision {
        return Err("the document changed; capture a fresh base revision before previewing".into());
    }
    let tree = candidate_tree(tree_from_snapshot(&snapshot)?, &files)?;
    let candidate_revision = revision(&tree)?;
    let directory = preview_dir(peer, conversation, state_dir)?;
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|err| format!("could not create preview directory: {err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|err| format!("could not protect preview directory: {err}"))?;
    }
    let id = format!("preview-{}", hex::encode(crate::auth::random_bytes(12)));
    let request_path = directory.join(format!("{id}.request.json"));
    let result_path = directory.join(format!("{id}.result.json"));
    let request = serde_json::json!({
        "type":"preview_request", "id":id, "task_id":task_id,
        "base_revision":base_revision, "revision":candidate_revision, "files":files
    });
    let temporary = request_path.with_extension("tmp");
    tokio::fs::write(&temporary, request.to_string())
        .await
        .map_err(|err| format!("could not write preview request: {err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
            .await
            .map_err(|err| format!("could not protect preview request: {err}"))?;
    }
    tokio::fs::rename(&temporary, &request_path)
        .await
        .map_err(|err| format!("could not publish preview request: {err}"))?;
    let deadline =
        tokio::time::Instant::now() + tokio::time::Duration::from_secs(PREVIEW_TIMEOUT_SECS);
    let result = loop {
        if let Ok(raw) = tokio::fs::read_to_string(&result_path).await {
            let parsed: Value = serde_json::from_str(&raw)
                .map_err(|err| format!("invalid preview result: {err}"))?;
            let same = parsed["request_id"].as_str() == Some(id.as_str())
                && parsed["task_id"].as_str() == Some(task_id)
                && parsed["base_revision"].as_str() == Some(base_revision)
                && parsed["revision"].as_str() == Some(candidate_revision.as_str());
            if same {
                break parsed;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            break serde_json::json!({"ok":false,"error":"timed out waiting for browser preview","id":id,"task_id":task_id,"revision":candidate_revision,"base_revision":base_revision});
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    };
    let _ = tokio::fs::remove_file(&request_path).await;
    let _ = tokio::fs::remove_file(&result_path).await;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Tree {
        Tree {
            main: "main.md".into(),
            files: BTreeMap::from([
                (
                    "main.md".into(),
                    Entry {
                        kind: "text".into(),
                        id: "stable".into(),
                        sha: "old".into(),
                        size: 3,
                    },
                ),
                (
                    "fig.png".into(),
                    Entry {
                        kind: "asset".into(),
                        id: "".into(),
                        sha: "asset".into(),
                        size: 4,
                    },
                ),
            ]),
            settings: Some(Settings {
                engine: "xelatex".into(),
                release: "2025".into(),
            }),
        }
    }

    #[test]
    fn candidate_digest_preserves_ids_assets_and_settings() {
        let next =
            candidate_tree(tree(), &serde_json::json!({"main.md":"new"})).expect("candidate");
        assert_eq!(next.files["main.md"].id, "stable");
        assert_eq!(next.files["fig.png"].sha, "asset");
        assert_eq!(next.settings.as_ref().expect("settings").engine, "xelatex");
        assert_ne!(
            revision(&next).expect("digest"),
            revision(&tree()).expect("digest")
        );
    }

    #[test]
    fn candidate_rejects_new_or_unsafe_paths() {
        assert!(candidate_tree(tree(), &serde_json::json!({"new.md":"x"})).is_err());
        assert!(candidate_tree(tree(), &serde_json::json!({"../main.md":"x"})).is_err());
    }
}
