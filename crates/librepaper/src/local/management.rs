//! Local-only companion controls. Remote websites never receive this page's
//! nonce or CORS access, including sites authorized to compile documents.

use std::path::Path;

use axum::body::Body;
use axum::http::{Method, Request, Response, StatusCode};

use super::pairing::PairingStore;
use super::protocol::BASE_PATH;
use super::service::Runner;

fn page(status: StatusCode, content: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header("content-type", "text/html; charset=utf-8")
        .header("cache-control", "no-store")
        .header("referrer-policy", "same-origin")
        .header("x-frame-options", "DENY")
        .header("cross-origin-resource-policy", "same-origin")
        .header("content-security-policy", "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'; form-action 'self'; base-uri 'none'")
        .body(Body::from(content.to_owned()))
        .expect("constant management response headers")
}

fn escape(value: &str) -> String {
    html_escape::encode_double_quoted_attribute(value).into_owned()
}

fn form(nonce: &str, action: &str, label: &str, extra: &str) -> String {
    format!(
        "<form method=\"post\" action=\"{BASE_PATH}/manage\"><input type=\"hidden\" name=\"nonce\" value=\"{}\"><input type=\"hidden\" name=\"action\" value=\"{}\">{extra}<button>{}</button></form>",
        escape(nonce), escape(action), escape(label)
    )
}

pub(super) async fn handle(
    state_home: &Path,
    port: u16,
    instance: &str,
    nonce: &str,
    runner: &dyn Runner,
    request: Request<Body>,
) -> Response<Body> {
    let store = PairingStore::new(state_home, None);
    // Embedded services belong to the server process and must never offer
    // controls that stop it or change the user's standalone installation.
    let standalone = store
        .read_service()
        .is_some_and(|state| state.instance == instance && state.pid == std::process::id());
    let mut notice = String::new();
    let mut refresh = false;
    if request.method() == Method::POST {
        let own_origin = request
            .headers()
            .get("origin")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|origin| {
                ["127.0.0.1", "localhost", "[::1]"]
                    .iter()
                    .any(|host| origin == format!("http://{host}:{port}"))
            });
        let own_site = request
            .headers()
            .get("sec-fetch-site")
            .is_none_or(|value| value == "same-origin");
        if !own_origin || !own_site {
            return page(
                StatusCode::FORBIDDEN,
                "Open the companion settings to use these controls.",
            );
        }
        let Ok(bytes) = axum::body::to_bytes(request.into_body(), 4096).await else {
            return page(
                StatusCode::PAYLOAD_TOO_LARGE,
                "The settings request is too large.",
            );
        };
        let fields: std::collections::HashMap<_, _> =
            url::form_urlencoded::parse(&bytes).into_owned().collect();
        if fields.get("nonce").map(String::as_str) != Some(nonce) {
            return page(
                StatusCode::FORBIDDEN,
                "Reload the companion settings and try again.",
            );
        }
        match fields.get("action").map(String::as_str) {
            Some("rescan") => refresh = true,
            Some("revoke") => {
                let (Some(origin), Some(project)) = (fields.get("origin"), fields.get("project"))
                else {
                    return page(StatusCode::BAD_REQUEST, "Choose a document to disconnect.");
                };
                store.revoke_one(origin, project);
                notice = "Document disconnected. New local jobs require permission again.".into();
            }
            Some("startup-enable" | "startup-disable") if standalone => {
                let enabled = fields["action"] == "startup-enable";
                notice = match super::lifecycle::set_startup(enabled) {
                    Ok(()) if enabled => "The companion will start when you log in.".into(),
                    Ok(()) => "Automatic startup is disabled.".into(),
                    Err(error) => error,
                };
            }
            Some("quit") if standalone => {
                return match super::lifecycle::request_stop(state_home) {
                    Ok(()) => page(StatusCode::OK, "<h1>Companion stopping</h1><p>You can close this window. Open the companion from LibrePaper when you need it again.</p>"),
                    Err(error) => page(StatusCode::INTERNAL_SERVER_ERROR, &escape(&error)),
                };
            }
            _ => return page(StatusCode::BAD_REQUEST, "Unknown companion setting."),
        }
    } else if request.method() != Method::GET {
        return page(
            StatusCode::METHOD_NOT_ALLOWED,
            "Use the companion settings page.",
        );
    }

    let caps = runner.capabilities(refresh).await;
    let mut tool_rows = String::new();
    for (name, tool) in [
        ("Quarto", &caps.quarto.tool),
        ("pdfLaTeX", &caps.tools.pdflatex),
        ("XeLaTeX", &caps.tools.xelatex),
        ("LuaLaTeX", &caps.tools.lualatex),
        ("BibTeX", &caps.tools.bibtex),
        ("Biber", &caps.tools.biber),
    ] {
        tool_rows.push_str(&format!(
            "<tr><th>{name}</th><td>{}</td><td>{}</td></tr>",
            if tool.available {
                "Available"
            } else {
                "Not found"
            },
            escape(tool.version.as_deref().unwrap_or(&tool.note))
        ));
    }
    let mut grants = String::new();
    for (origin, project) in store.active_pairings() {
        let extra = format!(
            "<input type=\"hidden\" name=\"origin\" value=\"{}\"><input type=\"hidden\" name=\"project\" value=\"{}\">",
            escape(&origin), escape(&project)
        );
        grants.push_str(&format!(
            "<li><strong>{}</strong> — {} {}</li>",
            escape(&project),
            escape(&origin),
            form(nonce, "revoke", "Disconnect", &extra)
        ));
    }
    if grants.is_empty() {
        grants = "<li>No documents connected. Enable local rendering in an online project to connect it.</li>".into();
    }
    let lifecycle = if standalone {
        format!(
            "<h2>Background app</h2><div class=\"actions\">{}{}{}</div>",
            form(nonce, "startup-enable", "Start at login", ""),
            form(nonce, "startup-disable", "Disable startup", ""),
            form(nonce, "quit", "Quit companion", "")
        )
    } else {
        "<p>This companion runs with your LibrePaper server. Its lifecycle is managed by that server.</p>".into()
    };
    let details = serde_json::to_string_pretty(&caps).unwrap_or_default();
    let body = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>LibrePaper companion</title><style>body{{font:16px/1.5 system-ui,sans-serif;max-width:850px;margin:32px auto;padding:0 20px}}h1,h2{{line-height:1.2}}table{{width:100%;border-collapse:collapse}}th,td{{text-align:left;padding:8px;border-bottom:1px solid #ddd}}button{{font:inherit;padding:6px 12px;cursor:pointer}}.actions{{display:flex;gap:12px;flex-wrap:wrap}}li{{margin:12px 0}}li form{{display:inline}}pre{{overflow:auto}}.notice{{font-weight:bold}}</style></head><body><h1>LibrePaper companion</h1><p>Running on this computer · version {}</p><p class=\"notice\">{}</p><h2>Local tools</h2><table><thead><tr><th>Tool</th><th>Status</th><th>Version or next step</th></tr></thead><tbody>{tool_rows}</tbody></table>{}<h2>Connected documents</h2><ul>{grants}</ul>{lifecycle}<h2>Updates</h2><p><a href=\"https://github.com/LibrePaper/librepaper/releases/latest\" target=\"_blank\" rel=\"noopener noreferrer\">Download the latest companion</a>. Installing an update preserves document permissions.</p><details><summary>Details</summary><pre>{}</pre></details></body></html>",
        escape(crate::VERSION), escape(&notice), form(nonce, "rescan", "Check tools again", ""), escape(&details)
    );
    page(StatusCode::OK, &body)
}

