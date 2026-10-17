//! Which coding agents are on this computer, and which the sidebar can drive.
//!
//! The browser cannot see a PATH, so it cannot offer a choice worth making.
//! This module is what turns "paste these instructions into your agent and
//! hope its permission system allows them" into "here are the agents you
//! actually have installed; pick one".
//!
//! An agent is used from the document sidebar and nowhere else, so the only
//! question here is whether LibrePaper can drive it over the Agent Client
//! Protocol. LibrePaper never writes another tool's configuration: the
//! document reaches the session as an MCP server handed over ACP, which is
//! the agent's business to accept and not ours to install.
//!
//! They reach ACP by different routes, and the difference is the user's to
//! know rather than ours to hide: opencode and Gemini speak it natively,
//! Claude Code, Codex and Pi through an adapter fetched on first use.
//! `acp_note` carries a limitation of a route that
//! works but not fully.
//!
//! The built-in table below is a starting guess, not the limit.
//! Settings, Agents, Add agent in the desktop app declares any ACP-speaking
//! command as an agent this machine offers, which is how an agent LibrePaper
//! has never heard of gets driven from the sidebar, and how a changed entry
//! point is corrected without waiting for a release. That declaration goes
//! through the authenticated control API (`service/control.rs`), which only
//! the desktop shell can reach. A document page chooses *which* agent, never
//! *what command*, the same invariant the build protocol keeps by refusing a
//! caller-supplied `command` (see `protocol::BuildRequestV2::validate_shape`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::store::{read_json, write_private_json};

/// The setup page for every agent, one section each.
const DOCS: &str = "https://librepaper.org/agents.html";

#[derive(Clone, Copy, Debug)]
struct Kind {
    id: &'static str,
    label: &'static str,
    /// What to look for on PATH.
    executable: &'static str,
    /// The command that speaks the Agent Client Protocol on stdio. The first
    /// element is looked up on PATH like any other executable.
    acp: &'static [&'static str],
    /// The same adapter run straight from the npm registry, for an agent whose
    /// ACP entry point ships separately from the agent itself. Offered only
    /// when the adapter is not already on PATH, and only behind a button that
    /// says it will fetch something: this flow exists precisely so that
    /// nothing installs software without the user being told first.
    acp_fetch: Option<&'static [&'static str]>,
    /// A known limitation of this agent's ACP route that does not stop it
    /// working but does change what to expect. Shown next to the agent rather
    /// than discovered when a task mysteriously does nothing.
    acp_note: Option<&'static str>,
    /// The npm package that provides `acp[0]`, for an agent whose ACP entry
    /// point ships separately. Named when the adapter is missing.
    npm: Option<&'static str>,
    /// The anchor of this agent's section on the setup page at `DOCS`, or
    /// empty when the page has none.
    docs: &'static str,
    /// The environment variable through which the adapter is told where the
    /// agent's own executable is, so it runs the installed agent rather than
    /// a copy of its own.
    executable_env: Option<&'static str>,
    /// Environment variables always set for the adapter.
    env: &'static [(&'static str, &'static str)],
}

