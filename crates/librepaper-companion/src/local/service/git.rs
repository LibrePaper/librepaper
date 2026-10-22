//! The `git/<op>` routes: desktop Git sync's typed primitives. This file only
//! authenticates, reads the body and shapes the answer; the work is in
//! `crate::local::git`.

use super::*;
use crate::local::git::GitError;

/// Git requests may carry file contents, so they get a larger body than the
/// 64 KiB other routes allow.
const MAX_GIT_BODY_BYTES: usize = 32 * 1024 * 1024;

pub(super) async fn handle(
    inner: &Inner,
    headers: &HeaderMap,
    origin: Option<&str>,
    op: &str,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_bytes(request, MAX_GIT_BODY_BYTES).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let origin = store::normalize_origin(origin.unwrap_or_default());
    let result = if op == "choose-local" {
        choose_local(inner, &origin, &body).await
    } else {
        inner.git.call(&origin, op, &body).await
    };
    match result {
        Ok(answer) => write_json(200, &answer),
        Err(error) => write_json(error.status, &error.to_json()),
    }
}

/// Opens the folder chooser and records the folder as a local remote.
async fn choose_local(inner: &Inner, origin: &str, body: &[u8]) -> Result<Value, GitError> {
    #[derive(serde::Deserialize)]
    struct Request {
        project: String,
    }
    let request: Request = serde_json::from_slice(body)
        .map_err(|error| GitError::bad(format!("bad JSON body: {error}")))?;
    let project = store::valid_project(&request.project)
        .ok_or_else(|| GitError::bad("A Git request needs a valid project."))?;
    let Ok(_dialog) = inner.folder_dialog.try_lock() else {
        return Err(GitError::busy(
            "A folder chooser is already open on this computer.",
        ));
    };
    // Opening a native chooser is itself the local user's explicit gesture;
    // the route is authenticated and POST-only, and the page only ever sees an
    // opaque grant id, never the path.
    let chosen = crate::local::folder::choose_git_directory(origin, &project)
        .await
        .map_err(GitError::bad)?;
    inner.git.add_grant(origin, &project, chosen).await
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{credential, inner as build_inner, PORT};
    use super::*;

    const PAGE: &str = "https://paper.example";

    fn post(inner: &Inner, op: &str, body: Value, with_credential: bool) -> Request<Body> {
        let mut builder = Request::post(format!("{BASE_PATH}/git/{op}"))
            .header("host", format!("127.0.0.1:{PORT}"))
            .header("x-librepaper-origin", PAGE)
            .header("content-type", "application/json");
        if with_credential {
            builder = builder.header("authorization", format!("Bearer {}", credential(inner)));
        }
        builder.body(Body::from(body.to_string())).unwrap()
    }

    async fn send(inner: &Arc<Inner>, request: Request<Body>) -> (u16, Value) {
        let peer: SocketAddr = "127.0.0.1:12345".parse().unwrap();
        let response = super::super::handle(State(inner.clone()), ConnectInfo(peer), request).await;
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    async fn ask(inner: &Arc<Inner>, op: &str, mut body: Value) -> (u16, Value) {
        body["project"] = json!("paper");
        send(inner, post(inner, op, body, true)).await
    }

    #[tokio::test]
    async fn status_reports_git_and_no_connection() {
        let state = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let inner = build_inner(state.path(), cache.path()).await;
        let (status, answer) = ask(&inner, "status", json!({})).await;
        assert_eq!(status, 200);
        assert_eq!(answer["connected"], false);
        assert_eq!(answer["supported"], true);
        assert!(answer["git"].as_str().unwrap().starts_with('2'));
    }

    #[tokio::test]
    async fn the_routes_need_the_credential_and_know_their_operations() {
        let state = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let inner = build_inner(state.path(), cache.path()).await;
        let (status, _) = send(
            &inner,
            post(&inner, "status", json!({"project": "paper"}), false),
        )
        .await;
        assert_eq!(status, 401);
        let (status, _) = ask(&inner, "no-such-op", json!({})).await;
        assert_eq!(status, 404);
        let (status, _) = send(&inner, post(&inner, "status", json!({}), true)).await;
        assert_eq!(status, 400);
    }

    #[tokio::test]
    async fn a_connection_round_trips_through_the_routes() {
        let state = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let remote = tempfile::tempdir().unwrap();
        let init = std::process::Command::new("git")
            .args(["init", "--bare", "--quiet"])
            .arg(remote.path())
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .unwrap();
        assert!(init.success());
        let inner = build_inner(state.path(), cache.path()).await;
        let grant =
            inner
                .git
                .insert_grant_for_test(PAGE, "paper", &remote.path().canonicalize().unwrap());

        let (status, answer) = ask(
            &inner,
            "connect",
            json!({
                "connection_id": "0b0a7b1c-5d2e-4f3a-8c1d-2e3f4a5b6c7d",
                "remote": {"grant": grant},
                "branch": "main",
            }),
        )
        .await;
        assert_eq!(status, 200, "{answer}");
        assert_eq!(answer["ok"], true);

        let (_, answer) = ask(&inner, "status", json!({})).await;
        assert_eq!(answer["connected"], true);
        assert_eq!(answer["branch"], "main");

        let (status, answer) =
            ask(&inner, "put-blobs", json!({"blobs": [{"text": "hello\n"}]})).await;
        assert_eq!(status, 200, "{answer}");
        let id = answer["ids"][0].as_str().unwrap().to_string();
        let (status, answer) = ask(&inner, "blobs", json!({"ids": [id.clone()]})).await;
        assert_eq!(status, 200, "{answer}");
        assert_eq!(answer["blobs"][&id], "aGVsbG8K");

        let (status, _) = ask(&inner, "forget", json!({})).await;
        assert_eq!(status, 200);
        let (_, answer) = ask(&inner, "status", json!({})).await;
        assert_eq!(answer["connected"], false);
    }
}