#[cfg(test)]
mod tests {
    use super::super::service::FakeRunner;
    use super::*;

    #[tokio::test]
    async fn management_requires_local_origin_and_nonce_before_mutation() {
        let temporary = tempfile::tempdir().expect("temporary state");
        let store = PairingStore::new(temporary.path(), None);
        let (token, _) = store
            .issue("https://papers.example", "paper", "test")
            .expect("grant");
        for (origin, nonce) in [
            ("https://papers.example", "secret"),
            ("http://127.0.0.1:8763", "wrong"),
        ] {
            let request = Request::post(format!("{BASE_PATH}/manage"))
                .header("origin", origin)
                .body(Body::from(format!(
                    "nonce={nonce}&action=revoke&origin=https%3A%2F%2Fpapers.example&project=paper"
                )))
                .expect("request");
            let response = handle(
                temporary.path(),
                8763,
                "test",
                "secret",
                &FakeRunner::default(),
                request,
            )
            .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            assert_eq!(
                store
                    .authenticate("https://papers.example", &token)
                    .as_deref(),
                Some("paper")
            );
        }
        let request = Request::post(format!("{BASE_PATH}/manage"))
            .header("origin", "http://127.0.0.1:8763")
            .header("sec-fetch-site", "same-origin")
            .body(Body::from(
                "nonce=secret&action=revoke&origin=https%3A%2F%2Fpapers.example&project=paper",
            ))
            .expect("request");
        let response = handle(
            temporary.path(),
            8763,
            "test",
            "secret",
            &FakeRunner::default(),
            request,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(store
            .authenticate("https://papers.example", &token)
            .is_none());
    }

    #[tokio::test]
    async fn embedded_management_has_no_process_controls_and_escapes_document_names() {
        let temporary = tempfile::tempdir().expect("temporary state");
        PairingStore::new(temporary.path(), None)
            .issue("https://papers.example", "<script>bad</script>", "test")
            .expect("grant");
        let response = handle(
            temporary.path(),
            8763,
            "test",
            "secret",
            &FakeRunner::default(),
            Request::get(format!("{BASE_PATH}/manage"))
                .body(Body::empty())
                .expect("request"),
        )
        .await;
        assert_eq!(response.headers()["x-frame-options"], "DENY");
        assert_eq!(
            response.headers()["cross-origin-resource-policy"],
            "same-origin"
        );
        let bytes = axum::body::to_bytes(response.into_body(), 128 * 1024)
            .await
            .expect("body");
        let html = String::from_utf8(bytes.to_vec()).expect("utf8");
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>bad"));
        assert!(!html.contains("Quit companion"));
        assert!(html.contains("managed by that server"));
    }
}
