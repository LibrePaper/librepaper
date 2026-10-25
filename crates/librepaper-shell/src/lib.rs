//! Internal to librepaper: no stable API, versioned in lockstep with it.
//! The reader shell: the pages and the bundles they load, compiled into the
//! binary. The renderers and the editor fonts are not: the pages are told
//! where to fetch them.
//!
//! What is embedded is a build output. `web/` holds the Svelte sources, bun
//! and vite build them into `web/dist`, and this serves whatever is there --
//! by path, rather than from a table of routes, because the names of a bundle's
//! files are decided by the bundler and change with their contents.

use std::collections::HashMap;

use include_dir::{include_dir, Dir, File};

// The data type and the two pure functions live in librepaper-base, so the
// server does not depend on this crate; the paths stay as consumers spell them.
pub use librepaper_base::shell::{content_type, renderers, ShellFile};

static SHELL: Dir<'static> = include_dir!("$LIBREPAPER_SHELL_DIST");
const ASSETS_LOCK: &str = include_str!(concat!(env!("OUT_DIR"), "/assets.lock"));

/// A file the lock pins, and the directory it lives under on the asset mirror:
/// `{prefix}/{sha256}/{file}`. Each is a release of its own repository, named
/// for the SHA-256 of its bytes, so a browser caches it for a year and the URL
/// has to change whenever the file does. The reader page is told the current
/// URLs when it is served.
struct Pin {
    file: &'static str,
    prefix: &'static str,
}

/// The renderers and the editor fonts, which are not in the binary. The five
/// `wasm` modules are fetched by the page that renders with them; the `fonts`
/// files are the faces the editor offers, each fetched when it is used.
const PINS: &[Pin] = &[
    pin("markdown.wasm", "wasm"),
    pin("bibliography.wasm", "wasm"),
    pin("citations.wasm", "wasm"),
    pin("typst.wasm", "wasm"),
    pin("pandoc.wasm", "wasm"),
    pin("harper.wasm", "wasm"),
    pin("jetbrains-mono.woff2", "fonts"),
    pin("jetbrains-mono-italic.woff2", "fonts"),
    pin("jetbrains-mono-semibold.woff2", "fonts"),
    pin("fira-code.woff2", "fonts"),
    pin("source-code-pro.woff2", "fonts"),
    pin("source-code-pro-italic.woff2", "fonts"),
    pin("ibm-plex-mono.woff2", "fonts"),
    pin("ibm-plex-mono-italic.woff2", "fonts"),
    pin("ibm-plex-mono-semibold.woff2", "fonts"),
    pin("cascadia-code.woff2", "fonts"),
    pin("cascadia-code-italic.woff2", "fonts"),
    pin("opendyslexic.woff2", "fonts"),
    pin("opendyslexic-italic.woff2", "fonts"),
    pin("opendyslexic-bold.woff2", "fonts"),
];

const fn pin(file: &'static str, prefix: &'static str) -> Pin {
    Pin { file, prefix }
}

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
        parse_lock(ASSETS_LOCK)
            .expect("assets.lock is valid")
            .remove(LATEX)
            .expect("assets.lock pins a latex release")
    })
}

/// The pinned digest of each asset, read from `assets.lock`: comment lines
/// start with `#`, and each other line is `file repository tag sha256`.
/// Exactly the files in `PINS` and one `latex` row (no `.wasm` suffix), each
/// once, each with a lowercase SHA-256. Keyed by file name.
fn parse_lock(source: &str) -> Result<HashMap<String, String>, String> {
    let mut pins = HashMap::new();
    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let columns: Vec<&str> = line.split_whitespace().collect();
        let [file, _repository, _tag, sha] = columns[..] else {
            return Err(format!(
                "assets.lock: expected `file repository tag sha256`, got {line:?}"
            ));
        };
        let known = file == LATEX || PINS.iter().any(|pin| pin.file == file);
        if !known {
            return Err(format!("assets.lock: unknown module {file:?}"));
        }
        let valid_sha = sha.len() == 64
            && sha
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
        if !valid_sha {
            return Err(format!(
                "assets.lock: {file} needs a lowercase 64-digit SHA-256, got {sha:?}"
            ));
        }
        if pins.insert(file.to_string(), sha.to_string()).is_some() {
            return Err(format!("assets.lock: {file} appears twice"));
        }
    }
    for pin in PINS {
        if !pins.contains_key(pin.file) {
            return Err(format!("assets.lock: missing {}", pin.file));
        }
    }
    if !pins.contains_key(LATEX) {
        return Err(format!("assets.lock: missing {LATEX}"));
    }
    Ok(pins)
}

