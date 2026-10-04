//! Local tool discovery: find `typst`, `pandoc` and `calepin` as explicit
//! absolute paths, and report each individually.
//!
//! An installed LibrePaper app does not imply an installed builder. This
//! module never installs anything, never modifies `PATH`, and never leaks a
//! filesystem path to the browser -- `discover` returns `Capabilities`
//! (paths omitted), `tool_paths` returns the paths for the native runner's
//! own use.
//!
//! What it does not look for any more is a TeX installation. It used to
//! detect the distribution, its TEXMF root and its bin directory, tell
//! `biber`'s version format from `pdflatex`'s and answer `makeindex`'s
//! missing `--version`. None of that had a consumer: `TOOL_NAMES` names no
//! TeX tool, so the branches were unreachable, and the distribution and the
//! two directories were written to the cache and never read.
//!
//! Results are cached under `<state home>/librepaper/local/tools.json` and
//! reused until an explicit rescan, a changed PATH, or a
//! cached tool's executable going missing or changing on disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::protocol::{
    BuilderCapability, BuilderOperation, Capabilities, Confinement, Tool, Tools,
};

/// The tool names this module discovers, in the order `Tools` lists them.
const TOOL_NAMES: &[&str] = &["typst", "pandoc", "calepin"];

/// How long a `--version` probe is allowed to run. A hung or misbehaving
/// binary must not hang discovery.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// The resolved, absolute paths the builders run -- never sent to the
/// browser. Built from the same cache `discover` maintains.
#[derive(Clone, Debug, Default)]
pub struct ToolPaths {
    tools: BTreeMap<String, PathBuf>,
    versions: BTreeMap<String, String>,
}

impl ToolPaths {
    pub(crate) fn version(&self, tool: &str) -> String {
        self.versions.get(tool).cloned().unwrap_or_default()
    }
    pub(crate) fn all(&self) -> BTreeMap<String, PathBuf> {
        self.tools.clone()
    }
}

/// One tool's cached identity: where discovery found it, when it was last
/// seen (for invalidation), and the wire-shaped result.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct CachedTool {
    #[serde(default)]
    path: Option<PathBuf>,
    #[serde(default)]
    mtime: Option<i64>,
    #[serde(default)]
    tool: Tool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Cache {
    #[serde(default)]
    path_fingerprint: String,
    #[serde(default)]
    tools: BTreeMap<String, CachedTool>,
    #[serde(default)]
    confinement: Confinement,
    #[serde(default)]
    platform: String,
}

fn cache_path() -> PathBuf {
    crate::local::paths::state_home_or_die()
        .join("librepaper")
        .join("local")
        .join("tools.json")
}

/// Searches exactly the process PATH. The cache stores this fingerprint so
/// pre-PATH-only caches are discarded after upgrading.
fn search_dirs(raw: &std::ffi::OsStr) -> Vec<PathBuf> {
    std::env::split_paths(raw).collect()
}

fn path_fingerprint(raw: &std::ffi::OsStr) -> String {
    format!("path-v2:{:?}", raw)
}

fn path_snapshot() -> (Vec<PathBuf>, String) {
    let raw = std::env::var_os("PATH").unwrap_or_default();
    let dirs = search_dirs(&raw);
    let fingerprint = path_fingerprint(&raw);
    (dirs, fingerprint)
}

/// The first directory among `dirs` that holds `tool`, resolved to its
/// canonical absolute path. Spaces and non-ASCII bytes in a directory name
/// are preserved as-is: this never shells out to find the file, only
/// `std::fs` metadata, so nothing here re-tokenises the path.
fn find_tool(tool: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    for dir in dirs {
        // A path with a separator is resolved directly, so a directory name
        // holding the PATH separator is not split in two.
        if let Ok(candidate) = which::which(dir.join(tool)) {
            return std::fs::canonicalize(&candidate).ok().or(Some(candidate));
        }
    }
    None
}

fn mtime_of(path: &Path) -> Option<i64> {
    let metadata = std::fs::metadata(path).ok()?;
    let modified = metadata.modified().ok()?;
    let secs = modified.duration_since(UNIX_EPOCH).ok()?.as_secs();
    i64::try_from(secs).ok()
}

/// Runs `path --version` under a cleared environment and a bounded timeout,
/// and returns the combined output.
///
/// Isolated, unlike an adapter's probe: this is deciding what is installed,
/// and a tool that reads its configuration out of the ambient environment
/// would make that answer depend on who started the service.
async fn probe_version(path: &Path) -> Option<String> {
    let (stdout, stderr) =
        super::tools::version_output(path, super::tools::Probe::isolated(PROBE_TIMEOUT)).await?;
    let text = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    (!text.trim().is_empty()).then_some(text)
}

