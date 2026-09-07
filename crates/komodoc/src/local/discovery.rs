//! Local tool discovery: find `pdflatex`, `xelatex`, `lualatex`, `bibtex`,
//! `bibtex8`, `biber` and `makeindex` as explicit absolute paths, and report
//! each individually. See `SPEC-wasmtex.md`, "Local tool discovery".
//!
//! An installed Komodoc app does not imply an installed TeX distribution;
//! finding Biber does not imply finding a complete TeX installation. This
//! module never installs anything, never modifies `PATH` or a TeX
//! installation, and never leaks a filesystem path to the browser --
//! `discover` returns `Capabilities` (paths omitted), `tool_paths` returns
//! the paths for `native.rs`'s own use.
//!
//! Results are cached under `<config home>/komodoc/local/tools.json` and
//! reused until an explicit rescan, a changed configured search path, or a
//! cached tool's executable going missing or changing on disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::process::Command;

use super::confine;
use super::protocol::{Capabilities, Confinement, Distribution, Tool, Tools};

/// The tool names this module discovers, in the order `Tools` lists them.
const TOOL_NAMES: &[&str] = &[
    "pdflatex",
    "xelatex",
    "lualatex",
    "bibtex",
    "bibtex8",
    "biber",
    "makeindex",
];

/// How long a `--version` probe is allowed to run. A hung or misbehaving
/// binary must not hang discovery.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// The resolved, absolute paths `native.rs` runs -- never sent to the
/// browser. Built from the same cache `discover` maintains.
#[derive(Clone, Debug, Default)]
pub struct ToolPaths {
    tools: BTreeMap<String, PathBuf>,
    /// The directory `PATH` should put first for a spawned tool, so a
    /// sibling binary (e.g. `biber` beside `pdflatex`) resolves the same
    /// distribution rather than a stray one earlier on the real `PATH`.
    bin_dir: Option<PathBuf>,
    /// The distribution's TEXMF root, when discovery could find one, for
    /// confinement's read-only bind list.
    texmf_root: Option<PathBuf>,
}

impl ToolPaths {
    pub fn get(&self, tool: &str) -> Option<&Path> {
        self.tools.get(tool).map(PathBuf::as_path)
    }

    pub fn bin_dir(&self) -> Option<&Path> {
        self.bin_dir.as_deref()
    }

    pub fn texmf_root(&self) -> Option<&Path> {
        self.texmf_root.as_deref()
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
    #[serde(default)]
    distribution: Option<Distribution>,
    #[serde(default)]
    bin_dir: Option<PathBuf>,
    #[serde(default)]
    texmf_root: Option<PathBuf>,
}

fn cache_path() -> PathBuf {
    crate::cli::config_home()
        .join("komodoc")
        .join("local")
        .join("tools.json")
}

fn paths_file() -> PathBuf {
    crate::cli::config_home()
        .join("komodoc")
        .join("local")
        .join("paths.txt")
}

/// The directories to search before `PATH` and the conventional locations:
/// one per line of `paths.txt`, then `$KOMODOC_TEX_PATH` (colon-separated).
pub fn configured_paths() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(text) = std::fs::read_to_string(paths_file()) {
        for line in text.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                dirs.push(PathBuf::from(trimmed));
            }
        }
    }
    if let Ok(env_path) = std::env::var("KOMODOC_TEX_PATH") {
        for part in env_path.split(':') {
            if !part.is_empty() {
                dirs.push(PathBuf::from(part));
            }
        }
    }
    dirs
}

/// PATH's directories, then the conventional install locations for TeX
/// Live, TinyTeX, MacTeX, MiKTeX -- searched after the configured ones.
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

    // TeX Live: /usr/local/texlive/<year>/bin/<arch>/
    push_children_bin(Path::new("/usr/local/texlive"), &mut dirs);

    // TinyTeX: ~/.TinyTeX/bin/<arch>/
    if let Some(home) = home_dir() {
        push_children_bin(&home.join(".TinyTeX"), &mut dirs);
    }

    // MacTeX.
    dirs.push(PathBuf::from("/Library/TeX/texbin"));

    // Windows: MiKTeX and TeX Live under Program Files and %LOCALAPPDATA%.
    for base_var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(base) = std::env::var(base_var) {
            let base = PathBuf::from(base);
            dirs.push(base.join("MiKTeX").join("miktex").join("bin").join("x64"));
            dirs.push(base.join("MiKTeX").join("miktex").join("bin"));
            push_children_bin(&base.join("texlive"), &mut dirs);
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        dirs.push(
            local
                .join("Programs")
                .join("MiKTeX")
                .join("miktex")
                .join("bin")
                .join("x64"),
        );
        dirs.push(
            local
                .join("Programs")
                .join("MiKTeX")
                .join("miktex")
                .join("bin"),
        );
    }

    dirs
}

