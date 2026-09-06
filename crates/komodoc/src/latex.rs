//! The LaTeX mirror, as this process serves it.
//!
//! Komodoc carries no TeX. A browser that has been asked to compile a `.tex`
//! document fetches a distribution -- an Emscripten module, its loader, and
//! the TeX Live files the engine reads -- and every one of those files comes
//! from `/latex/` on the reader's own origin. `--latex <url-or-dir>` says
//! where this process reads them from: a bucket over https, or a directory on
//! this machine for a self-hoster who has mirrored one.
//!
//! Same-origin, and not a redirect, for a reason that is Emscripten's rather
//! than ours. A module's loader resolves its `.wasm` relative to the worker's
//! own URL, and the documents origin's CSP is `script-src 'self' ... blob:
//! https:` -- so a cross-origin mirror would mean fetching every loader as
//! text and running it from a blob, which is a lot of machinery to avoid one
//! proxy. The proxy is also the privacy answer: the list of packages a
//! document asks for is a description of the document, and it goes to the
//! deployment the author already trusts with the source and nowhere else.
//!
//! What is served is bounded to two shapes. `manifest.json` is the mirror's
//! index and the only file whose URL carries no digest, so it is `no-cache`.
//! Everything else is named by a digest of its own bytes -- see
//! `latex/tools/mirror.mjs` -- so it is immutable for a year and a new release is a
//! new path rather than a cache to invalidate.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Where the project keeps the mirror it builds: a Cloudflare worker of static
/// files that `make latex-push` deploys from deploy/latex/wrangler.toml. A
/// deployment that passes `--latex` with no value gets this, which is the
/// sandbox's own and is a self-hoster's to mirror or replace rather than to
/// depend on.
pub const DEFAULT_MIRROR: &str = "https://komodoc-latex.arelbundock.workers.dev/";

/// The mirror's index: the one path that is not named by a digest, and so the
/// one that may not be cached for ever.
const MANIFEST: &str = "manifest.json";

/// Where the files come from.
#[derive(Debug)]
pub enum Mirror {
    /// A directory on this machine, the shape `latex/tools/mirror.mjs` writes.
    Directory(PathBuf),
    /// A bucket. One client, no retries: a mirror that is briefly unreachable
    /// is a card that briefly cannot fetch, which the browser already has to
    /// handle, and a retry here would only turn one slow request into three.
    Upstream {
        base: String,
        client: reqwest::Client,
    },
}

/// One answer, ready to be turned into a response by the route.
pub struct Served {
    pub status: u16,
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub cache_control: &'static str,
}

impl Mirror {
    /// Reads the flag. An `http:` upstream is refused here rather than left to
    /// fail silently in every browser: the shell's CSP permits `https:` and
    /// `blob:` and nothing else, so a mirror on a private network over plain
    /// HTTP would be a card that never finishes fetching and a console nobody
    /// is looking at. A directory that is not there is refused for the same
    /// reason -- at startup, where the operator is.
    ///
    /// An https mirror that is merely unreachable is *not* refused. It may be
    /// a bucket that is still being filled, or a network this machine will
    /// have in a minute, and neither is a reason for a deployment that serves
    /// markdown to refuse to start.
    pub fn open(flag: &str) -> Result<Mirror, String> {
        let flag = flag.trim();
        if flag.is_empty() {
            return Err("--latex needs a URL or a directory".into());
        }
        if let Some(rest) = flag.strip_prefix("http://") {
            let _ = rest;
            return Err(format!(
                "--latex {flag} is plain HTTP, and a browser will not load a compiler from it.\n\n  \
                 The page a document is framed in only permits https: and blob: sources, so an\n  \
                 http: mirror fails in every browser with nothing on screen to say why.\n\n  \
                 Serve the mirror over https, or point --latex at the directory itself:\n\n    \
                 komodoc serve --latex /srv/komodoc/latex"
            ));
        }
        if flag.starts_with("https://") {
            let base = if flag.ends_with('/') {
                flag.to_string()
            } else {
                format!("{flag}/")
            };
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .map_err(|err| format!("could not build an HTTP client for --latex: {err}"))?;
            return Ok(Mirror::Upstream { base, client });
        }
        let path = PathBuf::from(flag);
        if !path.is_dir() {
            return Err(format!(
                "--latex {flag} is neither an https URL nor a directory on this machine.\n\n  \
                 Build one with `node latex/tools/mirror.mjs`, or pass the bucket that serves it."
            ));
        }
        Ok(Mirror::Directory(path))
    }

