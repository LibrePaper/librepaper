//! The Run directory: the folder on this computer a project's code runs in.
//!
//! The person's data and scripts live in a folder of their own, and a Quarto
//! render runs with that folder as its working directory. The companion keeps
//! one such folder per origin and project, on this computer only. These routes
//! read it, choose it with the native folder dialog, create the default one,
//! and open it in the system file manager.

use super::*;

use super::sync::{answer, refuse, Failure};

use crate::local::bindings::BindingStore;

const CHOOSE_TITLE: &str = "Choose the folder your code runs in";

#[derive(serde::Deserialize)]
struct ProjectOnly {
    project: String,
}

/// The project name of a request, which must be a valid slug.
fn project_of(raw: &str) -> Result<String, Failure> {
    store::valid_project(raw).ok_or_else(|| refuse(400, "a run directory needs a valid project"))
}

fn status_of(inner: &Inner, origin: Option<&str>, project: &str) -> Value {
    let status = inner
        .bindings
        .run_dir_status(origin.unwrap_or_default(), project);
    json!({
        "project": project,
        "path": status.path.map(|path| path.display().to_string()),
        "exists": status.exists,
        "suggested": status.suggested.display().to_string(),
    })
}

pub(super) async fn handle_status(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let Some(project) = query_value(request.uri(), "project") else {
        return write_json(400, &json!({"error": "a run directory needs a project"}));
    };
    answer(project_of(&project).map(|project| status_of(inner, origin, &project)))
}

pub(super) async fn handle_choose(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_body::<ProjectOnly>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    answer(choose(inner, origin, &body.project).await)
}

async fn choose(inner: &Inner, origin: Option<&str>, project: &str) -> Result<Value, Failure> {
    let project = project_of(project)?;
    let Ok(_dialog) = inner.folder_dialog.try_lock() else {
        return Err((
            409,
            json!({"error": "A folder chooser is already open on this computer."}),
        ));
    };
    let chosen = match crate::local::folder::choose_directory_with_title(CHOOSE_TITLE).await {
        Ok(chosen) => chosen,
        Err(why) if why == crate::local::folder::CANCELLED => {
            return Ok(status_of(inner, origin, &project));
        }
        Err(why) => return Err(refuse(400, &why)),
    };
    inner
        .bindings
        .set_run_dir(origin.unwrap_or_default(), &project, &chosen)
        .map_err(|why| refuse(400, &why))?;
    Ok(status_of(inner, origin, &project))
}

pub(super) async fn handle_default(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_body::<ProjectOnly>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    answer(use_default(inner, origin, &body.project))
}

fn use_default(inner: &Inner, origin: Option<&str>, project: &str) -> Result<Value, Failure> {
    let project = project_of(project)?;
    place(
        inner,
        origin,
        &project,
        &BindingStore::default_run_dir(&project),
    )
}

/// Creates `folder` if needed and stores it as the project's Run directory.
fn place(
    inner: &Inner,
    origin: Option<&str>,
    project: &str,
    folder: &Path,
) -> Result<Value, Failure> {
    std::fs::create_dir_all(folder)
        .map_err(|error| refuse(500, &format!("could not create the folder: {error}")))?;
    inner
        .bindings
        .set_run_dir(origin.unwrap_or_default(), project, folder)
        .map_err(|why| refuse(400, &why))?;
    Ok(status_of(inner, origin, project))
}

pub(super) async fn handle_open(
    inner: &Arc<Inner>,
    headers: &HeaderMap,
    origin: Option<&str>,
    request: Request<Body>,
) -> Reply {
    if let Err(response) = authenticate(inner, headers, origin) {
        return response;
    }
    let body = match read_json_body::<ProjectOnly>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    answer(open(inner, origin, &body.project))
}

fn open(inner: &Inner, origin: Option<&str>, project: &str) -> Result<Value, Failure> {
    let project = project_of(project)?;
    let status = inner
        .bindings
        .run_dir_status(origin.unwrap_or_default(), &project);
    let folder = match status.path {
        Some(path) if status.exists => path,
        _ => {
            return Err(refuse(
                404,
                "this project has no run directory on this computer",
            ))
        }
    };
    show_in_file_manager(&folder)
        .map_err(|error| refuse(500, &format!("could not open the folder: {error}")))?;
    Ok(status_of(inner, origin, &project))
}

