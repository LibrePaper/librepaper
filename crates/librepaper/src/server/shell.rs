//! The reader shell: the pages and the bundles they load, compiled into the
//! binary. The renderers are not: the pages are told where to fetch them.
//!
//! What is embedded is a build output. `web/` holds the Svelte sources, bun
//! and vite build them into `web/dist`, and this serves whatever is there --
//! by path, rather than from a table of routes, because the names of a bundle's
//! files are decided by the bundler and change with their contents.

use std::collections::HashMap;

use include_dir::{include_dir, Dir, File};

static SHELL: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../web/dist");
const WASM_LOCK: &str = include_str!("../../../../assets.lock");

/// The renderers, which are not in the binary. Each is a release of its own
/// engine repository, published to the asset mirror under a directory named
/// for the SHA-256 of its bytes: a module is fetched once per browser and
/// cached for a year, so the URL has to change whenever the module does. The
/// reader page is told the current URLs when it is served.
const MODULES: &[&str] = &["markdown", "bibliography", "citations", "typst"];

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
    /// one, so serving a bundle costs no compression work per request.
    pub brotli: Option<axum::body::Bytes>,
    /// A file whose bytes never change under this name, so it can be cached
    /// for a year rather than five minutes. Everything the bundler names for
    /// its own contents is one.
    pub immutable: bool,
}

impl ShellFile {}

fn file(name: &str) -> Option<&'static [u8]> {
    SHELL.get_file(name).map(File::contents)
}

/// The name of the LaTeX release's row in the lock: no `.wasm` suffix, because
/// what it pins is a directory of files, not a module.
const LATEX: &str = "latex";

/// The pinned LaTeX release: the SHA-256 of its MANIFEST.json, which is also
/// its directory name on the asset mirror (`latex/{sha256}/`).
pub fn latex_release() -> &'static str {
    static RELEASE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    RELEASE.get_or_init(|| {
        parse_lock(WASM_LOCK)
            .expect("assets.lock is valid")
            .remove(LATEX)
            .expect("assets.lock pins a latex release")
    })
}

/// The pinned digest of each asset, read from `assets.lock`: comment lines
/// start with `#`, and each other line is `module repository tag sha256`.
/// Exactly the four known `.wasm` modules and one `latex` row (no `.wasm`
/// suffix), each once, each with a lowercase SHA-256.
fn parse_lock(source: &str) -> Result<HashMap<String, String>, String> {
    let mut pins = HashMap::new();
    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let columns: Vec<&str> = line.split_whitespace().collect();
        let [module, _repository, _tag, sha] = columns[..] else {
            return Err(format!(
                "assets.lock: expected `module repository tag sha256`, got {line:?}"
            ));
        };
        let name = module.strip_suffix(".wasm").unwrap_or(module);
        let known = module == LATEX || (MODULES.contains(&name) && module != name);
        if !known {
            return Err(format!("assets.lock: unknown module {module:?}"));
        }
        let valid_sha = sha.len() == 64
            && sha
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
        if !valid_sha {
            return Err(format!(
                "assets.lock: {module} needs a lowercase 64-digit SHA-256, got {sha:?}"
            ));
        }
        if pins.insert(name.to_string(), sha.to_string()).is_some() {
            return Err(format!("assets.lock: {module} appears twice"));
        }
    }
    for name in MODULES {
        if !pins.contains_key(*name) {
            return Err(format!("assets.lock: missing {name}.wasm"));
        }
    }
    if !pins.contains_key(LATEX) {
        return Err(format!("assets.lock: missing {LATEX}"));
    }
    Ok(pins)
}

/// The browser URL of every renderer, `{mirror}wasm/{sha256}/{module}.wasm`,
/// as the JSON object a page is given in place of `__MODULES__`. `mirror` is
/// the validated asset mirror, which ends in `/`.
/// The output is placed inside a `<script type="application/json">` element,
/// so any `<` characters are escaped as unicode escape sequences to prevent
/// early script tag closure, which would still be valid JSON.
fn modules_json(mirror: &str, lock: &str) -> Result<String, String> {
    let pins = parse_lock(lock)?;
    let mut modules = serde_json::Map::new();
    for name in MODULES {
        let url = format!("{mirror}wasm/{}/{name}.wasm", pins[*name]);
        modules.insert(name.to_string(), serde_json::Value::String(url));
    }
    Ok(serde_json::Value::Object(modules).to_string().replace('<', "\\u003c"))
}