const KINDS: &[Kind] = &[
    Kind {
        id: "claude",
        label: "Claude Code",
        executable: "claude",
        acp: &["claude-agent-acp"],
        acp_fetch: Some(&["npx", "-y", "@agentclientprotocol/claude-agent-acp"]),
        acp_note: None,
        npm: Some("@agentclientprotocol/claude-agent-acp"),
        docs: "claude-code",
        executable_env: Some("CLAUDE_CODE_EXECUTABLE"),
        env: &[],
    },
    Kind {
        id: "codex",
        label: "Codex",
        executable: "codex",
        acp: &["codex-acp"],
        acp_fetch: Some(&["npx", "-y", "@agentclientprotocol/codex-acp"]),
        acp_note: Some(
            "A `librepaper` entry in your own Codex config makes the adapter skip LibrePaper's tools.",
        ),
        npm: Some("@agentclientprotocol/codex-acp"),
        docs: "codex",
        executable_env: Some("CODEX_PATH"),
        // In its default mode the adapter lets Codex's own reviewer approve
        // tools, so the user's Access choice is never consulted. In
        // workspace-write mode the request reaches LibrePaper.
        env: &[("INITIAL_AGENT_MODE", "workspace-write")],
    },
    Kind {
        id: "gemini",
        label: "Gemini CLI",
        executable: "gemini",
        acp: &["gemini", "--experimental-acp"],
        acp_fetch: None,
        acp_note: None,
        npm: None,
        // The setup page has no section for it yet.
        docs: "",
        executable_env: None,
        env: &[],
    },
    Kind {
        id: "opencode",
        label: "opencode",
        executable: "opencode",
        // Native ACP: the agent itself speaks it, so there is no adapter to
        // install and nothing to fetch. It also accepts MCP servers from the
        // ACP client, which is how the document tools reach the session.
        acp: &["opencode", "acp"],
        acp_fetch: None,
        acp_note: None,
        npm: None,
        docs: "opencode",
        executable_env: None,
        env: &[],
    },
    Kind {
        id: "pi",
        label: "Pi",
        executable: "pi",
        acp: &["pi-acp"],
        acp_fetch: Some(&["npx", "-y", "pi-acp"]),
        // pi-acp stores the ACP client's MCP servers and never hands them to
        // Pi. The document tools reach Pi only through the pi-mcp-adapter
        // extension, which reads the global ~/.config/mcp/mcp.json.
        acp_note: Some(
            "Pi needs the pi-mcp-adapter extension and a librepaper entry in ~/.config/mcp/mcp.json",
        ),
        npm: Some("pi-acp"),
        docs: "pi",
        executable_env: None,
        env: &[],
    },
];

/// One agent found on this computer, as the sidebar sees it.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Detected {
    pub id: String,
    pub label: String,
    /// LibrePaper can drive this agent from the sidebar.
    pub assistant: bool,
    /// Driving it would first fetch its ACP adapter from the npm registry.
    /// The sidebar labels the button with this rather than fetching quietly.
    pub assistant_fetches: bool,
    /// Why the sidebar cannot drive it, when it cannot. Empty when it can.
    /// A named missing adapter beats a greyed-out button with no explanation.
    pub assistant_blocked: String,
    /// A known limitation of an agent the sidebar *can* drive. Empty when
    /// there is nothing to warn about.
    pub assistant_note: String,
    /// This agent's section of the setup page, which the page links to.
    /// Empty for an agent added in Settings.
    pub assistant_docs: String,
}

fn kind(id: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|kind| kind.id == id)
}

/* ------------------------------------------------ Agents this machine adds */

/// An ACP agent the user added in the desktop app's settings (Settings,
/// Agents, Add agent). This is the general answer for any agent not in the
/// built-in table, including one that does not exist yet.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Custom {
    pub label: String,
    /// The command that speaks ACP on stdio, already split into arguments.
    pub command: Vec<String>,
}

#[derive(Clone)]
pub struct CustomStore {
    dir: PathBuf,
}

/// Custom agent ids share the connection-name rules: they appear in command
/// lines and in the sidebar.
///
/// A built-in id is allowed, and overrides that built-in's ACP command rather
/// than adding a second row for it. The table is a compiled-in default and
/// this machine is the authority: an agent may have changed its entry point,
/// or the user may have a locally built adapter. Overriding replaces, never
/// duplicates, so the sidebar cannot end up offering two things under one
/// name.
pub fn valid_agent_id(id: &str) -> bool {
    super::connections::valid_name(id)
}

