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
    ("bibliography", "wasm/bibliography.wasm"),
    ("citations", "wasm/citations.wasm"),
    ("typst", "wasm/typst.wasm"),
];

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
    pub body: axum::body::Bytes,
    /// A precompressed HTTP representation of `body`, when the build supplied
    /// one. Renderer releases carry these so serving a large module costs no
    /// compression work per request.
    pub brotli: Option<axum::body::Bytes>,
    /// A file whose bytes never change under this name, so it can be cached
    /// for a year rather than five minutes. Everything the bundler names for
    /// its own contents is one, and so are the renderers.
    pub immutable: bool,
}

impl ShellFile {}

fn file(name: &str) -> Option<&'static [u8]> {
    SHELL.get_file(name).map(File::contents)
}

/// Where a renderer is served from: its name under /wasm/, stamped with a
/// digest of the bytes, so the URL changes whenever the module does.
fn module_route(name: &str, body: &[u8]) -> String {
    let digest = hex::encode(Sha256::digest(body));
    format!("/wasm/{name}.{}.wasm", &digest[..16])
}

/// The Typst module embedded by the build.
pub fn typst_module() -> Option<&'static [u8]> {
    file("wasm/typst.wasm")
}

/// The source formats this process can render in a reader, and so offer an
/// editor for. Markdown always; HTML always, since its renderer is the
/// identity and needs no module at all; typst when the module was built.
pub fn renderers() -> Vec<String> {
    let mut list = vec![
        "markdown".to_string(),
        "quarto".to_string(),
        "html".to_string(),
    ];
    if typst_module().is_some() {
        list.push("typst".to_string());
    }
    list
}

/// Everything the server answers with, ready to serve.
pub fn load_shell(_config: &Configuration) -> Result<HashMap<String, ShellFile>, String> {
    // The renderers first, because the reader page has to be told where they
    // are before it is served.
    let mut shell = HashMap::new();
    let mut modules = serde_json::Map::new();
    for (name, path) in MODULES {
        let Some(body) = file(path) else { continue };
        let brotli_path = format!("{path}.br");
        let brotli = file(&brotli_path)
            .ok_or_else(|| format!("missing {brotli_path} in the shell: run make wasm"))?;
        let route = module_route(name, body);
        modules.insert(name.to_string(), serde_json::Value::String(route.clone()));
        shell.insert(
            route,
            ShellFile {
                kind: "application/wasm",
                body: axum::body::Bytes::from_static(body),
                brotli: Some(axum::body::Bytes::from_static(brotli)),
                immutable: true,
            },
        );
    }
    let modules = serde_json::Value::Object(modules).to_string();

    for entry in walk(&SHELL) {
        let path = entry.path().to_string_lossy().to_string();
        // The renderers are served under their digest.
        if path.starts_with("wasm/") {
            continue;
        }
        // A compressed copy is an encoding of the file beside it, not a file
        // of its own. It is attached below; it is never a route, or a browser
        // that asked for the bundle could be handed brotli it never
        // negotiated, under a name ending in ".br".
        if path.ends_with(".br") {
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
                .replace("__MODULES__", &modules)
                .into_bytes()
                .into()
        } else {
            axum::body::Bytes::from_static(entry.contents())
        };
        // What the build compressed, if it compressed this one. Only the
        // immutable directories have copies -- see web/tools/compress-shell.mjs
        // -- because a page is rewritten as it is served and a page compressed
        // at build time would still carry the placeholder.
        let brotli = file(&format!("{path}.br")).map(axum::body::Bytes::from_static);
        shell.insert(
            route,
            ShellFile {
                kind,
                body,
                brotli,
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

#[cfg(test)]
mod shell_tests {
    use super::*;

    fn shell() -> HashMap<String, ShellFile> {
        load_shell(&Configuration::default()).expect("the shell is embedded in the binary")
    }

    /// The bundle a reader waits for is the one worth compressing, and the
    /// build compresses it. Before this, every asset was served raw: the
    /// server has no compression middleware, so nothing else would have.
    #[test]
    fn the_bundled_assets_carry_a_compressed_copy() {
        let shell = shell();
        let scripts: Vec<_> = shell
            .iter()
            .filter(|(route, _)| route.starts_with("/assets/") && route.ends_with(".js"))
            .collect();
        assert!(
            !scripts.is_empty(),
            "the shell has no bundled scripts, so this proves nothing"
        );
        let compressed = scripts
            .iter()
            .filter(|(_, file)| file.brotli.is_some())
            .count();
        assert!(
            compressed > 0,
            "no bundled script has a brotli copy: has `bun run build` been run without \
             tools/compress-shell.mjs?"
        );
        for (route, file) in &scripts {
            if let Some(brotli) = &file.brotli {
                assert!(
                    brotli.len() < file.body.len(),
                    "{route} is larger compressed, so the copy should not have been written"
                );
            }
        }
    }

    /// A compressed copy is an encoding, not a file. Serving one under its own
    /// name would hand brotli to a browser that never negotiated it.
    #[test]
    fn a_compressed_copy_is_never_a_route_of_its_own() {
        for route in shell().keys() {
            assert!(!route.ends_with(".br"), "{route} is served as a file");
        }
    }

    /// Pages are rewritten as they are served -- `__MODULES__` becomes this
    /// deployment's renderer URLs -- so a page compressed at build time would
    /// be served with the placeholder still in it.
    #[test]
    fn pages_are_never_served_precompressed() {
        for (route, file) in shell() {
            if file.kind.starts_with("text/html") {
                assert!(
                    file.brotli.is_none(),
                    "{route} is a page and carries a build-time compressed copy"
                );
            }
        }
    }
}