/// A JSON object of the URL of every pinned file with `prefix`, each
/// `{mirror}{prefix}/{sha256}/{file}`, keyed by `key` of the file name. `mirror`
/// is the validated asset mirror, which ends in `/`.
///
/// The page carries it in a `<script type="application/json">` element, so
/// every `<` is written as `\u003c`: the same JSON, but it can never close
/// that element early.
fn urls_json(
    mirror: &str,
    lock: &str,
    prefix: &str,
    key: fn(&'static str) -> &'static str,
) -> Result<String, String> {
    let pins = parse_lock(lock)?;
    let mut urls = serde_json::Map::new();
    for pin in PINS.iter().filter(|pin| pin.prefix == prefix) {
        let url = format!("{mirror}{prefix}/{}/{}", pins[pin.file], pin.file);
        urls.insert(key(pin.file).to_string(), serde_json::Value::String(url));
    }
    Ok(serde_json::Value::Object(urls)
        .to_string()
        .replace('<', "\\u003c"))
}

/// The browser URL of every renderer, `{mirror}wasm/{sha256}/{module}.wasm`,
/// keyed by the module's name without `.wasm`, as the JSON object a page is
/// given in place of `__MODULES__`.
fn modules_json(mirror: &str, lock: &str) -> Result<String, String> {
    urls_json(mirror, lock, "wasm", |file| {
        file.strip_suffix(".wasm").unwrap_or(file)
    })
}

/// The browser URL of every editor font, `{mirror}fonts/{sha256}/{file}`, keyed
/// by the file name, as the JSON object a page is given in place of `__FONTS__`.
fn fonts_json(mirror: &str, lock: &str) -> Result<String, String> {
    urls_json(mirror, lock, "fonts", |file| file)
}

/// Everything the server answers with, ready to serve.
pub fn load_shell(asset_mirror: &str) -> Result<HashMap<String, ShellFile>, String> {
    // Where the renderers and the fonts are comes first, because the reader
    // page has to be told before it is served.
    let mut shell = HashMap::new();
    let modules = modules_json(asset_mirror, ASSETS_LOCK)?;
    let fonts = fonts_json(asset_mirror, ASSETS_LOCK)?;

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
            // documentation page, and where the renderers and fonts are.
            entry
                .contents_utf8()
                .unwrap_or_default()
                .replace("__MODULES__", &modules)
                .replace("__FONTS__", &fonts)
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

    const MIRROR: &str = "https://librepaper-mirror-0001.s3.bhs.io.cloud.ovh.net/";

    fn shell() -> HashMap<String, ShellFile> {
        load_shell(MIRROR).expect("the shell is embedded in the binary")
    }

    /// A lock with every row `PINS` names, and the `latex` row.
    fn lock(sha: &str) -> String {
        let mut lock = String::from("# pinned renderers and fonts\n");
        for pin in PINS {
            lock.push_str(&format!("{} librepaper-test v0.0.0 {sha}\n", pin.file));
        }
        lock.push_str(&format!("latex wasm-latex engines-test {sha}\n"));
        lock
    }

    #[test]
    fn the_latex_release_is_the_pinned_manifest_digest() {
        let release = latex_release();
        assert_eq!(release.len(), 64);
        assert_eq!(parse_lock(ASSETS_LOCK).unwrap()[LATEX], release);
    }

    #[test]
    fn the_checked_in_lock_parses() {
        parse_lock(ASSETS_LOCK).expect("assets.lock is valid");
    }

    #[test]
    fn a_module_url_is_the_mirror_then_its_digest_then_its_name() {
        let sha = "a".repeat(64);
        let json = modules_json("https://assets.example/", &lock(&sha)).unwrap();
        let modules: serde_json::Value = serde_json::from_str(&json).unwrap();
        for name in [
            "markdown",
            "bibliography",
            "citations",
            "typst",
            "pandoc",
            "harper",
        ] {
            assert_eq!(
                modules[name],
                format!("https://assets.example/wasm/{sha}/{name}.wasm")
            );
        }
        assert_eq!(modules.as_object().unwrap().len(), 6);
    }

    #[test]
    fn a_font_url_is_the_mirror_then_its_digest_then_its_name() {
        let sha = "a".repeat(64);
        let json = fonts_json("https://assets.example/", &lock(&sha)).unwrap();
        let fonts: serde_json::Value = serde_json::from_str(&json).unwrap();
        let names: Vec<&str> = PINS
            .iter()
            .filter(|pin| pin.prefix == "fonts")
            .map(|pin| pin.file)
            .collect();
        assert_eq!(names.len(), 14);
        assert_eq!(fonts.as_object().unwrap().len(), 14);
        for name in names {
            assert_eq!(
                fonts[name],
                format!("https://assets.example/fonts/{sha}/{name}")
            );
        }
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
    fn a_lock_missing_one_font_is_refused() {
        let sha = "a".repeat(64);
        let without_font: String = lock(&sha)
            .lines()
            .filter(|line| !line.starts_with("fira-code.woff2"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(parse_lock(&without_font).is_err());
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