impl CustomStore {
    pub fn new(state_home: &Path) -> Self {
        Self {
            dir: state_home.join("librepaper").join("local"),
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.join("agents.json")
    }

    fn load(&self) -> BTreeMap<String, Custom> {
        read_json(&self.path()).unwrap_or_default()
    }

    pub fn list(&self) -> Vec<(String, Custom)> {
        self.load().into_iter().collect()
    }

    pub fn get(&self, id: &str) -> Option<Custom> {
        self.load().get(id).cloned()
    }

    /// Declare an agent. The command is checked now rather than at the moment
    /// the user presses start, so a typo is a CLI error instead of a sidebar
    /// failure minutes later.
    pub fn add(&self, id: &str, label: &str, command: &[String]) -> Result<(), String> {
        if !valid_agent_id(id) {
            return Err(
                "use a short lower-case id that is not already a built-in agent name".into(),
            );
        }
        let Some(program) = command.first().filter(|part| !part.trim().is_empty()) else {
            return Err("give the command that speaks ACP on stdio".into());
        };
        if on_path(program).is_none() && !Path::new(program).is_file() {
            return Err(format!("'{program}' is not on PATH"));
        }
        let mut all = self.load();
        if all.len() >= 32 && !all.contains_key(id) {
            return Err("too many custom agents on this computer".into());
        }
        all.insert(
            id.to_string(),
            Custom {
                label: if label.trim().is_empty() {
                    id.to_string()
                } else {
                    label.trim().to_string()
                },
                command: command.to_vec(),
            },
        );
        write_private_json(&self.path(), &all).map_err(|error| error.to_string())
    }

    pub fn remove(&self, id: &str) -> bool {
        let mut all = self.load();
        let removed = all.remove(id).is_some();
        if removed {
            let _ = write_private_json(&self.path(), &all);
        }
        removed
    }

    /// Remove an agent and report persistence failures to callers that need
    /// to confirm the change, such as the companion settings API.
    pub fn try_remove(&self, id: &str) -> Result<bool, String> {
        let mut all = self.load();
        if all.remove(id).is_none() {
            return Ok(false);
        }
        write_private_json(&self.path(), &all).map_err(|error| error.to_string())?;
        Ok(true)
    }
}

/// Look up `name` on PATH. Returns the absolute path to the first match that
/// is executable, which is all a caller needs to decide whether to offer it.
pub fn on_path(name: &str) -> Option<PathBuf> {
    which::which(name).ok()
}

/// Every known agent present on this computer, in the order the sidebar
/// should offer them.
pub fn detect(state_home: &Path) -> Vec<Detected> {
    let mut found = built_in();
    // Declared agents come after the built-ins, and a declared one that is no
    // longer installed is dropped rather than offered and then failing.
    for (id, custom) in CustomStore::new(state_home).list() {
        let Some(program) = custom.command.first() else {
            continue;
        };
        let is_installed = on_path(program).is_some() || Path::new(program).is_file();
        if !is_installed {
            continue;
        }
        // A declaration for a built-in overrides how it is driven and leaves
        // the rest of the row alone, so the agent keeps the label and the
        // configuration route LibrePaper already knows for it. Its compiled-in
        // caveat goes too: it described the entry point just replaced.
        if let Some(existing) = found.iter_mut().find(|entry| entry.id == id) {
            existing.assistant = true;
            existing.assistant_fetches = false;
            existing.assistant_blocked = String::new();
            existing.assistant_note = String::new();
            continue;
        }
        found.push(Detected {
            id,
            label: custom.label,
            assistant: true,
            assistant_fetches: false,
            assistant_blocked: String::new(),
            assistant_note: String::new(),
            assistant_docs: String::new(),
        });
    }
    found
}

/// Why an agent cannot be driven without its ACP adapter, and how to set it up.
fn blocked_message(kind: &Kind) -> String {
    match kind.npm {
        Some(package) => format!(
            "{} needs its ACP adapter, which needs Node.js 22 or newer. Install Node.js, run `npm install -g {package}`, then restart LibrePaper.",
            kind.label
        ),
        None => format!(
            "Install {} to use {} in LibrePaper.",
            kind.acp[0], kind.label
        ),
    }
}

/// The setup page, at this agent's section when the page has one.
fn docs_url(kind: &Kind) -> String {
    if kind.docs.is_empty() {
        DOCS.to_string()
    } else {
        format!("{DOCS}#{}", kind.docs)
    }
}

/// The note shown next to a drivable agent. A fetched adapter also says how to
/// install it for good, so it is not fetched again on every run.
fn note(kind: &Kind, fetched: bool) -> String {
    let install = match (fetched, kind.npm) {
        (true, Some(package)) => Some(format!("To install it for good: npm install -g {package}")),
        _ => None,
    };
    [kind.acp_note.map(str::to_string), install]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
}

fn built_in() -> Vec<Detected> {
    KINDS
        .iter()
        .filter_map(|kind| {
            on_path(kind.executable)?;
            let adapter = adapter(kind);
            Some(Detected {
                id: kind.id.to_string(),
                label: kind.label.to_string(),
                assistant: adapter.is_some(),
                assistant_fetches: matches!(adapter, Some(Adapter::Fetched(_))),
                assistant_blocked: match adapter {
                    Some(_) => String::new(),
                    None => blocked_message(kind),
                },
                assistant_note: match adapter {
                    Some(found) => note(kind, matches!(found, Adapter::Fetched(_))),
                    None => String::new(),
                },
                assistant_docs: docs_url(kind),
            })
        })
        .collect()
}

/// How this machine can reach an agent's ACP entry point.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Adapter {
    /// Already on PATH; running it fetches nothing.
    Installed(&'static [&'static str]),
    /// Reachable only by fetching it from the npm registry on first run.
    Fetched(&'static [&'static str]),
}