/// Opens `folder` in the system file manager without waiting for it.
fn show_in_file_manager(folder: &Path) -> std::io::Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    let mut child = std::process::Command::new(program)
        .arg(folder)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    // Reap it in the background so no zombie is left behind.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{bearer, inner as build_inner};
    use super::*;

    const ORIGIN: &str = "https://paper.example";

    async fn fixture() -> (Arc<Inner>, tempfile::TempDir, tempfile::TempDir) {
        let state_home = tempfile::tempdir().unwrap();
        let cache_home = tempfile::tempdir().unwrap();
        let inner = build_inner(state_home.path(), cache_home.path()).await;
        (inner, state_home, cache_home)
    }

    fn get(uri: &str) -> Request<Body> {
        Request::get(uri).body(Body::empty()).unwrap()
    }

    fn post(uri: &str, body: Value) -> Request<Body> {
        Request::post(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn body_of(response: Reply) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    #[tokio::test]
    async fn status_with_nothing_set_has_no_path_and_a_suggestion() {
        let (inner, _state, _cache) = fixture().await;
        let response = handle_status(
            &inner,
            &bearer(&inner),
            Some(ORIGIN),
            get("/run-directory?project=paper-abc"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = body_of(response).await;
        assert_eq!(body["project"], "paper-abc");
        assert!(body["path"].is_null(), "{body}");
        assert_eq!(body["exists"], false);
        assert!(body["suggested"].as_str().unwrap().ends_with("paper-abc"));
    }

    #[tokio::test]
    async fn a_default_folder_is_created_and_then_reported() {
        let (inner, _state, _cache) = fixture().await;
        let home = tempfile::tempdir().unwrap();
        let folder = home.path().join("LibrePaper").join("paper-abc");
        let body = place(&inner, Some(ORIGIN), "paper-abc", &folder).unwrap();
        assert_eq!(body["exists"], true, "{body}");
        assert!(folder.is_dir());
        let response = handle_status(
            &inner,
            &bearer(&inner),
            Some(ORIGIN),
            get("/run-directory?project=paper-abc"),
        )
        .await;
        let status = body_of(response).await;
        assert_eq!(status["exists"], true);
        let canonical = std::fs::canonicalize(&folder).unwrap();
        assert_eq!(status["path"], canonical.display().to_string());
        // Another origin does not see it.
        let other = handle_status(
            &inner,
            &bearer(&inner),
            Some("https://other.example"),
            get("/run-directory?project=paper-abc"),
        )
        .await;
        assert!(body_of(other).await["path"].is_null());
    }

    #[tokio::test]
    async fn a_request_without_a_project_is_refused() {
        let (inner, _state, _cache) = fixture().await;
        let response =
            handle_status(&inner, &bearer(&inner), Some(ORIGIN), get("/run-directory")).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = handle_default(
            &inner,
            &bearer(&inner),
            Some(ORIGIN),
            post("/run-directory/default", json!({})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = handle_default(
            &inner,
            &bearer(&inner),
            Some(ORIGIN),
            post("/run-directory/default", json!({"project": ""})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = handle_open(
            &inner,
            &bearer(&inner),
            Some(ORIGIN),
            post("/run-directory/open", json!({"project": "../bad"})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn open_with_no_folder_is_a_404() {
        let (inner, _state, _cache) = fixture().await;
        let response = handle_open(
            &inner,
            &bearer(&inner),
            Some(ORIGIN),
            post("/run-directory/open", json!({"project": "paper"})),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_missing_or_wrong_origin_is_refused() {
        let (inner, _state, _cache) = fixture().await;
        for origin in [None, Some("not an origin")] {
            let response = handle_status(
                &inner,
                &bearer(&inner),
                origin,
                get("/run-directory?project=paper"),
            )
            .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
    }
}