    /// What this mirror is, for the startup line. Never the credentials of
    /// anything, because there are none: a mirror is public static files.
    pub fn describe(&self) -> String {
        match self {
            Mirror::Directory(path) => path.display().to_string(),
            Mirror::Upstream { base, .. } => base.clone(),
        }
    }

    /// A `HEAD` of the manifest, once, at startup: enough to tell an operator
    /// their bucket is empty or misspelled while they are still looking at the
    /// terminal. The answer is a warning, never a death.
    pub async fn probe(&self) -> Option<String> {
        match self {
            Mirror::Directory(root) => {
                if root.join(MANIFEST).is_file() {
                    None
                } else {
                    Some(format!(
                        "warning: {} has no manifest.json; run `node latex/tools/mirror.mjs` to build one",
                        root.display()
                    ))
                }
            }
            Mirror::Upstream { base, client } => {
                match client.head(format!("{base}{MANIFEST}")).send().await {
                    Ok(response) if response.status().is_success() => None,
                    Ok(response) => Some(format!(
                        "warning: the LaTeX mirror at {base} answered {} for manifest.json",
                        response.status().as_u16()
                    )),
                    Err(err) => Some(format!(
                        "warning: the LaTeX mirror at {base} is unreachable: {err}"
                    )),
                }
            }
        }
    }

    /// Serves one path under the mirror, or says it is not there.
    ///
    /// Two kinds of path arrive. The loader, the module and the manifest are
    /// asked for by the digested URLs the manifest hands out. A TeX Live file
    /// is not: the engine asks for it by name, as
    /// `packages/<engine>/<format code>/<name>`, built in C from the base
    /// URL it was given, and only the manifest knows which digested file that
    /// name is. So a name is looked up first and served as the file it names,
    /// and a name the manifest does not have is a 301, not a 404 -- the engine
    /// reads a 301 as "this file does not exist" and moves on, and a 404 as a
    /// network failure to retry, which is a compile that never ends.
    pub async fn get(&self, path: &str) -> Served {
        let Some(path) = safe_path(path) else {
            return missing();
        };
        if let Some(key) = package_key(&path) {
            return match self.resolve(&key).await {
                Resolved::File(url) => match safe_path(&url) {
                    Some(url) if package_key(&url).is_none() => self.fetch(&url).await,
                    _ => absent(),
                },
                Resolved::NotThere => absent(),
                Resolved::Unreachable => unreachable(),
            };
        }
        self.fetch(&path).await
    }

    /// Which digested file a name is, from the manifest's package index. The
    /// index is kept for `INDEX_REFRESH` and read again after that, on the
    /// next name asked for, so a mirror updated in place is noticed within
    /// that and a compile costs one manifest read per refresh rather than one
    /// per file.
    async fn resolve(&self, key: &str) -> Resolved {
        let name = self.describe();
        let cached = indexes()
            .lock()
            .expect("the index lock")
            .get(&name)
            .cloned();
        // Fresh enough to answer from, whichever way it answers. A mirror
        // can replace a file in place -- `serve.mjs --record` does, when
        // upstream has a better copy than this machine's TeX Live -- and
        // that changes which digested file a name is, so a hit on an old
        // copy of the index is as stale as a miss.
        if let Some(index) = cached
            .as_ref()
            .filter(|index| index.loaded.elapsed() < INDEX_REFRESH)
        {
            return match index.packages.get(key) {
                Some(url) => Resolved::File(url.clone()),
                None => Resolved::NotThere,
            };
        }
        match self.load_index().await {
            Some(index) => {
                let index = Arc::new(index);
                indexes()
                    .lock()
                    .expect("the index lock")
                    .insert(name, index.clone());
                match index.packages.get(key) {
                    Some(url) => Resolved::File(url.clone()),
                    None => Resolved::NotThere,
                }
            }
            // The manifest could not be read now. What was known before still
            // stands, and a name it did not have is still not there; with
            // nothing known at all, the mirror is what is unreachable.
            None if cached.is_some() => Resolved::NotThere,
            None => Resolved::Unreachable,
        }
    }

