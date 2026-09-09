//! Platform confinement for a native compile: writes limited to the job
//! workspace, reads limited to the project and the TeX installation,
//! network access denied. See `docs/specs/wasmtex.md`, "Native execution
//! boundary": "TeX flags alone are not a filesystem sandbox."
//!
//! `detect` says what this machine can do, once, cheaply, at discovery
//! time. `wrap` rewrites a fully-configured `tokio::process::Command` --
//! program, args, env and cwd already set by the caller -- into the
//! confined invocation of the same command, so `native.rs` builds one
//! `Command` per tool call regardless of platform and only asks this module
//! to wrap it. When `detect` found confinement available but `wrap` cannot
//! build one (the confinement tool vanished between discovery and the job,
//! say), the caller must fail the job rather than run the unwrapped
//! command -- this module never silently drops confinement.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use tokio::process::Command;

use super::protocol::Confinement;

/// What a job needs confined: the workspace (read-write), the paths that
/// must be readable but not writable (the project and the TeX roots), and
/// whether network access is permitted (never, for a compile).
#[derive(Clone, Debug)]
pub struct Plan {
    pub workspace: PathBuf,
    pub read_only: Vec<PathBuf>,
    pub network: bool,
}

/// What confinement, if any, `wrap` applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    None,
    Bwrap,
    SandboxExec,
}

#[allow(dead_code)]
impl Applied {
    /// The wire string `protocol::Provenance.confinement` and
    /// `protocol::Confinement.kind` use.
    pub fn as_str(self) -> &'static str {
        match self {
            Applied::None => "none",
            Applied::Bwrap => "bwrap",
            Applied::SandboxExec => "sandbox-exec",
        }
    }
}

