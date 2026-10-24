//! Typed command plans for local build tools.
//!
//! This module contains no HTTP or browser-facing types.  Paths in a plan are
//! resolved by the companion after authorization; callers cannot provide an
//! executable or shell command.  The plan is intentionally data-only so the
//! service can apply its existing cancellation and deadline machinery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::local::integrations::Integration;

pub mod diagnostics;
pub mod runner;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuilderId {
    Typst,
    Calepin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Pdf,
    Html,
    Docx,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildRequest {
    pub builder: BuilderId,
    pub engine: Option<String>,
    pub entrypoint: PathBuf,
    pub output: Output,
    pub options: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandPlan {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub output: PathBuf,
    pub environment: BTreeMap<String, String>,
}

impl BuilderId {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "typst" => Ok(Self::Typst),
            "calepin" => Ok(Self::Calepin),
            _ => Err(format!("unknown local builder: {value}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Typst => "typst",
            Self::Calepin => "calepin",
        }
    }
}

impl Output {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Html => "html",
            Self::Docx => "docx",
        }
    }
}

/// Construct a safe native invocation. `tools` is companion-owned discovery
/// data, and `workspace` must be a fresh service-owned snapshot directory.
pub fn plan(
    request: &BuildRequest,
    tools: &BTreeMap<String, PathBuf>,
    workspace: &Path,
) -> Result<CommandPlan, String> {
    if !crate::local::protocol::safe_relative_path(&request.entrypoint.to_string_lossy()) {
        return Err("entrypoint must be a safe relative path".into());
    }
    let entrypoint = workspace.join(&request.entrypoint);
    if !entrypoint.is_file() {
        return Err("entrypoint is not present in the staged project".into());
    }
    let name = request
        .entrypoint
        .file_name()
        .ok_or("entrypoint has no file name")?;
    let output = workspace
        .join("out")
        .join(name)
        .with_extension(request.output.extension());
    let mut plan = match request.builder {
        BuilderId::Typst => typst_plan(request, tools, workspace, &entrypoint, &output)?,
        BuilderId::Calepin => calepin_plan(request, tools, workspace, &entrypoint, &output)?,
    };
    plan.cwd = workspace.to_path_buf();
    Ok(plan)
}

fn calepin_plan(
    request: &BuildRequest,
    tools: &BTreeMap<String, PathBuf>,
    workspace: &Path,
    entrypoint: &Path,
    output: &Path,
) -> Result<CommandPlan, String> {
    validate_options(&request.options, &[])?;
    if !matches!(request.output, Output::Pdf | Output::Html) {
        return Err("Calepin output must be PDF or HTML".into());
    }
    // From discovery, like every other native builder. This used to search
    // `PATH` again here, which could resolve a different executable from
    // the one discovery reported as available -- and reported a build
    // failure for a tool the capabilities call had just called present.
    let executable = if crate::local::integrations::get(Integration::Calepin)
        .path
        .is_some()
    {
        crate::local::integrations::executable(Integration::Calepin)
            .ok_or("custom Calepin path not found")?
    } else {
        tool(tools, "calepin")?
    };
    let mut args = vec![
        "compile".into(),
        entrypoint.display().to_string(),
        output.display().to_string(),
        "--format".into(),
        request.output.extension().into(),
    ];
    args.extend(crate::local::integrations::extra_args(Integration::Calepin));
    Ok(CommandPlan {
        executable,
        args,
        cwd: workspace.to_path_buf(),
        output: output.to_path_buf(),
        environment: BTreeMap::new(),
    })
}

fn tool(tools: &BTreeMap<String, PathBuf>, name: &str) -> Result<PathBuf, String> {
    tools
        .get(name)
        .filter(|path| path.is_file())
        .cloned()
        .ok_or_else(|| format!("tool-missing: {name}"))
}

fn typst_plan(
    request: &BuildRequest,
    tools: &BTreeMap<String, PathBuf>,
    workspace: &Path,
    entrypoint: &Path,
    output: &Path,
) -> Result<CommandPlan, String> {
    validate_options(&request.options, &[])?;
    if request.output != Output::Pdf {
        return Err("native Typst currently produces PDF only".into());
    }
    Ok(CommandPlan {
        executable: tool(tools, "typst")?,
        // The project's own font files are found wherever they sit in it,
        // the same as in the browser.
        args: vec![
            "compile".into(),
            "--root".into(),
            workspace.display().to_string(),
            "--font-path".into(),
            workspace.display().to_string(),
            entrypoint.display().to_string(),
            output.display().to_string(),
        ],
        cwd: workspace.to_path_buf(),
        output: output.to_path_buf(),
        environment: BTreeMap::new(),
    })
}

fn validate_options(options: &BTreeMap<String, String>, allowed: &[&str]) -> Result<(), String> {
    for (key, value) in options {
        if !allowed.iter().any(|allowed| allowed == key) {
            return Err(format!("unsupported builder option: {key}"));
        }
        if !matches!(value.as_str(), "true" | "false") {
            return Err(format!("builder option {key} must be boolean"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_options_are_rejected_for_every_builder() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::write(root.path().join("main.typ"), "= Title").expect("source");
        let mut options = BTreeMap::new();
        options.insert("command".into(), "evil".into());
        for builder in [BuilderId::Typst, BuilderId::Calepin] {
            let request = BuildRequest {
                builder,
                engine: None,
                entrypoint: "main.typ".into(),
                output: Output::Pdf,
                options: options.clone(),
            };
            assert!(plan(&request, &BTreeMap::new(), root.path()).is_err());
        }
    }

    #[test]
    fn typst_plan_passes_the_workspace_as_font_path() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::write(root.path().join("main.typ"), "= Title").expect("source");
        let typst = root.path().join("typst-bin");
        std::fs::write(&typst, "").expect("tool");
        let tools = BTreeMap::from([("typst".to_string(), typst)]);
        let request = BuildRequest {
            builder: BuilderId::Typst,
            engine: None,
            entrypoint: "main.typ".into(),
            output: Output::Pdf,
            options: BTreeMap::new(),
        };
        let command = plan(&request, &tools, root.path()).expect("typst plan");
        let workspace = root.path().display().to_string();
        let flag = command
            .args
            .iter()
            .position(|arg| arg == "--font-path")
            .expect("--font-path flag");
        assert_eq!(command.args[flag + 1], workspace);
    }

    // LaTeX is built in the browser: no TeX builder is nameable on the wire.
    #[test]
    fn tex_builders_are_not_part_of_the_vocabulary() {
        for name in ["tex", "latexmk", "tectonic"] {
            assert!(BuilderId::parse(name).is_err(), "{name} is still nameable");
        }
    }
}