/// The setup hint every missing tool carries.
fn setup_hint(tool: &str) -> String {
    format!("{tool} not found: install it locally and ensure it is on the companion's PATH")
}

/// Turns one tool's raw `--version` banner into the `Tool` the browser is
/// allowed to see: the first line of the banner, which is what every
/// supported builder puts its version on.
fn tool_from_banner(tool: &str, banner: Option<&str>) -> Tool {
    let first_line = banner
        .unwrap_or_default()
        .lines()
        .next()
        .unwrap_or("")
        .trim();
    if first_line.is_empty() {
        return Tool {
            available: false,
            version: None,
            note: setup_hint(tool),
        };
    }
    Tool {
        available: true,
        version: Some(first_line.to_string()),
        note: String::new(),
    }
}

fn platform_name() -> String {
    std::env::consts::OS.to_string()
}

/// Rebuilds the cache from scratch: searches, probes every tool, detects
/// execution policy, and writes the result.
async fn rediscover(dirs: Vec<PathBuf>, path_fingerprint: String) -> Cache {
    let mut tools = BTreeMap::new();

    for name in TOOL_NAMES {
        let found = find_tool(name, &dirs);
        let (banner, mtime) = match &found {
            Some(path) => (probe_version(path).await, mtime_of(path)),
            None => (None, None),
        };
        let tool = tool_from_banner(name, banner.as_deref());
        tools.insert(
            (*name).to_string(),
            CachedTool {
                path: found,
                mtime,
                tool,
            },
        );
    }

    Cache {
        path_fingerprint,
        tools,
        confinement: Confinement {
            available: false,
            kind: "none".into(),
            reason: "local execution is authorized by explicit user consent".into(),
        },
        platform: platform_name(),
    }
}

/// Whether the cache is still trustworthy: PATH has not changed, and every
/// tool the cache found is still on disk with the same modification time.
fn cache_is_stale(cache: &Cache, path_fingerprint: &str) -> bool {
    if cache.path_fingerprint != path_fingerprint
        || TOOL_NAMES
            .iter()
            .any(|name| !cache.tools.contains_key(*name))
    {
        return true;
    }
    for cached in cache.tools.values() {
        match (&cached.path, cached.mtime) {
            (Some(path), Some(mtime)) => {
                if mtime_of(path) != Some(mtime) {
                    return true;
                }
            }
            (Some(_), None) => return true,
            (None, _) => {
                // A tool that was missing last time might have appeared
                // since (a fresh install, a new configured path); a full
                // rescan is what `refresh` is for, so an absent tool alone
                // does not force one here.
            }
        }
    }
    false
}