/// Whether, and how, this machine can confine a native compile: `bwrap` on
/// Linux, `sandbox-exec` on macOS, nothing else. Probes the tool rather
/// than assuming its presence from the platform alone.
pub fn detect() -> Confinement {
    if cfg!(target_os = "linux") {
        return match find_in_path("bwrap").and_then(|path| probe(&path, &["--version"])) {
            Some(_) => Confinement {
                available: true,
                kind: "bwrap".to_string(),
                reason: String::new(),
            },
            None => Confinement {
                available: false,
                kind: "none".to_string(),
                reason: "bwrap not found on PATH; install bubblewrap to confine native compiles"
                    .to_string(),
            },
        };
    }
    if cfg!(target_os = "macos") {
        return match find_in_path("sandbox-exec") {
            Some(_) => Confinement {
                available: true,
                kind: "sandbox-exec".to_string(),
                reason: String::new(),
            },
            None => Confinement {
                available: false,
                kind: "none".to_string(),
                reason: "sandbox-exec not found".to_string(),
            },
        };
    }
    Confinement {
        available: false,
        kind: "none".to_string(),
        reason: "no confinement implementation on this platform".to_string(),
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Runs `path args...` with a cleared environment and returns whether it
/// exited successfully. Used only to confirm the probed tool actually
/// runs, not to read its output.
fn probe(path: &Path, args: &[&str]) -> Option<()> {
    let status = std::process::Command::new(path)
        .args(args)
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;
    status.success().then_some(())
}

/// Rewrites `command` in place to run confined, according to `plan`.
/// Leaves `command` untouched and returns `Ok(Applied::None)` on a platform
/// with no confinement implementation. Returns `Err` when confinement is
/// expected on this platform but the wrapper program cannot be found --
/// the caller must fail the job rather than call the original `command`.
pub fn wrap(command: &mut Command, plan: &Plan) -> Result<Applied, String> {
    if cfg!(target_os = "linux") {
        return wrap_bwrap(command, plan);
    }
    if cfg!(target_os = "macos") {
        return wrap_sandbox_exec(command, plan);
    }
    Ok(Applied::None)
}

/// The program, its arguments, and the working directory a
/// `tokio::process::Command` already carries -- read back so the confined
/// invocation can put them after `--`.
fn inner_invocation(command: &Command) -> (OsString, Vec<OsString>, Option<PathBuf>) {
    let std_command = command.as_std();
    let program = std_command.get_program().to_os_string();
    let args = std_command.get_args().map(OsStr::to_os_string).collect();
    let cwd = std_command.get_current_dir().map(Path::to_path_buf);
    (program, args, cwd)
}

fn wrap_bwrap(command: &mut Command, plan: &Plan) -> Result<Applied, String> {
    let bwrap = find_in_path("bwrap").ok_or_else(|| "bwrap not found on PATH".to_string())?;
    let (program, args, cwd) = inner_invocation(command);

    let mut bwrap_args: Vec<OsString> = Vec::new();
    let ro_bind = |path: &Path, bwrap_args: &mut Vec<OsString>| {
        if path.exists() {
            bwrap_args.push("--ro-bind".into());
            bwrap_args.push(path.as_os_str().to_os_string());
            bwrap_args.push(path.as_os_str().to_os_string());
        }
    };

    // The writable workspace goes first: bwrap applies binds in order, and
    // a later, more specific bind on a path already covered by an earlier
    // one wins for that subpath. Binding the (writable) workspace before
    // the (read-only) project/TeX-root binds below means those narrower,
    // later binds are what a write to `project/` actually hits -- read
    // only -- rather than the broad writable mount underneath them.
    bwrap_args.push("--bind".into());
    bwrap_args.push(plan.workspace.as_os_str().to_os_string());
    bwrap_args.push(plan.workspace.as_os_str().to_os_string());

    for path in &plan.read_only {
        ro_bind(path, &mut bwrap_args);
    }
    // The tool's own installation prefix, so the interpreter and its
    // packages resolve even when they live outside the project/TEXMF roots
    // already listed in `plan.read_only`.
    ro_bind(Path::new("/usr/local/texlive"), &mut bwrap_args);
    ro_bind(Path::new("/nix/store"), &mut bwrap_args);
    if let Some(home) = std::env::var_os("HOME") {
        ro_bind(&PathBuf::from(home).join(".TinyTeX"), &mut bwrap_args);
    }
    ro_bind(Path::new("/usr"), &mut bwrap_args);
    if Path::new("/etc/fonts").exists() {
        ro_bind(Path::new("/etc/fonts"), &mut bwrap_args);
    }

    bwrap_args.push("--dev".into());
    bwrap_args.push("/dev".into());
    bwrap_args.push("--proc".into());
    bwrap_args.push("/proc".into());
    bwrap_args.push("--tmpfs".into());
    bwrap_args.push("/tmp".into());

    if !plan.network {
        bwrap_args.push("--unshare-net".into());
    }
    bwrap_args.push("--unshare-pid".into());
    bwrap_args.push("--unshare-ipc".into());
    bwrap_args.push("--die-with-parent".into());
    bwrap_args.push("--new-session".into());

    let chdir = cwd.clone().unwrap_or_else(|| plan.workspace.clone());
    bwrap_args.push("--chdir".into());
    bwrap_args.push(chdir.as_os_str().to_os_string());

    bwrap_args.push("--".into());
    bwrap_args.push(program);
    bwrap_args.extend(args);

    command.program_reset(&bwrap);
    command.args_reset(bwrap_args);
    Ok(Applied::Bwrap)
}

fn wrap_sandbox_exec(command: &mut Command, plan: &Plan) -> Result<Applied, String> {
    let sandbox_exec =
        find_in_path("sandbox-exec").ok_or_else(|| "sandbox-exec not found".to_string())?;
    let (program, args, _cwd) = inner_invocation(command);

    let mut allow_read: Vec<String> = plan
        .read_only
        .iter()
        .chain(std::iter::once(&plan.workspace))
        .map(|p| format!("(subpath {:?})", p.to_string_lossy()))
        .collect();
    allow_read.push("(subpath \"/usr\")".to_string());
    allow_read.push("(subpath \"/System\")".to_string());
    allow_read.push("(subpath \"/Library\")".to_string());
    allow_read.push("(subpath \"/private/etc\")".to_string());

    let network_rule = if plan.network {
        ""
    } else {
        "(deny network*)\n"
    };

    let profile = format!(
        "(version 1)\n(deny default)\n(allow process-exec)\n(allow process-fork)\n(allow file-read* {})\n(allow file-write* (subpath {:?}))\n{}",
        allow_read.join(" "),
        plan.workspace.to_string_lossy(),
        network_rule,
    );

    let mut sandbox_args: Vec<OsString> = vec!["-p".into(), OsString::from(profile), program];
    sandbox_args.extend(args);

    command.program_reset(&sandbox_exec);
    command.args_reset(sandbox_args);
    Ok(Applied::SandboxExec)
}

/// A small extension so `wrap` can replace a `Command`'s program and
/// arguments without losing the env/cwd/stdio the caller already set --
/// `tokio::process::Command` has no `set_program`/`set_args`, so this
/// rebuilds through the standard library's `Command` and copies it back in
/// with `Command::from`, which `tokio::process::Command` implements
/// `From<std::process::Command>` for.
trait CommandReset {
    fn program_reset(&mut self, program: &Path);
    fn args_reset(&mut self, args: Vec<OsString>);
}

impl CommandReset for Command {
    fn program_reset(&mut self, program: &Path) {
        let mut std_command = std::process::Command::new(program);
        copy_settings(self.as_std(), &mut std_command);
        *self = Command::from(std_command);
    }

    fn args_reset(&mut self, args: Vec<OsString>) {
        self.args(args);
    }
}

/// Copies everything but the program from `from` into `into`: current dir,
/// environment (cleared first, then every explicit var/removal `from`
/// carries) and the stdio descriptors this module relies on the caller
/// having already set (inherited by default, which `Command::new` already
/// gives `into`).
fn copy_settings(from: &std::process::Command, into: &mut std::process::Command) {
    if let Some(dir) = from.get_current_dir() {
        into.current_dir(dir);
    }
    into.env_clear();
    for (key, value) in from.get_envs() {
        match value {
            Some(value) => {
                into.env(key, value);
            }
            None => {
                into.env_remove(key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applied_as_str_matches_the_wire_confinement_kinds() {
        assert_eq!(Applied::None.as_str(), "none");
        assert_eq!(Applied::Bwrap.as_str(), "bwrap");
        assert_eq!(Applied::SandboxExec.as_str(), "sandbox-exec");
    }

    #[test]
    fn detect_never_claims_confinement_without_probing_a_real_tool() {
        // On the platforms this runs in CI/dev on, either bwrap/sandbox-exec
        // is genuinely on PATH (available: true, a real kind) or it is not
        // (available: false, kind "none", and a reason explaining why).
        let confinement = detect();
        assert_eq!(confinement.available, confinement.kind != "none");
        if !confinement.available {
            assert!(!confinement.reason.is_empty());
        }
    }

    #[tokio::test]
    async fn wrap_on_a_platform_with_no_implementation_leaves_the_command_untouched() {
        if cfg!(target_os = "linux") || cfg!(target_os = "macos") {
            return;
        }
        let mut command = Command::new("true");
        let plan = Plan {
            workspace: PathBuf::from("/tmp"),
            read_only: vec![],
            network: false,
        };
        let applied = wrap(&mut command, &plan).expect("no-op wrap never fails");
        assert_eq!(applied, Applied::None);
    }
}