    /// The manifest's `packages` map, read through the same path a browser
    /// reads the manifest by.
    async fn load_index(&self) -> Option<Index> {
        let served = self.fetch(MANIFEST).await;
        if served.status != 200 {
            return None;
        }
        let manifest: serde_json::Value = serde_json::from_slice(&served.bytes).ok()?;
        let packages = manifest
            .get("packages")
            .and_then(|value| value.as_object())
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|(key, entry)| {
                        let url = entry.get("url")?.as_str()?;
                        Some((key.clone(), url.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Index {
            packages,
            loaded: Instant::now(),
        })
    }

    /// One file, by the path the mirror keeps it under.
    async fn fetch(&self, path: &str) -> Served {
        let path = path.to_string();
        if let Mirror::Upstream { base, .. } = self {
            if !request_stays_under_base(base, &path) {
                return missing();
            }
        }
        let cache_control = if path == MANIFEST {
            // The only file whose URL carries no digest, and so the only one an
            // updated mirror changes in place.
            "no-cache"
        } else {
            "public, max-age=31536000, immutable"
        };
        let content_type = content_type(&path);
        match self {
            Mirror::Directory(root) => match tokio::fs::read(root.join(&path)).await {
                Ok(bytes) => Served {
                    status: 200,
                    bytes,
                    content_type,
                    cache_control,
                },
                Err(_) => missing(),
            },
            Mirror::Upstream { base, client } => {
                let Ok(response) = client.get(format!("{base}{path}")).send().await else {
                    // The mirror is the browser's only source, so a mirror that
                    // is down is a gateway that is down, and saying so is more
                    // use to whoever is looking than a 404 would be.
                    return Served {
                        status: 502,
                        bytes: b"the LaTeX mirror is unreachable".to_vec(),
                        content_type: "text/plain; charset=utf-8",
                        cache_control: "no-store",
                    };
                };
                // The upstream's own answer, passed through: a 403 on a bucket
                // whose policy is wrong should not read as a missing file.
                let status = response.status().as_u16();
                let bytes = response.bytes().await.unwrap_or_default().to_vec();
                if !(200..300).contains(&status) {
                    return Served {
                        status,
                        bytes,
                        content_type: "text/plain; charset=utf-8",
                        cache_control: "no-store",
                    };
                }
                Served {
                    status,
                    bytes,
                    content_type,
                    cache_control,
                }
            }
        }
    }
}

fn missing() -> Served {
    Served {
        status: 404,
        bytes: b"not found".to_vec(),
        content_type: "text/plain; charset=utf-8",
        cache_control: "no-store",
    }
}

/// A TeX Live file the mirror does not have, in the one status the engine
/// reads as "does not exist" rather than "try again". Not cached by the
/// browser, since a mirror can gain the file later.
fn absent() -> Served {
    Served {
        status: 301,
        bytes: b"no such file".to_vec(),
        content_type: "text/plain; charset=utf-8",
        cache_control: "no-store",
    }
}

fn unreachable() -> Served {
    Served {
        status: 502,
        bytes: b"the LaTeX mirror is unreachable".to_vec(),
        content_type: "text/plain; charset=utf-8",
        cache_control: "no-store",
    }
}

/// The manifest's package index: the engine's name for a file, as
/// `<engine>/<format code>/<name>`, to the digested path that holds it. The
/// names the mirror recorded as having nowhere to fetch from are not kept;
/// a name that is not here is not there, whatever the reason.
#[derive(Debug)]
struct Index {
    packages: HashMap<String, String>,
    loaded: Instant,
}

enum Resolved {
    File(String),
    NotThere,
    Unreachable,
}

/// How long the index is answered from before the manifest is read again.
/// Long enough that a compile, which asks for hundreds of files, reads the
/// manifest once; short enough that a mirror updated in place is noticed
/// without a restart.
const INDEX_REFRESH: Duration = Duration::from_secs(30);

/// One index per mirror, by the name `describe` gives it. A process has one
/// mirror, or a test has a few, and none of them is a reason for the mirror
/// to be anything other than the enum the flag parses into.
fn indexes() -> &'static Mutex<HashMap<String, Arc<Index>>> {
    static INDEXES: OnceLock<Mutex<HashMap<String, Arc<Index>>>> = OnceLock::new();
    INDEXES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The engine's name for a file, if this is a request for one: four
/// components, `packages`, an engine, a numeric format code and a name that is
/// not already a digested file name. A digested name starts with sixteen hex
/// digits and a dash, which no TeX Live file does.
fn package_key(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').collect();
    let [prefix, engine, format, name] = parts[..] else {
        return None;
    };
    if prefix != "packages"
        || engine.is_empty()
        || format.is_empty()
        || !format.bytes().all(|byte| byte.is_ascii_digit())
        || name.is_empty()
    {
        return None;
    }
    let digested = name.len() > 17
        && name.as_bytes()[16] == b'-'
        && name.bytes().take(16).all(|byte| byte.is_ascii_hexdigit());
    if digested {
        return None;
    }
    Some(format!("{engine}/{format}/{name}"))
}

/// The path a request may have, or nothing. Percent escapes are decoded first,
/// because `%2e%2e` is `..` to a filesystem and only the decoded form is worth
/// checking; then every component has to be an ordinary name. A path that
/// leaves the mirror does not get a cleaned-up version of itself, it gets a
/// 404, because there is no legitimate request that needs cleaning.
///
/// A single decode is not the end of it: `%252e%252e` decodes to `%2e%2e`,
/// which still reads as an ordinary filename here but is `..` again to
/// whatever parses it next -- namely `url::Url`, building the upstream
/// request in `get`. Every legitimate mirror path is `manifest.json` or a
/// digest-named file (see `latex/tools/mirror.mjs`), and neither ever
/// contains a literal `%`, so a `%` surviving the one decode this function
/// does is refused outright rather than decoded again.
fn safe_path(raw: &str) -> Option<String> {
    let decoded = percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .ok()?
        .to_string();
    if decoded.is_empty()
        || decoded.contains('\0')
        || decoded.contains('\\')
        || decoded.contains('%')
    {
        return None;
    }
    let mut out = String::new();
    for component in Path::new(&decoded).components() {
        let Component::Normal(name) = component else {
            return None;
        };
        let name = name.to_str()?;
        if name == "." || name == ".." {
            return None;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(name);
    }
    if out.is_empty() {
        return None;
    }
    Some(out)
}

/// A second, independent gate for the `Upstream` variant only: parse the
/// exact URL `get` is about to send, and refuse it unless it is still on the
/// base's origin and its path still starts with the base's path. `safe_path`
/// already keeps ordinary requests from carrying `..` or an encoded `..`
/// through to this parse, but this check does not rely on that -- it looks at
/// what `url::Url` actually resolved, which is the thing a browser-facing
/// proxy must agree with regardless of how the path got here.
fn request_stays_under_base(base: &str, path: &str) -> bool {
    let Ok(base_url) = url::Url::parse(base) else {
        return false;
    };
    let Ok(request_url) = url::Url::parse(&format!("{base}{path}")) else {
        return false;
    };
    if request_url.origin() != base_url.origin() {
        return false;
    }
    request_url.path().starts_with(base_url.path())
}

/// What each kind of file in a mirror is. Only the loader and the module have
/// a type a browser insists on: `application/wasm` is what
/// `WebAssembly.instantiateStreaming` requires, and a `.js` served as
/// something else will not run under the shell's CSP. The TeX files are bytes
/// the engine reads out of Cache Storage and never a browser's business.
fn content_type(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".js") || lower.ends_with(".mjs") {
        "text/javascript; charset=utf-8"
    } else if lower.ends_with(".wasm") {
        "application/wasm"
    } else if lower.ends_with(".json") {
        "application/json; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The engine asks for a TeX Live file by name, and the manifest says
    // which digested file that is. A name the manifest has is that file, with
    // the cache life a digested file earns; a name it does not have is a 301,
    // the one status the engine reads as "does not exist"; and a digested path
    // asked for directly is untouched by any of this.
    #[tokio::test]
    async fn a_package_asked_for_by_name_is_the_file_the_manifest_names() {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let file = "packages/pdftex/b2/b20a30ef79872ed1-article.cls";
        std::fs::write(
            dir.path().join("manifest.json"),
            format!(
                r#"{{"version":1,"distributions":{{}},"packages":{{"pdftex/26/article.cls":{{"url":"{file}","size":12}}}},"absent":{{"pdftex/26/nowhere.sty":true}}}}"#
            ),
        )
        .expect("manifest");
        std::fs::create_dir_all(dir.path().join("packages/pdftex/b2")).expect("directory");
        std::fs::write(dir.path().join(file), b"\\ProvidesClass").expect("the class");
        let mirror = Mirror::Directory(dir.path().to_path_buf());

        let served = mirror.get("packages/pdftex/26/article.cls").await;
        assert_eq!(served.status, 200);
        assert_eq!(served.bytes, b"\\ProvidesClass");
        assert_eq!(served.cache_control, "public, max-age=31536000, immutable");

        for name in [
            "packages/pdftex/26/nothere.sty",
            "packages/pdftex/26/nowhere.sty",
        ] {
            let served = mirror.get(name).await;
            assert_eq!(served.status, 301, "{name}");
            assert_eq!(served.cache_control, "no-store");
        }

        let served = mirror.get(file).await;
        assert_eq!(served.status, 200);
        assert_eq!(served.bytes, b"\\ProvidesClass");
    }

    #[test]
    fn a_package_key_is_a_name_and_never_a_digested_file() {
        assert_eq!(
            package_key("packages/pdftex/26/amsmath.sty").as_deref(),
            Some("pdftex/26/amsmath.sty")
        );
        assert_eq!(
            package_key("packages/xetex/3/cmr10").as_deref(),
            Some("xetex/3/cmr10")
        );
        assert_eq!(
            package_key("packages/pdftex/84/843c4a4ee0404b93-amsmath.sty"),
            None
        );
        assert_eq!(package_key("packages/pdftex/b2/article.cls"), None);
        assert_eq!(
            package_key("swiftlatex-pdftex/2dfb2fc534b459b5/engine.js"),
            None
        );
        assert_eq!(package_key("manifest.json"), None);
    }

    // Every way out of the mirror that has ever been tried on a static route,
    // in the two forms a browser can send them.
    #[test]
    fn a_path_that_leaves_the_mirror_is_refused() {
        for escape in [
            "../secrets",
            "a/../../secrets",
            "%2e%2e/secrets",
            "a/%2e%2e/%2e%2e/secrets",
            "/etc/passwd",
            "..\\secrets",
            "",
            ".",
        ] {
            assert!(safe_path(escape).is_none(), "{escape} was allowed");
        }
    }

    #[test]
    fn an_ordinary_mirror_path_survives_unchanged() {
        assert_eq!(safe_path("manifest.json").as_deref(), Some("manifest.json"));
        assert_eq!(
            safe_path("swiftlatex-pdftex/2dfb2fc534b459b5/swiftlatexpdftex.wasm").as_deref(),
            Some("swiftlatex-pdftex/2dfb2fc534b459b5/swiftlatexpdftex.wasm")
        );
        assert_eq!(
            safe_path("packages/pdftex/b2/b20a30ef79872ed1-article.cls").as_deref(),
            Some("packages/pdftex/b2/b20a30ef79872ed1-article.cls")
        );
    }

    #[test]
    fn a_plain_http_mirror_is_refused_with_a_reason() {
        let err = Mirror::open("http://mirror.internal/latex/").expect_err("accepted http");
        assert!(err.contains("plain HTTP"), "{err}");
        assert!(err.contains("https"), "{err}");
    }

    #[test]
    fn a_bucket_and_a_directory_are_both_a_mirror() {
        let base = match Mirror::open("https://example.invalid/latex").expect("https") {
            Mirror::Upstream { base, .. } => base,
            _ => panic!("an https URL is not a directory"),
        };
        assert_eq!(base, "https://example.invalid/latex/");

        let dir = tempfile::tempdir().expect("tempdir");
        assert!(matches!(
            Mirror::open(&dir.path().display().to_string()),
            Ok(Mirror::Directory(_))
        ));
        assert!(Mirror::open("/no/such/mirror/anywhere").is_err());
    }
}