fn adapter(kind: &'static Kind) -> Option<Adapter> {
    if on_path(kind.acp[0]).is_some() {
        return Some(Adapter::Installed(kind.acp));
    }
    // `npx` is what fetches it, so without npx there is no fetch to offer.
    let fetch = kind.acp_fetch?;
    on_path(fetch[0])?;
    Some(Adapter::Fetched(fetch))
}

/// The ACP command for an agent, when this machine can reach one. `None` means
/// the sidebar assistant cannot drive it, and the caller must say so rather
/// than substituting a different agent: which model runs is the user's choice.
pub fn acp_command(state_home: &Path, id: &str) -> Option<Vec<String>> {
    if let Some(custom) = CustomStore::new(state_home).get(id) {
        return (!custom.command.is_empty()).then_some(custom.command);
    }
    let command = match adapter(kind(id)?)? {
        Adapter::Installed(command) | Adapter::Fetched(command) => command,
    };
    Some(command.iter().map(|part| part.to_string()).collect())
}

/// Environment variables to pass to an agent process, when the adapter needs
/// to know where the agent's own executable is. Empty for a custom agent or
/// unknown id; otherwise, if the kind has `executable_env` and the executable
/// is found on PATH, returns the environment variable and absolute path.
pub fn acp_environment(state_home: &Path, id: &str) -> Vec<(String, String)> {
    if CustomStore::new(state_home).get(id).is_some() {
        return Vec::new();
    }
    kind(id).map(environment_for).unwrap_or_default()
}

