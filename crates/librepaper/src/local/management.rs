//! Local-only companion controls. Remote websites never receive this page's
//! nonce or CORS access, including sites authorized to compile documents.

use std::path::Path;

use axum::body::Body;
use axum::http::{Method, Request, Response, StatusCode};

use super::pairing::PairingStore;
use super::presets::{Operation, PresetStore, WorkspaceMode};
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
    let presets = PresetStore::new(state_home);
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
            Some("preset-remove") => {
                let Some(id) = fields.get("preset") else {
                    return page(StatusCode::BAD_REQUEST, "Choose a preset to remove.");
                };
                notice = match presets.remove(id) {
                    Ok(()) => "Preset removed and its grants revoked.".into(),
                    Err(error) => error,
                };
            }
            Some("preset-revoke") => {
                let Some(id) = fields.get("grant") else {
                    return page(
                        StatusCode::BAD_REQUEST,
                        "Choose a preset permission to revoke.",
                    );
                };
                notice = match presets.revoke(id) {
                    Ok(true) => "Preset permission revoked.".into(),
                    Ok(false) => "Preset permission was already absent.".into(),
                    Err(error) => error,
                };
            }
            Some("preset-grant") => {
                let (Some(preset), Some(origin), Some(project), Some(entrypoint)) = (
                    fields.get("preset"),
                    fields.get("origin"),
                    fields.get("project"),
                    fields.get("entrypoint"),
                ) else {
                    return page(
                        StatusCode::BAD_REQUEST,
                        "Specify the preset, website, document, and source file.",
                    );
                };
                notice = match presets.grant(origin, project, preset, WorkspaceMode::Snapshot,
                    Operation::Build, entrypoint, crate::util::now_unix()) {
                    Ok(_) => "Preset build permission granted for this website, document, and source file.".into(),
                    Err(error) => error,
                };
            }
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
    for builder in caps
        .builders
        .iter()
        .filter(|builder| builder.id != "tex" && builder.id != "quarto")
    {
        tool_rows.push_str(&format!(
            "<tr><th>{}</th><td>{}</td><td>{}</td></tr>",
            escape(&builder.id),
            if builder.available {
                "Available"
            } else {
                "Unavailable"
            },
            escape(builder.version.as_deref().unwrap_or(&builder.note)),
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
    let mut preset_rows = String::new();
    for preset in presets.list() {
        let hidden = format!(
            "<input type=\"hidden\" name=\"preset\" value=\"{}\">",
            escape(&preset.id)
        );
        let grant_fields = format!(
            "{hidden}<label>Website <input name=\"origin\" type=\"url\" required placeholder=\"https://papers.example\"></label> <label>Document <input name=\"project\" required></label> <label>Source file <input name=\"entrypoint\" required placeholder=\"main.tex\"></label> "
        );
        preset_rows.push_str(&format!(
            "<li><strong>{}</strong> ({})<p>Allow this preset to build a temporary project copy for the specified document. Local source files additionally require a folder permission.</p>{}{}</li>",
            escape(&preset.display_name), escape(&preset.base_adapter),
            form(nonce, "preset-grant", "Allow builds", &grant_fields),
            form(nonce, "preset-remove", "Remove preset", &hidden),
        ));
    }
    if preset_rows.is_empty() {
        preset_rows = "<li>No local build presets configured.</li>".into();
    }
    let mut preset_grants = String::new();
    match presets.grants() {
        Ok(grants) => {
            for grant in grants {
                let hidden = format!(
                    "<input type=\"hidden\" name=\"grant\" value=\"{}\">",
                    escape(&grant.id)
                );
                preset_grants.push_str(&format!(
                    "<li>{} · {} · {} · {} {}</li>",
                    escape(&grant.preset_id),
                    escape(&grant.origin),
                    escape(&grant.project),
                    escape(&grant.entrypoint),
                    form(nonce, "preset-revoke", "Revoke", &hidden),
                ));
            }
        }
        Err(error) => preset_grants = format!("<li>{}</li>", escape(&error)),
    }
    let preset_section = format!(
        "<h2>Local build presets</h2><p>Define or update presets using <code>librepaper local preset</code> on this computer.</p><ul>{preset_rows}</ul><h3>Preset permissions</h3><ul>{preset_grants}</ul>"
    );
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
    let body = body.replace(
        "<h2>Connected documents</h2>",
        &format!("{preset_section}<h2>Connected documents</h2>"),
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

    #[tokio::test]
    async fn preset_permissions_can_only_be_granted_and_revoked_locally() {
        let temporary = tempfile::tempdir().expect("temporary state");
        let presets = PresetStore::new(temporary.path());
        let preset = serde_json::from_value(serde_json::json!({
            "id": "paper-preset", "display_name": "Paper", "base_adapter": "typst",
            "source_formats": ["typst"], "semantic_revision": 0
        }))
        .expect("preset definition");
        presets.create(preset).expect("create preset");
        for (origin, expected) in [
            ("https://papers.example", StatusCode::FORBIDDEN),
            ("http://127.0.0.1:8763", StatusCode::OK),
        ] {
            let request = Request::post(format!("{BASE_PATH}/manage"))
                .header("origin", origin)
                .body(Body::from("nonce=secret&action=preset-grant&preset=paper-preset&origin=https%3A%2F%2Fpapers.example&project=paper&entrypoint=main.typ"))
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
            assert_eq!(response.status(), expected);
            if expected == StatusCode::FORBIDDEN {
                assert!(presets.grants().expect("grants").is_empty());
            }
        }
        let grants = presets.grants().expect("grants");
        assert_eq!(grants.len(), 1);
        let request = Request::post(format!("{BASE_PATH}/manage"))
            .header("origin", "http://127.0.0.1:8763")
            .body(Body::from(format!(
                "nonce=secret&action=preset-revoke&grant={}",
                grants[0].id
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
        assert_eq!(response.status(), StatusCode::OK);
        assert!(presets.grants().expect("grants").is_empty());
    }
}