/// The source formats this process can render in a reader, and so offer an
/// editor for. Markdown and typst are rendered by modules the browser loads
/// from the asset mirror; HTML needs no module at all, its renderer being the
/// identity.
pub fn renderers() -> Vec<String> {
    vec![
        "markdown".to_string(),
        "quarto".to_string(),
        "html".to_string(),
        "typst".to_string(),
    ]
}

/// Everything the server answers with, ready to serve.
pub fn load_shell(asset_mirror: &str) -> Result<HashMap<String, ShellFile>, String> {
    // Where the renderers are comes first, because the reader page has to be
    // told before it is served.
    let mut shell = HashMap::new();
    let modules = modules_json(asset_mirror, WASM_LOCK)?;

    for entry in walk(&SHELL) {
        let path = entry.path().to_string_lossy().to_string();
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
        load_shell(crate::config::DEFAULT_ASSET_MIRROR)
            .expect("the shell is embedded in the binary")
    }

    fn lock(sha: &str) -> String {
        format!(
            "# pinned renderers\n\
             markdown.wasm wasm-markdown v0.1.1 {sha}\n\
             bibliography.wasm wasm-bibliography v0.1.0 {sha}\n\
             citations.wasm wasm-citations v0.1.0 {sha}\n\
             typst.wasm wasm-typst v0.1.0 {sha}\n\
             latex wasm-latex engines-test {sha}\n"
        )
    }

    #[test]
    fn the_latex_release_is_the_pinned_manifest_digest() {
        let release = latex_release();
        assert_eq!(release.len(), 64);
        assert_eq!(parse_lock(WASM_LOCK).unwrap()[LATEX], release);
    }

    #[test]
    fn the_checked_in_lock_parses() {
        parse_lock(WASM_LOCK).expect("assets.lock is valid");
    }

    #[test]
    fn a_module_url_is_the_mirror_then_its_digest_then_its_name() {
        let sha = "a".repeat(64);
        let json = modules_json("https://assets.example/", &lock(&sha)).unwrap();
        let modules: serde_json::Value = serde_json::from_str(&json).unwrap();
        for name in MODULES {
            assert_eq!(
                modules[name],
                format!("https://assets.example/wasm/{sha}/{name}.wasm")
            );
        }
        assert_eq!(modules.as_object().unwrap().len(), 4);
    }

    #[test]
    fn a_malformed_lock_is_refused() {
        let sha = "a".repeat(64);
        let good = lock(&sha);
        assert!(parse_lock(&good.replace("citations.wasm", "other.wasm")).is_err());
        assert!(parse_lock(&good.replace(&sha, &"A".repeat(64))).is_err());
        assert!(parse_lock(&good.replace(&sha, "abc")).is_err());
        assert!(parse_lock("markdown.wasm wasm-markdown v0.1.1\n").is_err());
        let without_typst: String = good
            .lines()
            .filter(|line| !line.starts_with("typst"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(parse_lock(&without_typst).is_err());
        assert!(parse_lock(&format!("{good}typst.wasm wasm-typst v0.1.0 {sha}\n")).is_err());
        let without_latex: String = good
            .lines()
            .filter(|line| !line.starts_with("latex"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(parse_lock(&without_latex).is_err());
        assert!(parse_lock(&format!("{good}latex wasm-latex engines-test {sha}\n")).is_err());
        assert!(parse_lock(&good.replace("latex ", "latex.wasm ")).is_err());
    }

    #[test]
    fn no_renderer_is_a_route_of_the_shell() {
        for route in shell().keys() {
            assert!(!route.starts_with("/wasm/"), "{route} is served");
        }
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
