//! What a typst compile could not find, fetched: packages from the registry,
//! and font families from the deployment's library, kept in a cache between
//! runs. The browser does the same with the module's `needs` export, so a
//! document that reaches a package or a font there reaches it here, and
//! `publish` compiles what the editor previewed.
//!
//! The compiler itself never fetches. A compile answers what it went looking
//! for and did not find, this fetches that into the cache, and the compile is
//! run again with the cache one round fuller. A few rounds at most: a
//! package's own dependencies surface on the round after it arrives, and a
//! document that compiles cleanly needs nothing more.

use std::io::Read;
use std::path::{Path, PathBuf};

use wasm_typst::typst::{Needs, Package};

use super::render::Noted;

/// The most a package archive may unpack to. Packages are source and a few
/// figures; anything larger is not one.
const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;
/// The most one font file may be.
const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;
/// How many fetch-and-compile rounds a document may take before it is left
/// with whatever it has. Dependencies nest a few levels deep at most.
const ROUNDS: usize = 8;

/// Where fetched packages and fonts are kept between runs.
///
/// Packages are laid out the way the `typst` binary lays out its own cache --
/// `typst/packages/<namespace>/<name>/<version>/` under the cache home -- so a
/// machine that has run it already has the packages, and one that has run
/// this has them for it.
#[derive(Clone, Debug)]
pub struct Cache {
    root: PathBuf,
    /// Where package archives come from, for the one test that serves its own.
    registry: String,
}

/// Where the typst registry serves a package archive: `preview/<name>-<version>.tar.gz`
/// under this.
pub const REGISTRY: &str = "https://packages.typst.org/";

impl Cache {
    /// `$XDG_CACHE_HOME`, or `~/.cache`. `None` on a machine with neither,
    /// which compiles without packages rather than dying: the compiler says
    /// what it could not find.
    pub fn discover() -> Option<Cache> {
        let root = match std::env::var("XDG_CACHE_HOME") {
            Ok(base) if !base.is_empty() => PathBuf::from(base),
            _ => {
                let home = std::env::var("HOME").ok().filter(|h| !h.is_empty())?;
                PathBuf::from(home).join(".cache")
            }
        };
        Some(Cache::at(root))
    }

    pub fn at(root: impl Into<PathBuf>) -> Cache {
        Cache {
            root: root.into(),
            registry: REGISTRY.to_string(),
        }
    }

    /// The same cache, fetching packages from another registry. For tests.
    #[cfg(test)]
    pub fn with_registry(mut self, base: &str) -> Cache {
        self.registry = base.to_string();
        self
    }

    fn package_dir(&self, package: &Package) -> PathBuf {
        self.root
            .join("typst")
            .join("packages")
            .join(&package.namespace)
            .join(&package.name)
            .join(&package.version)
    }

    /// Whether a path is one the compiler asks a package's file by:
    /// `@preview/cetz/0.3.4/src/lib.typ`.
    pub fn is_package_path(path: &Path) -> bool {
        path.to_str().is_some_and(|text| text.starts_with('@'))
    }

    /// A package's file, by the path the compiler asks for, or `None` for a
    /// package the cache does not have.
    pub fn package_file(&self, path: &Path) -> Option<Vec<u8>> {
        let text = path.to_str()?.strip_prefix('@')?;
        let mut parts = text.splitn(4, '/');
        let namespace = parts.next()?;
        let name = parts.next()?;
        let version = parts.next()?;
        let inside = parts.next()?;
        if [namespace, name, version]
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..")
        {
            return None;
        }
        let dir = self.package_dir(&Package {
            namespace: namespace.to_string(),
            name: name.to_string(),
            version: version.to_string(),
        });
        let resolved = within(&dir, Path::new(inside))?;
        std::fs::read(resolved).ok()
    }

    fn has_package(&self, package: &Package) -> bool {
        self.package_dir(package).join("typst.toml").is_file()
    }

    fn family_dir(&self, family: &str) -> PathBuf {
        self.root
            .join("librepaper")
            .join("fonts")
            .join(family_dir_name(family))
    }

    fn has_family(&self, family: &str) -> bool {
        std::fs::read_dir(self.family_dir(family))
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
    }