fn environment_for(kind: &Kind) -> Vec<(String, String)> {
    let mut environment = kind
        .env
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect::<Vec<_>>();
    if let Some(variable) = kind.executable_env {
        if let Some(path) = on_path(kind.executable) {
            environment.push((variable.to_string(), path.to_string_lossy().into_owned()));
        }
    }
    environment
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Kind` built for the resolution tests, so they do not depend on what
    /// happens to be installed on the machine running them.
    fn kind_with(acp: &'static [&'static str], fetch: Option<&'static [&'static str]>) -> Kind {
        Kind {
            id: "test",
            label: "Test Agent",
            executable: "sh",
            acp,
            acp_fetch: fetch,
            acp_note: None,
            npm: None,
            docs: "test",
            executable_env: None,
            env: &[],
        }
    }

    #[test]
    fn an_installed_adapter_is_preferred_over_fetching_one() {
        // `sh` stands in for an adapter that is already on PATH.
        let installed = Box::leak(Box::new(kind_with(
            &["sh"],
            Some(&["npx", "-y", "@example/adapter"]),
        )));
        assert_eq!(adapter(installed), Some(Adapter::Installed(&["sh"])));
    }

    #[test]
    fn a_missing_adapter_falls_back_to_fetching_it_only_when_npx_exists() {
        let fetchable = Box::leak(Box::new(kind_with(
            &["definitely-not-installed-adapter"],
            Some(&["sh", "-c", "adapter"]),
        )));
        // `sh` stands in for npx here: the fetch is offered because the tool
        // that would perform it is present.
        assert_eq!(
            adapter(fetchable),
            Some(Adapter::Fetched(&["sh", "-c", "adapter"]))
        );

        // With no way to fetch it, there is no adapter and the sidebar must
        // say so rather than offering a button that cannot work.
        let stranded = Box::leak(Box::new(kind_with(
            &["definitely-not-installed-adapter"],
            Some(&["definitely-not-installed-fetcher"]),
        )));
        assert_eq!(adapter(stranded), None);
    }

    #[test]
    fn a_declared_agent_is_offered_and_drives_its_own_command() {
        let dir = tempfile::tempdir().unwrap();
        let store = CustomStore::new(dir.path());
        // `sh` stands in for any ACP-speaking command: what matters is that
        // an agent LibrePaper has never heard of becomes drivable without
        // touching this table.
        store
            .add("mine", "My Agent", &["sh".into(), "--acp".into()])
            .unwrap();
        let found = detect(dir.path());
        let mine = found
            .iter()
            .find(|entry| entry.id == "mine")
            .expect("declared agent");
        assert_eq!(mine.label, "My Agent");
        assert!(mine.assistant);
        assert_eq!(
            acp_command(dir.path(), "mine"),
            Some(vec!["sh".to_string(), "--acp".to_string()])
        );
        assert!(store.try_remove("mine").unwrap());
        assert!(!store.try_remove("mine").unwrap());
        assert!(detect(dir.path()).iter().all(|entry| entry.id != "mine"));
    }

    #[test]
    fn a_declaration_is_checked_when_it_is_made_not_when_start_is_pressed() {
        let dir = tempfile::tempdir().unwrap();
        let store = CustomStore::new(dir.path());
        assert!(store
            .add("mine", "", &["definitely-not-installed".into()])
            .unwrap_err()
            .contains("not on PATH"));
        assert!(store.add("mine", "", &[]).is_err());
        assert!(store.add("Not An Id", "", &["sh".into()]).is_err());
    }

    #[test]
    fn declaring_a_built_in_overrides_how_it_is_driven_without_duplicating_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = CustomStore::new(dir.path());
        // Pi's compiled-in route is the npm adapter with a known caveat. A
        // machine with its own working entry point says so, and the caveat
        // goes with the route it described.
        store.add("pi", "Renamed", &["sh".into()]).unwrap();
        let found = detect(dir.path());
        assert_eq!(
            found.iter().filter(|entry| entry.id == "pi").count(),
            1,
            "a declaration must not add a second row for the same agent"
        );
        let pi = found
            .iter()
            .find(|entry| entry.id == "pi")
            .expect("the declared command resolves, so the agent is offered");
        assert!(pi.assistant);
        assert!(pi.assistant_blocked.is_empty());
        assert!(
            pi.assistant_note.is_empty(),
            "the caveat described the replaced route"
        );
        assert_eq!(
            acp_command(dir.path(), "pi"),
            Some(vec!["sh".to_string()]),
            "the declared command is what actually runs"
        );
        if on_path("pi").is_some() {
            assert_eq!(
                pi.label, "Pi",
                "an override keeps what is known about the agent"
            );
        } else {
            assert_eq!(pi.label, "Renamed");
        }
    }

    #[test]
    fn a_known_adapter_limitation_travels_with_the_agent() {
        // pi-acp never hands the client's MCP servers to Pi. The document
        // tools are this session's only route to the document, so a silent
        // assistant is the failure mode that warning exists to prevent.
        let pi = KINDS.iter().find(|kind| kind.id == "pi").expect("pi");
        assert!(pi
            .acp_note
            .is_some_and(|note| note.contains("pi-mcp-adapter")));
        // opencode speaks ACP natively and takes MCP servers from the client,
        // so it needs no adapter, no fetch and no caveat.
        let opencode = KINDS
            .iter()
            .find(|kind| kind.id == "opencode")
            .expect("opencode");
        assert_eq!(opencode.acp, &["opencode", "acp"]);
        assert!(opencode.acp_fetch.is_none());
        assert!(opencode.acp_note.is_none());
    }

    #[test]
    fn a_declared_agent_whose_command_vanished_is_not_offered() {
        let dir = tempfile::tempdir().unwrap();
        let store = CustomStore::new(dir.path());
        store.add("mine", "Mine", &["sh".into()]).unwrap();
        // Simulate the command disappearing after it was declared.
        let path = store.path();
        let mut all: BTreeMap<String, Custom> = read_json(&path).unwrap();
        all.get_mut("mine").unwrap().command = vec!["definitely-not-installed".into()];
        write_private_json(&path, &all).unwrap();
        assert!(detect(dir.path()).iter().all(|entry| entry.id != "mine"));
    }

    #[test]
    fn an_agent_that_cannot_be_driven_says_why() {
        let dir = tempfile::tempdir().unwrap();
        // Every detected agent either offers the sidebar or explains itself.
        // A greyed-out button with no reason is the failure this replaced.
        for found in detect(dir.path()) {
            assert_eq!(
                found.assistant,
                found.assistant_blocked.is_empty(),
                "{} must either be drivable or say why not",
                found.id
            );
            if !found.assistant {
                assert!(found.assistant_blocked.contains(&found.label));
            }
        }
    }

    #[test]
    fn claude_kind_declares_executable_env() {
        let claude = KINDS
            .iter()
            .find(|kind| kind.id == "claude")
            .expect("claude");
        assert_eq!(claude.executable_env, Some("CLAUDE_CODE_EXECUTABLE"));
    }

    #[test]
    fn codex_starts_in_workspace_write_mode() {
        let codex = KINDS.iter().find(|kind| kind.id == "codex").expect("codex");
        assert_eq!(codex.executable_env, Some("CODEX_PATH"));
        assert!(codex.acp_fetch.is_some());
        assert!(environment_for(codex).contains(&(
            "INITIAL_AGENT_MODE".to_string(),
            "workspace-write".to_string()
        )));
    }

    #[test]
    fn a_kind_with_executable_on_path_and_executable_env_set_yields_env_variable() {
        let mut kind_on_path = kind_with(&["sh"], None);
        kind_on_path.executable_env = Some("TEST_EXECUTABLE");
        let env = environment_for(&kind_on_path);
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, "TEST_EXECUTABLE");
        assert!(env[0].1.ends_with("/sh"));
    }

    #[test]
    fn a_missing_executable_yields_empty_environment() {
        let mut kind_missing = kind_with(&["sh"], None);
        kind_missing.executable = "definitely-not-installed-executable";
        kind_missing.executable_env = Some("TEST_EXECUTABLE");
        let env = environment_for(&kind_missing);
        assert!(env.is_empty());
    }

    #[test]
    fn a_kind_without_executable_env_yields_empty_environment() {
        let kind_no_env = kind_with(&["sh"], None);
        let env = environment_for(&kind_no_env);
        assert!(env.is_empty());
    }

    #[test]
    fn a_blocked_claude_says_how_to_get_its_adapter() {
        let message = blocked_message(kind("claude").expect("claude"));
        assert!(message.contains("npm install -g @agentclientprotocol/claude-agent-acp"));
        assert!(message.contains("Node.js 22"));
        assert!(message.ends_with("then restart LibrePaper."));
    }

    #[test]
    fn a_blocked_agent_without_npm_says_how_to_install_its_adapter() {
        let gemini = kind("gemini").expect("gemini");
        assert_eq!(
            blocked_message(gemini),
            "Install gemini to use Gemini CLI in LibrePaper."
        );
    }

    #[test]
    fn each_agent_links_to_its_setup_section() {
        assert_eq!(
            docs_url(kind("claude").expect("claude")),
            "https://librepaper.org/agents.html#claude-code"
        );
        assert_eq!(
            docs_url(kind("codex").expect("codex")),
            "https://librepaper.org/agents.html#codex"
        );
        // No section yet: the page itself.
        assert_eq!(
            docs_url(kind("gemini").expect("gemini")),
            "https://librepaper.org/agents.html"
        );
    }

    #[test]
    fn a_fetched_adapter_says_how_to_install_it_for_good() {
        let codex = kind("codex").expect("codex");
        let claude = kind("claude").expect("claude");
        let fetched = note(codex, true);
        assert!(fetched.contains(codex.acp_note.expect("codex note")));
        assert!(fetched.contains("npm install -g @agentclientprotocol/codex-acp"));
        assert!(note(claude, false).is_empty());
        assert_eq!(
            note(claude, true),
            "To install it for good: npm install -g @agentclientprotocol/claude-agent-acp"
        );
    }
}