fn load_cache() -> Option<Cache> {
    let text = std::fs::read_to_string(cache_path()).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_cache(cache: &Cache) {
    if let Ok(text) = serde_json::to_string_pretty(cache) {
        let _ =
            librepaper_base::private_files::publish(&cache_path(), text.as_bytes(), "tool cache");
    }
}

fn capabilities_from(cache: &Cache) -> Capabilities {
    let get = |name: &str| {
        cache
            .tools
            .get(name)
            .map(|c| c.tool.clone())
            .unwrap_or_default()
    };
    let available = |name: &str| get(name).available;
    let version = |name: &str| get(name).version;
    let snapshot = || vec!["snapshot".to_string()];
    let builders = vec![
        BuilderCapability {
            id: "typst".into(),
            available: available("typst"),
            version: version("typst"),
            source_formats: vec!["typst".into()],
            outputs: vec!["pdf".into()],
            engines: Vec::new(),
            operations: vec![BuilderOperation {
                kind: "build".into(),
                workspace_modes: snapshot(),
            }],
            preview: false,
            note: String::new(),
        },
        BuilderCapability {
            id: "pandoc".into(),
            available: available("pandoc"),
            version: version("pandoc"),
            source_formats: vec!["markdown".into()],
            outputs: vec!["html".into(), "docx".into()],
            engines: Vec::new(),
            operations: vec![BuilderOperation {
                kind: "build".into(),
                workspace_modes: snapshot(),
            }],
            preview: false,
            note: String::new(),
        },
    ];
    Capabilities {
        tools: Tools {
            quarto: Tool::default(),
        },
        confinement: cache.confinement.clone(),
        platform: cache.platform.clone(),
        quarto: Default::default(),
        calepin: Default::default(),
        zotero: Default::default(),
        builders,
    }
}

fn tool_paths_from(cache: &Cache) -> ToolPaths {
    let mut tools = BTreeMap::new();
    for (name, cached) in &cache.tools {
        if !cached.tool.available {
            continue;
        }
        if let Some(path) = &cached.path {
            tools.insert(name.clone(), path.clone());
        }
    }
    ToolPaths {
        tools,
        versions: cache
            .tools
            .iter()
            .filter_map(|(name, cached)| {
                cached
                    .tool
                    .version
                    .clone()
                    .map(|version| (name.clone(), version))
            })
            .collect(),
    }
}

/// Finds every tool and returns the browser-facing `Capabilities`, using
/// the cache unless `refresh` is set or the cache is stale (a changed
/// PATH changed, or a cached tool's executable missing or changed).
pub async fn discover(refresh: bool) -> Capabilities {
    let mut capabilities = capabilities_from(&discover_cache(refresh).await);
    capabilities.quarto = crate::local::quarto::discover().await;
    capabilities.tools.quarto = capabilities.quarto.tool.clone();
    capabilities.calepin = crate::local::preview::calepin::discover().await;
    if capabilities.quarto.tool.available {
        capabilities.builders.push(BuilderCapability {
            id: "quarto".into(),
            available: true,
            version: capabilities.quarto.tool.version.clone(),
            source_formats: vec!["markdown".into(), "quarto".into()],
            outputs: capabilities.quarto.formats.clone(),
            engines: Vec::new(),
            operations: vec![
                BuilderOperation {
                    kind: "build".into(),
                    workspace_modes: vec!["snapshot".into()],
                },
                BuilderOperation {
                    kind: "preview".into(),
                    workspace_modes: vec!["bound".into()],
                },
            ],
            preview: true,
            note: String::new(),
        });
    }
    if capabilities.calepin.available {
        capabilities.builders.push(BuilderCapability {
            id: "calepin".into(),
            available: true,
            version: capabilities.calepin.version.clone(),
            source_formats: vec!["typst".into()],
            outputs: vec!["html".into(), "pdf".into()],
            engines: Vec::new(),
            operations: vec![
                BuilderOperation {
                    kind: "build".into(),
                    workspace_modes: vec!["snapshot".into()],
                },
                BuilderOperation {
                    kind: "preview".into(),
                    workspace_modes: vec!["bound".into()],
                },
            ],
            preview: true,
            note: String::new(),
        });
    }
    capabilities
}

/// The resolved tool paths for `native.rs`, from the same cache `discover`
/// maintains. Never rescans on its own -- call `discover(true)`
/// first if a fresh scan is wanted -- so a job never pays a rescan's cost
/// mid-run.
pub async fn tool_paths() -> ToolPaths {
    tool_paths_from(&discover_cache(false).await)
}

async fn discover_cache(refresh: bool) -> Cache {
    let (dirs, fingerprint) = path_snapshot();
    if !refresh {
        if let Some(cache) = load_cache() {
            if !cache_is_stale(&cache, &fingerprint) {
                return cache;
            }
        }
    }
    let cache = rediscover(dirs, fingerprint).await;
    save_cache(&cache);
    cache
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn find_tool_keeps_a_directory_name_with_the_path_separator_whole() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("tools:custom");
        std::fs::create_dir(&dir).unwrap();
        let exe = dir.join("quarto");
        std::fs::write(&exe, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let found = find_tool("quarto", std::slice::from_ref(&dir)).unwrap();
        assert_eq!(found, std::fs::canonicalize(&exe).unwrap());
    }

    #[test]
    fn tool_from_banner_keeps_the_version_line() {
        let tool = tool_from_banner("typst", Some("typst 0.13.1 (abcdef)\nsome detail\n"));
        assert!(tool.available);
        assert_eq!(tool.version.as_deref(), Some("typst 0.13.1 (abcdef)"));
    }

    #[test]
    fn a_missing_or_silent_tool_gets_a_setup_hint() {
        for banner in [None, Some("   \n")] {
            let tool = tool_from_banner("pandoc", banner);
            assert!(!tool.available);
            assert!(tool.note.contains("pandoc not found"), "{}", tool.note);
        }
    }

    #[test]
    fn cache_from_the_old_configured_path_schema_is_stale() {
        let old: Cache = serde_json::from_str(
            r#"{"configured_paths":["/legacy/tools"],"tools":{}}"#,
        )
        .unwrap();
        assert_eq!(old.path_fingerprint, "");
        assert!(cache_is_stale(&old, "path-v2:\"/current/path\""));
    }

    #[test]
    fn search_directories_are_exactly_the_supplied_path() {
        let path = std::env::join_paths([
            PathBuf::from("/first/bin"),
            PathBuf::from("/second/bin"),
        ])
        .unwrap();
        assert_eq!(
            search_dirs(&path),
            vec![PathBuf::from("/first/bin"), PathBuf::from("/second/bin")]
        );
    }

    #[test]
    fn cache_fingerprint_changes_when_path_changes() {
        let first = std::ffi::OsStr::new("/first/bin");
        let second = std::ffi::OsStr::new("/second/bin");
        assert_ne!(path_fingerprint(first), path_fingerprint(second));
    }
}