    /// The font files cached for these families, named for the compile's
    /// sake, in a stable order.
    pub fn fonts_for(&self, families: &[String]) -> Vec<(String, Vec<u8>)> {
        let mut fonts = Vec::new();
        for family in families {
            let dir = self.family_dir(family);
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut names: Vec<String> = entries
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.path().is_file())
                .map(|entry| entry.file_name().to_string_lossy().to_string())
                .filter(|name| wasm_typst::typst::is_font(name))
                .collect();
            names.sort();
            for name in names {
                if let Ok(bytes) = std::fs::read(dir.join(&name)) {
                    fonts.push((format!("{}/{name}", family_dir_name(family)), bytes));
                }
            }
        }
        fonts
    }

    /// Fetches what a compile could not find and the cache does not have:
    /// packages from the registry, fonts from the library at `fonts_from`.
    /// `Ok(true)` if anything arrived. A family the library lacks, or a
    /// package the registry lacks, is said once and is not an error: the
    /// compile's own diagnostic already names it.
    pub async fn fetch(
        &self,
        needs: &Needs,
        fonts_from: Option<&str>,
        client: &reqwest::Client,
    ) -> Result<bool, String> {
        let mut arrived = false;
        for package in &needs.packages {
            if self.has_package(package) {
                continue;
            }
            let Some(url) = package.url() else {
                eprintln!(
                    "warning: @{}/{}:{} is not a package the registry serves",
                    package.namespace, package.name, package.version
                );
                continue;
            };
            let url = url.replace(REGISTRY, &self.registry);
            eprintln!(
                "fetching @{}/{}:{}",
                package.namespace, package.name, package.version
            );
            let response = client
                .get(&url)
                .send()
                .await
                .map_err(|err| format!("could not fetch {url}: {err}"))?;
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                eprintln!(
                    "warning: the registry has no @{}/{}:{}",
                    package.namespace, package.name, package.version
                );
                continue;
            }
            if !response.status().is_success() {
                return Err(format!("could not fetch {url}: {}", response.status()));
            }
            let archive = response
                .bytes()
                .await
                .map_err(|err| format!("could not read {url}: {err}"))?;
            let files = unpack(&archive)?;
            if !files.iter().any(|(name, _)| name == "typst.toml") {
                return Err(format!(
                    "{url} is not a typst package: it has no typst.toml"
                ));
            }
            self.store_package(package, &files)?;
            arrived = true;
        }
        if needs.fonts.iter().any(|family| !self.has_family(family)) {
            let Some(base) = fonts_from else {
                return Ok(arrived);
            };
            let base = base.trim_end_matches('/');
            let index_url = format!("{base}/api/fonts/index.json");
            let response = client
                .get(&index_url)
                .send()
                .await
                .map_err(|err| format!("could not fetch {index_url}: {err}"))?;
            if !response.status().is_success() {
                // A deployment without a library is not an error; the
                // document sets in the fallback, which the warning says.
                return Ok(arrived);
            }
            let index: serde_json::Value = response
                .json()
                .await
                .map_err(|err| format!("{index_url} is not a font index: {err}"))?;
            for family in &needs.fonts {
                if self.has_family(family) {
                    continue;
                }
                let Some(files) = index
                    .get("families")
                    .and_then(|families| families.get(family))
                    .and_then(|files| files.as_array())
                else {
                    eprintln!("warning: {base} has no font named \"{family}\"");
                    continue;
                };
                let mut fetched = Vec::new();
                for file in files.iter().filter_map(|file| file.as_str()) {
                    let url = format!("{base}/api/fonts/{file}");
                    let response = client
                        .get(&url)
                        .send()
                        .await
                        .map_err(|err| format!("could not fetch {url}: {err}"))?;
                    if !response.status().is_success() {
                        return Err(format!("could not fetch {url}: {}", response.status()));
                    }
                    let bytes = response
                        .bytes()
                        .await
                        .map_err(|err| format!("could not read {url}: {err}"))?;
                    if bytes.len() > MAX_FONT_BYTES {
                        return Err(format!("{url} is larger than a font file should be"));
                    }
                    let name = file.rsplit('/').next().unwrap_or(file).to_string();
                    fetched.push((name, bytes.to_vec()));
                }
                if fetched.is_empty() {
                    continue;
                }
                eprintln!("fetched the font \"{family}\" from {base}");
                let dir = self.family_dir(family);
                std::fs::create_dir_all(&dir)
                    .map_err(|err| format!("could not create {}: {err}", dir.display()))?;
                for (name, bytes) in fetched {
                    std::fs::write(dir.join(&name), bytes).map_err(|err| {
                        format!("could not write {name} to {}: {err}", dir.display())
                    })?;
                }
                arrived = true;
            }
        }
        Ok(arrived)
    }

    /// Writes a package's files under its directory, whole or not at all: a
    /// package that is half there would be found by `has_package` and never
    /// fetched again.
    fn store_package(&self, package: &Package, files: &[(String, Vec<u8>)]) -> Result<(), String> {
        let dir = self.package_dir(package);
        let staging = dir.with_extension(format!("part-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        for (name, bytes) in files {
            let Some(target) = within(&staging, Path::new(name)) else {
                continue;
            };
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|err| format!("could not create {}: {err}", parent.display()))?;
            }
            std::fs::write(&target, bytes)
                .map_err(|err| format!("could not write {}: {err}", target.display()))?;
        }
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("could not create {}: {err}", parent.display()))?;
        }
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::rename(&staging, &dir)
            .map_err(|err| format!("could not move the package into {}: {err}", dir.display()))
    }
}

