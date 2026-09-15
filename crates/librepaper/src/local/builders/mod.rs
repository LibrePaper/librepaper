//! Typed command plans for local build tools.
//!
//! This module contains no HTTP or browser-facing types.  Paths in a plan are
//! resolved by the companion after authorization; callers cannot provide an
//! executable or shell command.  The plan is intentionally data-only so the
//! service can apply its existing cancellation and deadline machinery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub mod diagnostics;
pub mod runner;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuilderId {
    Typst,
    Pandoc,
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
            "pandoc" => Ok(Self::Pandoc),
            "calepin" => Ok(Self::Calepin),
            _ => Err(format!("unknown local builder: {value}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Typst => "typst",
            Self::Pandoc => "pandoc",
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
        BuilderId::Pandoc => pandoc_plan(request, tools, workspace, &entrypoint, &output)?,
        BuilderId::Calepin => calepin_plan(request, workspace, &entrypoint, &output)?,
    };
    plan.cwd = workspace.to_path_buf();
    Ok(plan)
}

fn calepin_plan(
    request: &BuildRequest,
    workspace: &Path,
    entrypoint: &Path,
    output: &Path,
) -> Result<CommandPlan, String> {
    validate_options(&request.options, &[])?;
    if !matches!(request.output, Output::Pdf | Output::Html) {
        return Err("Calepin output must be PDF or HTML".into());
    }
    let executable =
        crate::local::preview::calepin::find_calepin().ok_or("tool-missing: calepin")?;
    Ok(CommandPlan {
        executable,
        args: vec![
            "compile".into(),
            entrypoint.display().to_string(),
            output.display().to_string(),
            "--format".into(),
            request.output.extension().into(),
        ],
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
        args: {
            let args = vec![
                "compile".into(),
                "--root".into(),
                workspace.display().to_string(),
                entrypoint.display().to_string(),
                output.display().to_string(),
            ];
            args
        },
        cwd: workspace.to_path_buf(),
        output: output.to_path_buf(),
        environment: BTreeMap::new(),
    })
}

fn pandoc_plan(
    request: &BuildRequest,
    tools: &BTreeMap<String, PathBuf>,
    workspace: &Path,
    entrypoint: &Path,
    output: &Path,
) -> Result<CommandPlan, String> {
    validate_options(&request.options, &[])?;
    if !matches!(request.output, Output::Html | Output::Docx) {
        return Err("unsupported Pandoc output".into());
    }
    Ok(CommandPlan {
        executable: tool(tools, "pandoc")?,
        args: {
            let mut args = vec![
                "--sandbox".into(),
                entrypoint.display().to_string(),
                "-o".into(),
                output.display().to_string(),
            ];
            if request.output == Output::Html {
                args.extend(["--standalone".into(), "--embed-resources".into()]);
            }
            args
        },
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
        for builder in [BuilderId::Typst, BuilderId::Pandoc, BuilderId::Calepin] {
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

    // LaTeX is built in the browser: no TeX builder is nameable on the wire.
    #[test]
    fn tex_builders_are_not_part_of_the_vocabulary() {
        for name in ["tex", "latexmk", "tectonic"] {
            assert!(BuilderId::parse(name).is_err(), "{name} is still nameable");
        }
    }
}