/// `<root>/<year-or-anything>/bin/<arch>` for every immediate child of
/// `root`, the shape both TeX Live and TinyTeX use.
fn push_children_bin(root: &Path, dirs: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let bin = entry.path().join("bin");
        let Ok(arches) = std::fs::read_dir(&bin) else {
            continue;
        };
        for arch in arches.flatten() {
            if arch.path().is_dir() {
                dirs.push(arch.path());
            }
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

fn exe_name(tool: &str) -> String {
    if cfg!(windows) {
        format!("{tool}.exe")
    } else {
        tool.to_string()
    }
}

/// The first directory among `dirs` that holds `tool`, resolved to its
/// canonical absolute path. Spaces and non-ASCII bytes in a directory name
/// are preserved as-is: this never shells out to find the file, only
/// `std::fs` metadata, so nothing here re-tokenises the path.
fn find_tool(tool: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let name = exe_name(tool);
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

/// Runs `path --version` (or, for `makeindex`, a flag that at least prints
/// its usage banner -- it has no `--version`) under a cleared environment
/// and a bounded timeout, and returns the combined output.
async fn probe_version(path: &Path, tool: &str) -> Option<String> {
    let arg = if tool == "makeindex" {
        "-h"
    } else {
        "--version"
    };
    let mut command = Command::new(path);
    command
        .arg(arg)
        .env_clear()
        .stdin(std::process::Stdio::null());
    let child = command.output();
    let output = tokio::time::timeout(PROBE_TIMEOUT, child)
        .await
        .ok()?
        .ok()?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if text.trim().is_empty() {
        text = String::from_utf8_lossy(&output.stderr).into_owned();
    } else {
        text.push('\n');
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

/// The setup hint every missing tool carries.
fn setup_hint(tool: &str) -> String {
    format!(
        "{tool} not found: install TeX Live or TinyTeX, or add its bin directory to ~/.config/komodoc/local/paths.txt"
    )
}

/// Turns one tool's raw `--version` banner into the `Tool` the browser is
/// allowed to see: `biber`'s wire version is just the number (`2.21`), the
/// TeX engines and BibTeX keep their full banner line, `makeindex` has no
/// `--version` at all and is reported available with no version.
fn tool_from_banner(tool: &str, banner: Option<&str>) -> Tool {
    let Some(banner) = banner else {
        return Tool {
            available: false,
            version: None,
            note: setup_hint(tool),
        };
    };
    if tool == "makeindex" {
        if banner.contains("Usage: makeindex") || banner.to_lowercase().contains("makeindex") {
            return Tool {
                available: true,
                version: None,
                note: "found (makeindex reports no version)".to_string(),
            };
        }
        return Tool {
            available: false,
            version: None,
            note: setup_hint(tool),
        };
    }
    let first_line = banner.lines().next().unwrap_or("").trim();
    if first_line.is_empty() {
        return Tool {
            available: false,
            version: None,
            note: setup_hint(tool),
        };
    }
    if tool == "biber" {
        let version = biber_re()
            .captures(first_line)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string());
        return Tool {
            available: true,
            version,
            note: String::new(),
        };
    }
    Tool {
        available: true,
        version: Some(first_line.to_string()),
        note: String::new(),
    }
}

fn biber_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"version:\s*([0-9]+\.[0-9]+)").unwrap())
}

/// The distribution a tool's banner and path suggest, tried in the same
/// order MiKTeX / TinyTeX / TeX Live / MacTeX would be told apart: a
/// name-bearing banner wins over a path guess.
fn distribution_from(banner: &str, path: &Path) -> Option<Distribution> {
    let year_re = regex::Regex::new(r"(20[0-9]{2})").unwrap();
    let year = || {
        year_re
            .captures(banner)
            .map(|c| c[1].to_string())
            .unwrap_or_default()
    };
    if banner.contains("MiKTeX") {
        return Some(Distribution {
            name: "MiKTeX".to_string(),
            year: year(),
        });
    }
    let path_str = path.to_string_lossy();
    if path_str.contains(".TinyTeX") {
        return Some(Distribution {
            name: "TinyTeX".to_string(),
            year: year(),
        });
    }
    if banner.contains("TeX Live") {
        return Some(Distribution {
            name: "TeX Live".to_string(),
            year: year(),
        });
    }
    if path_str.contains("/Library/TeX") {
        return Some(Distribution {
            name: "MacTeX".to_string(),
            year: year(),
        });
    }
    None
}

/// A TEXMF root, from `kpsewhich`, when one of the discovered tools can
/// report it; used only for confinement's read-only bind list.
async fn texmf_root_of(tool_path: &Path) -> Option<PathBuf> {
    let kpsewhich = tool_path.parent()?.join(exe_name("kpsewhich"));
    if !kpsewhich.is_file() {
        return None;
    }
    let mut command = Command::new(&kpsewhich);
    command.arg("-var-value").arg("SELFAUTOPARENT").env_clear();
    let output = tokio::time::timeout(PROBE_TIMEOUT, command.output())
        .await
        .ok()?
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(PathBuf::from(text))
    }
}