/// A family as a directory name: lowercase already, and anything a filesystem
/// might object to replaced.
fn family_dir_name(family: &str) -> String {
    family
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `root/path`, or `None` for a path that would leave the root.
fn within(root: &Path, path: &Path) -> Option<PathBuf> {
    let mut resolved = root.to_path_buf();
    for part in path.components() {
        match part {
            std::path::Component::Normal(name) => resolved.push(name),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(resolved)
}

/// A registry archive as its files: gzip around a tar.
fn unpack(archive: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(archive)
        .take(MAX_PACKAGE_BYTES)
        .read_to_end(&mut tar)
        .map_err(|err| format!("the package archive is not gzip: {err}"))?;
    Ok(untar(&tar))
}

/// The regular files in a tar, by name. Enough of the format for what the
/// registry produces: ustar headers, a name and an optional prefix, sizes in
/// octal. Directories, links and anything exotic are skipped.
pub fn untar(tar: &[u8]) -> Vec<(String, Vec<u8>)> {
    fn field(bytes: &[u8]) -> String {
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        String::from_utf8_lossy(&bytes[..end]).to_string()
    }
    let mut files = Vec::new();
    let mut at = 0;
    while at + 512 <= tar.len() {
        let header = &tar[at..at + 512];
        if header.iter().all(|b| *b == 0) {
            break;
        }
        let size = usize::from_str_radix(field(&header[124..136]).trim(), 8).unwrap_or(0);
        let start = at + 512;
        let Some(end) = start.checked_add(size).filter(|end| *end <= tar.len()) else {
            break;
        };
        let kind = header[156];
        if kind == b'0' || kind == 0 {
            let name = field(&header[0..100]);
            let prefix = field(&header[345..500]);
            let full = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let full = full.trim_start_matches("./").to_string();
            if !full.is_empty() && !full.ends_with('/') {
                files.push((full, tar[start..end].to_vec()));
            }
        }
        at = end + (512 - size % 512) % 512;
    }
    files
}

/// Compiles, and compiles again with what the cache already has for the
/// families the first compile named. Never fetches: this is the path for the
/// places that have no deployment to ask, and for a machine that is offline.
pub fn resolve_cached(
    cache: Option<&Cache>,
    mut compile: impl FnMut(&[(String, Vec<u8>)]) -> Noted,
) -> Noted {
    let noted = compile(&[]);
    let Some(cache) = cache else {
        return noted;
    };
    if noted.needs.fonts.is_empty() {
        return noted;
    }
    let library = cache.fonts_for(&noted.needs.fonts);
    if library.is_empty() {
        return noted;
    }
    compile(&library)
}

/// Compiles, fetching what each compile could not find, until the document
/// has everything or nothing more can be found. Packages come from the
/// registry; fonts from the deployment at `fonts_from`, when there is one.
/// A fetch that fails is said and the document is left with what it has: the
/// compile's diagnostics say what is missing, and a network that is down is
/// not a reason to say nothing at all.
pub async fn resolve(
    cache: Option<&Cache>,
    fonts_from: Option<&str>,
    mut compile: impl FnMut(&[(String, Vec<u8>)]) -> Noted,
) -> Noted {
    let mut library: Vec<(String, Vec<u8>)> = Vec::new();
    let mut families: Vec<String> = Vec::new();
    let mut noted = compile(&library);
    let Some(cache) = cache else {
        return noted;
    };
    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            eprintln!("warning: could not build an HTTP client to fetch packages: {err}");
            return noted;
        }
    };
    for _ in 0..ROUNDS {
        if noted.needs.is_empty() {
            break;
        }
        for family in &noted.needs.fonts {
            if !families.contains(family) {
                families.push(family.clone());
            }
        }
        let fetched = match cache.fetch(&noted.needs, fonts_from, &client).await {
            Ok(fetched) => fetched,
            Err(err) => {
                eprintln!("warning: {err}");
                false
            }
        };
        let now = cache.fonts_for(&families);
        let progressed = fetched || now.len() != library.len();
        library = now;
        if !progressed {
            break;
        }
        noted = compile(&library);
    }
    noted
}
