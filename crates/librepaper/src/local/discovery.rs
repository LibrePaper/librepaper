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
//! reused until an explicit rescan, a changed configured search path, or a
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
    #[cfg(test)]
    pub(crate) fn fixture(tools: BTreeMap<String, PathBuf>) -> Self {
        Self {
            tools,
            ..Default::default()
        }
    }
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
    configured_paths: Vec<PathBuf>,
    #[serde(default)]
    tools: BTreeMap<String, CachedTool>,
    #[serde(default)]
    confinement: Confinement,
    #[serde(default)]
    platform: String,
}

fn cache_path() -> PathBuf {
    crate::cli::state_home()
        .join("librepaper")
        .join("local")
        .join("tools.json")
}

/// The directories to search before `PATH` and the conventional locations:
/// whatever `--tool-path` resolved to, flag or its matching environment
/// variable. Clap does the splitting and the environment fallback; this
/// module only ever sees the resulting list, never the environment itself.
fn configured_paths(tool_path: &[PathBuf]) -> Vec<PathBuf> {
    tool_path.to_vec()
}

/// PATH's directories, then generic fallback locations searched after the
/// configured ones when launched with a minimal PATH.
fn fallback_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.extend(conventional_dirs());
    dirs
}

fn conventional_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // Homebrew on macOS.
    dirs.push(PathBuf::from("/opt/homebrew/bin"));

    // Standard Unix/Linux prefix.
    dirs.push(PathBuf::from("/usr/local/bin"));

    // User-local binary directories.
    if let Some(home) = home_dir() {
        dirs.push(home.join(".local").join("bin"));
        dirs.push(home.join(".cargo").join("bin"));
    }

    dirs
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// The first directory among `dirs` that holds `tool`, resolved to its
/// canonical absolute path. Spaces and non-ASCII bytes in a directory name
/// are preserved as-is: this never shells out to find the file, only
/// `std::fs` metadata, so nothing here re-tokenises the path.
fn find_tool(tool: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let name = super::tools::exe_name(tool);
    for dir in dirs {
        let candidate = dir.join(&name);
        if candidate.is_file() {
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
async fn rediscover(configured: Vec<PathBuf>) -> Cache {
    let mut dirs = configured.clone();
    dirs.extend(fallback_search_dirs());

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
        configured_paths: configured,
        tools,
        confinement: Confinement {
            available: false,
            kind: "none".into(),
            reason: "local execution is authorized by explicit user consent".into(),
        },
        platform: platform_name(),
    }
}

/// Whether the cache is still trustworthy: the configured search paths have
/// not changed, and every tool the cache found is still on disk with the
/// same modification time.
fn cache_is_stale(cache: &Cache, configured: &[PathBuf]) -> bool {
    if cache.configured_paths != configured
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
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(cache) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
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
            presets: true,
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
            presets: true,
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
/// configured path, or a cached tool's executable missing or changed).
/// `tool_path` is the resolved `--tool-path` directory list.
pub async fn discover(refresh: bool, tool_path: &[PathBuf]) -> Capabilities {
    let mut capabilities = capabilities_from(&discover_cache(refresh, tool_path).await);
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
            presets: true,
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
            presets: true,
            note: String::new(),
        });
    }
    capabilities
}

/// The resolved tool paths for `native.rs`, from the same cache `discover`
/// maintains. Never rescans on its own -- call `discover(true, tool_path)`
/// first if a fresh scan is wanted -- so a job never pays a rescan's cost
/// mid-run.
pub async fn tool_paths(tool_path: &[PathBuf]) -> ToolPaths {
    tool_paths_from(&discover_cache(false, tool_path).await)
}

async fn discover_cache(refresh: bool, tool_path: &[PathBuf]) -> Cache {
    let configured = configured_paths(tool_path);
    if !refresh {
        if let Some(cache) = load_cache() {
            if !cache_is_stale(&cache, &configured) {
                return cache;
            }
        }
    }
    let cache = rediscover(configured).await;
    save_cache(&cache);
    cache
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn configured_paths_passes_the_given_directories_through() {
        let dirs = configured_paths(&[PathBuf::from("/opt/a"), PathBuf::from("/opt/b")]);
        assert!(dirs.contains(&PathBuf::from("/opt/a")));
        assert!(dirs.contains(&PathBuf::from("/opt/b")));
    }
}