fn platform_name() -> String {
    std::env::consts::OS.to_string()
}

/// Rebuilds the cache from scratch: searches, probes every tool, detects
/// confinement, and writes the result.
async fn rediscover(configured: Vec<PathBuf>) -> Cache {
    let mut dirs = configured.clone();
    dirs.extend(fallback_search_dirs());

    let mut tools = BTreeMap::new();
    let mut bin_dir: Option<PathBuf> = None;
    let mut texmf_root: Option<PathBuf> = None;
    let mut distribution: Option<Distribution> = None;

    for name in TOOL_NAMES {
        let found = find_tool(name, &dirs);
        let (banner, mtime) = match &found {
            Some(path) => (probe_version(path, name).await, mtime_of(path)),
            None => (None, None),
        };
        let tool = tool_from_banner(name, banner.as_deref());
        if let (Some(path), Some(banner_text)) = (&found, &banner) {
            if bin_dir.is_none() {
                bin_dir = path.parent().map(Path::to_path_buf);
            }
            if distribution.is_none() {
                distribution = distribution_from(banner_text, path);
            }
            if texmf_root.is_none() {
                texmf_root = texmf_root_of(path).await;
            }
        }
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
        confinement: confine::detect(),
        platform: platform_name(),
        distribution,
        bin_dir,
        texmf_root,
    }
}

/// Whether the cache is still trustworthy: the configured search paths have
/// not changed, and every tool the cache found is still on disk with the
/// same modification time.
fn cache_is_stale(cache: &Cache, configured: &[PathBuf]) -> bool {
    if cache.configured_paths != configured {
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
    Capabilities {
        tools: Tools {
            pdflatex: get("pdflatex"),
            xelatex: get("xelatex"),
            lualatex: get("lualatex"),
            bibtex: get("bibtex"),
            bibtex8: get("bibtex8"),
            biber: get("biber"),
            makeindex: get("makeindex"),
        },
        confinement: cache.confinement.clone(),
        platform: cache.platform.clone(),
        distribution: cache.distribution.clone(),
    }
}

fn tool_paths_from(cache: &Cache) -> ToolPaths {
    let mut tools = BTreeMap::new();
    for (name, cached) in &cache.tools {
        if let Some(path) = &cached.path {
            tools.insert(name.clone(), path.clone());
        }
    }
    ToolPaths {
        tools,
        bin_dir: cache.bin_dir.clone(),
        texmf_root: cache.texmf_root.clone(),
    }
}

/// Finds every tool and returns the browser-facing `Capabilities`, using
/// the cache unless `refresh` is set or the cache is stale (a changed
/// configured path, or a cached tool's executable missing or changed).
pub async fn discover(refresh: bool) -> Capabilities {
    capabilities_from(&discover_cache(refresh).await)
}

/// The resolved tool paths for `native.rs`, from the same cache `discover`
/// maintains. Never rescans on its own -- call `discover(true)` first if a
/// fresh scan is wanted -- so a job never pays a rescan's cost mid-run.
pub async fn tool_paths() -> ToolPaths {
    tool_paths_from(&discover_cache(false).await)
}

async fn discover_cache(refresh: bool) -> Cache {
    let configured = configured_paths();
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
    fn tool_from_banner_extracts_bibers_bare_version_number() {
        let tool = tool_from_banner("biber", Some("biber version: 2.21 (beta)\n"));
        assert!(tool.available);
        assert_eq!(tool.version.as_deref(), Some("2.21"));
    }

    #[test]
    fn tool_from_banner_keeps_the_full_line_for_tex_engines() {
        let tool = tool_from_banner(
            "pdflatex",
            Some("pdfTeX 3.141592653-2.6-1.40.27 (TeX Live 2025)\nkpathsea version 6.4.1\n"),
        );
        assert!(tool.available);
        assert_eq!(
            tool.version.as_deref(),
            Some("pdfTeX 3.141592653-2.6-1.40.27 (TeX Live 2025)")
        );
    }

    #[test]
    fn a_missing_tool_gets_a_setup_hint() {
        let tool = tool_from_banner("xelatex", None);
        assert!(!tool.available);
        assert!(tool.note.contains("paths.txt"));
    }

    #[test]
    fn distribution_detection_prefers_the_banner_name() {
        let distribution = distribution_from(
            "pdfTeX 3.14 (TeX Live 2025/nixos.org)",
            Path::new("/usr/bin/pdflatex"),
        )
        .expect("a distribution");
        assert_eq!(distribution.name, "TeX Live");
        assert_eq!(distribution.year, "2025");
    }

    #[test]
    fn configured_paths_reads_the_env_var() {
        std::env::set_var("KOMODOC_TEX_PATH", "/opt/a:/opt/b");
        let dirs = configured_paths();
        std::env::remove_var("KOMODOC_TEX_PATH");
        assert!(dirs.contains(&PathBuf::from("/opt/a")));
        assert!(dirs.contains(&PathBuf::from("/opt/b")));
    }
}
