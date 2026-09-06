//! The reader shell: the pages, the bundles they load, and the renderers they
//! render with, all compiled into the binary.
//!
//! What is embedded is a build output. `web/` holds the Svelte sources, bun
//! and vite build them into `web/dist`, and this serves whatever is there --
//! by path, rather than from a table of routes, because the names of a bundle's
//! files are decided by the bundler and change with their contents.

use std::collections::HashMap;

use include_dir::{include_dir, Dir, File};
use sha2::{Digest, Sha256};

use crate::config::Configuration;

static SHELL: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../web/dist");

/// The renderers, which are build outputs of the engine crate rather than of
/// the web build, and are addressed by a digest of their own bytes: a module
/// is fetched once per browser and cached for a year, so a fixed route that
/// outlived a rebuild would hand a stale module to a loader that no longer
/// speaks to it. The reader page is told the current URLs when it is served.
const MODULES: &[(&str, &str)] = &[
    ("markdown", "wasm/markdown.wasm"),
    // Optional: `make typst` builds it, and a build without it simply does not
    // list typst among its renderers.
    ("typst", "wasm/typst.wasm"),
];

/// The in-frame half of the reader, which the server writes into every
/// document it serves. It is built on its own and keeps a fixed name, because
/// that name is what goes into those documents.
#[allow(dead_code)]
pub const AGENT_ROUTE: &str = "/agent.js";

pub fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        // `.mjs` as well as `.js`: pdf.js ships its worker under that
        // extension and the bundler emits it under that extension, and a
        // module worker served as `application/octet-stream` is refused by
        // the browser before it runs a line.
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("map") => "application/json; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        Some("wasm") => "application/wasm",
        Some("woff2") => "font/woff2",
        Some("txt") | Some("md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[derive(Clone, Debug)]
pub struct ShellFile {
    pub kind: &'static str,
    pub body: Vec<u8>,
    /// A file whose bytes never change under this name, so it can be cached
    /// for a year rather than five minutes. Everything the bundler names for
    /// its own contents is one, and so are the renderers.
    pub immutable: bool,
}

impl ShellFile {
    /// The file as text, for the pages that are text and the tests that read
    /// them.
    #[allow(dead_code)]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

fn file(name: &str) -> Option<&'static [u8]> {
    SHELL.get_file(name).map(File::contents)
}

/// Where a renderer is served from: its name under /wasm/, stamped with a
/// digest of the bytes, so the URL changes whenever the module does.
fn module_route(name: &str, body: &[u8]) -> String {
    let digest = hex::encode(Sha256::digest(body));
    format!("/wasm/{name}.{}.wasm", &digest[..16])
}

/// Where this build serves its renderers, for the tests that fetch one and
/// for the page that loads them.
#[allow(dead_code)]
pub fn module_url(name: &str) -> Option<String> {
    let path = MODULES.iter().find(|(known, _)| *known == name)?.1;
    Some(module_route(name, file(path)?))
}

/// The compiled typst renderer, if this build has one.
pub fn typst_module() -> Option<&'static [u8]> {
    file("wasm/typst.wasm")
}

/// The source formats this process can render in a reader, and so offer an
/// editor for. Markdown always; HTML always, since its renderer is the
/// identity and needs no module at all; typst when the module was built.
pub fn renderers() -> Vec<String> {
    let mut list = vec!["markdown".to_string(), "html".to_string()];
    if typst_module().is_some() {
        list.push("typst".to_string());
    }
    list
}

/// The project README rendered into the HTML the documentation page is built
/// around. The README is the only copy of that text: it is embedded from the
/// shell directory, where the build puts it, so the page cannot drift from
/// what the repository says.
fn documentation() -> Result<String, String> {
    let source = file("README.md")
        .ok_or("missing README.md in the shell: run make build, which copies it in")?;
    let body = komodoc_engine::markdown::render_body(&String::from_utf8_lossy(source));
    // The page opens with the mark and the wordmark, as the landing page does,
    // so the README's own title would say the name twice.
    let leading = regex::Regex::new(r"(?s)^\s*<h1[^>]*>.*?</h1>").expect("a constant pattern");
    Ok(leading.replace(&body, "").to_string())
}

/// Everything the server answers with, ready to serve.
pub fn load_shell(_config: &Configuration) -> Result<HashMap<String, ShellFile>, String> {
    let prose = documentation()?;

    // The renderers first, because the reader page has to be told where they
    // are before it is served.
    let mut shell = HashMap::new();
    let mut modules = serde_json::Map::new();
    for (name, path) in MODULES {
        let Some(body) = file(path) else { continue };
        let route = module_route(name, body);
        modules.insert(name.to_string(), serde_json::Value::String(route.clone()));
        shell.insert(
            route,
            ShellFile {
                kind: "application/wasm",
                body: body.to_vec(),
                immutable: true,
            },
        );
    }
    let modules = serde_json::Value::Object(modules).to_string();

    for entry in walk(&SHELL) {
        let path = entry.path().to_string_lossy().to_string();
        // The renderers are served under their digest, and the README is the
        // documentation page's text rather than a page of its own.
        if path.starts_with("wasm/") || path == "README.md" {
            continue;
        }
        let kind = content_type(&path);
        let route = format!("/{path}");
        // The bundler names every asset for a digest of its own contents, so
        // one of those can be kept for a year; a page is rewritten in place by
        // the next build and cannot be.
        let immutable = path.starts_with("assets/") || path.starts_with("fonts/");
        let body = if kind.starts_with("text/html") {
            // The one thing a page is told when it is served rather than when
            // it is built: what this deployment holds. The prose of the
            // documentation page, and where the renderers are.
            entry
                .contents_utf8()
                .unwrap_or_default()
                .replace("__README__", &prose)
                .replace("__MODULES__", &modules)
                .into_bytes()
        } else {
            entry.contents().to_vec()
        };
        shell.insert(
            route,
            ShellFile {
                kind,
                body,
                immutable,
            },
        );
    }
    Ok(shell)
}

/// Every file in the shell, at any depth.
fn walk(dir: &'static Dir<'static>) -> Vec<&'static File<'static>> {
    let mut found: Vec<&File> = dir.files().collect();
    for child in dir.dirs() {
        found.extend(walk(child));
    }
    found
}
