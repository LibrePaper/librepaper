//! Typed command plans for local build tools.
//!
//! This module contains no HTTP or browser-facing types.  Paths in a plan are
//! resolved by the companion after authorization; callers cannot provide an
//! executable or shell command.  The plan is intentionally data-only so the
//! service can apply its existing cancellation and confinement machinery.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub mod diagnostics;
pub mod runner;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuilderId {
    Tex,
    Latexmk,
    Tectonic,
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
            "tex" => Ok(Self::Tex),
            "latexmk" => Ok(Self::Latexmk),
            "tectonic" => Ok(Self::Tectonic),
            "typst" => Ok(Self::Typst),
            "pandoc" => Ok(Self::Pandoc),
            "calepin" => Ok(Self::Calepin),
            _ => Err(format!("unknown local builder: {value}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tex => "tex",
            Self::Latexmk => "latexmk",
            Self::Tectonic => "tectonic",
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
        BuilderId::Tex => tex_plan(request, tools, workspace, &entrypoint, &output)?,
        BuilderId::Latexmk => latexmk_plan(request, tools, workspace, &entrypoint, &output)?,
        BuilderId::Tectonic => tectonic_plan(request, tools, workspace, &entrypoint, &output)?,
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

fn tex_plan(
    request: &BuildRequest,
    tools: &BTreeMap<String, PathBuf>,
    workspace: &Path,
    entrypoint: &Path,
    output: &Path,
) -> Result<CommandPlan, String> {
    let engine = request.engine.as_deref().unwrap_or("pdflatex");
    if !matches!(engine, "pdflatex" | "xelatex" | "lualatex") {
        return Err(format!("unsupported TeX engine: {engine}"));
    }
    if request.output != Output::Pdf {
        return Err("TeX builders only produce PDF".into());
    }
    let mut args = vec![
        "-interaction=nonstopmode".into(),
        "-halt-on-error".into(),
        "-file-line-error".into(),
        "-no-shell-escape".into(),
        "-output-directory=out".into(),
    ];
    args.extend(fixed_tex_options(&request.options)?);
    args.push(relative_entrypoint(workspace, entrypoint)?);
    Ok(CommandPlan {
        executable: tool(tools, engine)?,
        args,
        cwd: workspace.to_path_buf(),
        output: output.to_path_buf(),
        environment: BTreeMap::new(),
    })
}

fn latexmk_plan(
    request: &BuildRequest,
    tools: &BTreeMap<String, PathBuf>,
    workspace: &Path,
    entrypoint: &Path,
    output: &Path,
) -> Result<CommandPlan, String> {
    if request.output != Output::Pdf {
        return Err("latexmk only produces PDF".into());
    }
    let engine = request.engine.as_deref().unwrap_or("pdflatex");
    if !matches!(engine, "pdflatex" | "xelatex" | "lualatex") {
        return Err(format!("unsupported latexmk engine: {engine}"));
    }
    let engine_flag = match engine {
        "pdflatex" => "-pdf",
        "xelatex" => "-xelatex",
        "lualatex" => "-lualatex",
        _ => unreachable!(),
    };
    let mut args = vec![
        "-norc".into(),
        "-interaction=nonstopmode".into(),
        "-halt-on-error".into(),
        "-file-line-error".into(),
        "-no-shell-escape".into(),
        engine_flag.into(),
        "-outdir=out".into(),
        relative_entrypoint(workspace, entrypoint)?,
    ];
    args.extend(fixed_tex_options(&request.options)?);
    let environment = helper_environment(tools);
    Ok(CommandPlan {
        executable: tool(tools, "latexmk")?,
        args,
        cwd: workspace.to_path_buf(),
        output: output.to_path_buf(),
        environment,
    })
}

fn tectonic_plan(
    request: &BuildRequest,
    tools: &BTreeMap<String, PathBuf>,
    workspace: &Path,
    entrypoint: &Path,
    output: &Path,
) -> Result<CommandPlan, String> {
    validate_options(&request.options, &["synctex"])?;
    if request.output != Output::Pdf {
        return Err("Tectonic only produces PDF".into());
    }
    Ok(CommandPlan {
        executable: tool(tools, "tectonic")?,
        args: {
            let mut args = vec![
                "--untrusted".into(),
                "--keep-logs".into(),
                "--outdir".into(),
                workspace.join("out").display().to_string(),
                entrypoint.display().to_string(),
            ];
            if request.options.get("synctex").is_some_and(|v| v == "true") {
                args.push("--synctex".into());
            }
            args
        },
        cwd: workspace.to_path_buf(),
        output: output.to_path_buf(),
        environment: BTreeMap::new(),
    })
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

fn fixed_tex_options(options: &BTreeMap<String, String>) -> Result<Vec<String>, String> {
    // Only options understood by the adapter are accepted.  In particular,
    // values are never copied into a shell command or `latexmk -e`.
    let mut args = Vec::new();
    for (key, value) in options {
        match (key.as_str(), value.as_str()) {
            ("synctex", "true") => args.push("-synctex=1".into()),
            ("synctex", "false") => args.push("-synctex=0".into()),
            _ => return Err(format!("unsupported TeX option: {key}")),
        }
    }
    Ok(args)
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

fn helper_environment(tools: &BTreeMap<String, PathBuf>) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::new();
    let mut dirs = Vec::new();
    if let Some(path) = tools.get("latexmk").and_then(|path| path.parent()) {
        dirs.push(path.display().to_string());
    }
    for tool in [
        "pdflatex",
        "xelatex",
        "lualatex",
        "bibtex",
        "biber",
        "makeindex",
    ] {
        if let Some(path) = tools.get(tool).filter(|path| path.is_absolute()) {
            if let Some(parent) = path.parent() {
                let value = parent.display().to_string();
                if !dirs.iter().any(|existing| existing == &value) {
                    dirs.push(value);
                }
            }
        }
    }
    if !dirs.is_empty() {
        environment.insert(
            "PATH".into(),
            dirs.join(if cfg!(windows) { ";" } else { ":" }),
        );
    }
    environment
}

fn relative_entrypoint(workspace: &Path, entrypoint: &Path) -> Result<String, String> {
    let relative = entrypoint
        .strip_prefix(workspace)
        .map_err(|_| "entrypoint is outside staged workspace")?;
    Ok(format!(
        "./{}",
        relative.to_string_lossy().replace('\\', "/")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_options_are_rejected_for_every_builder() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::write(root.path().join("main.tex"), "\\documentclass{article}").expect("source");
        let mut options = BTreeMap::new();
        options.insert("command".into(), "evil".into());
        for builder in [
            BuilderId::Tex,
            BuilderId::Latexmk,
            BuilderId::Tectonic,
            BuilderId::Typst,
            BuilderId::Pandoc,
            BuilderId::Calepin,
        ] {
            let request = BuildRequest {
                builder,
                engine: None,
                entrypoint: "main.tex".into(),
                output: Output::Pdf,
                options: options.clone(),
            };
            assert!(plan(&request, &BTreeMap::new(), root.path()).is_err());
        }
    }

    #[test]
    fn latexmk_uses_local_helper_paths_when_discovered() {
        let mut tools = BTreeMap::new();
        tools.insert("latexmk".into(), PathBuf::from("/bin/latexmk"));
        tools.insert("pdflatex".into(), PathBuf::from("/bin/pdflatex"));
        tools.insert("bibtex".into(), PathBuf::from("/bin/bibtex"));
        let env = helper_environment(&tools);
        assert_eq!(env.get("PATH"), Some(&"/bin".into()));
    }

    #[test]
    fn latexmk_plan_cannot_load_an_uploaded_rc_file() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            root.path().join("main.tex"),
            "\\documentclass{article}\\begin{document}x\\end{document}",
        )
        .expect("source");
        std::fs::write(root.path().join("latexmkrc"), "open F, '>marker';").expect("rc");
        let latexmk = root.path().join("latexmk");
        let pdflatex = root.path().join("pdflatex");
        std::fs::write(&latexmk, "stub").expect("latexmk stub");
        std::fs::write(&pdflatex, "stub").expect("pdflatex stub");
        let mut tools = BTreeMap::new();
        tools.insert("latexmk".into(), latexmk);
        tools.insert("pdflatex".into(), pdflatex);
        let request = BuildRequest {
            builder: BuilderId::Latexmk,
            engine: Some("pdflatex".into()),
            entrypoint: "main.tex".into(),
            output: Output::Pdf,
            options: BTreeMap::new(),
        };
        let plan = plan(&request, &tools, root.path()).expect("plan");
        assert!(plan.args.iter().any(|arg| arg == "-norc"));
    }

    #[test]
    fn dotted_entrypoint_keeps_its_stem_in_the_output_name() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            root.path().join("paper.v2.tex"),
            "\\documentclass{article}\\begin{document}x\\end{document}",
        )
        .expect("source");
        let latexmk = root.path().join("latexmk");
        let pdflatex = root.path().join("pdflatex");
        std::fs::write(&latexmk, "stub").expect("latexmk stub");
        std::fs::write(&pdflatex, "stub").expect("pdflatex stub");
        let request = BuildRequest {
            builder: BuilderId::Latexmk,
            engine: Some("pdflatex".into()),
            entrypoint: "paper.v2.tex".into(),
            output: Output::Pdf,
            options: BTreeMap::new(),
        };
        let tools = BTreeMap::from([("latexmk".into(), latexmk), ("pdflatex".into(), pdflatex)]);
        let plan = plan(&request, &tools, root.path()).expect("plan");
        assert_eq!(plan.output, root.path().join("out/paper.v2.pdf"));
    }

    #[tokio::test]
    async fn installed_native_builders_produce_declared_outputs() {
        let cases = [
            (
                "latexmk",
                BuilderId::Latexmk,
                "main.tex",
                "\\documentclass{article}\\begin{document}x\\end{document}",
                Output::Pdf,
            ),
            (
                "typst",
                BuilderId::Typst,
                "main.typ",
                "= Test\nhello",
                Output::Pdf,
            ),
            (
                "pandoc",
                BuilderId::Pandoc,
                "main.md",
                "# Test",
                Output::Html,
            ),
            (
                "tectonic",
                BuilderId::Tectonic,
                "main.tex",
                "\\documentclass{article}\\begin{document}x\\end{document}",
                Output::Pdf,
            ),
            (
                "calepin",
                BuilderId::Calepin,
                "main.typ",
                "= Test\nhello",
                Output::Pdf,
            ),
        ];
        for (tool, builder, name, source, output) in cases {
            let Some(tool_path) = std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join(tool))
                    .find(|path| path.is_file())
            }) else {
                continue;
            };
            let root = tempfile::tempdir().expect("tempdir");
            std::fs::create_dir(root.path().join("out")).expect("out");
            std::fs::write(root.path().join(name), source).expect("source");
            if builder == BuilderId::Latexmk {
                std::fs::write(
                    root.path().join(".latexmkrc"),
                    "open my $f, '>', 'latexmk-side-effect'; print $f 'executed'; close $f;",
                )
                .expect("rc");
            }
            let mut tools = BTreeMap::new();
            tools.insert(tool.into(), tool_path);
            if builder == BuilderId::Latexmk {
                let Some(engine) = std::env::var_os("PATH").and_then(|path| {
                    std::env::split_paths(&path)
                        .map(|dir| dir.join("pdflatex"))
                        .find(|path| path.is_file())
                }) else {
                    continue;
                };
                tools.insert("pdflatex".into(), engine);
            }
            let request = BuildRequest {
                builder,
                engine: (builder == BuilderId::Latexmk).then(|| "pdflatex".into()),
                entrypoint: name.into(),
                output,
                options: BTreeMap::new(),
            };
            let plan = plan(&request, &tools, root.path()).expect("plan");
            let (outcome, log) = runner::run_plan_logged(
                &plan,
                root.path(),
                tokio::sync::watch::channel(false).1,
                std::time::Instant::now() + std::time::Duration::from_secs(120),
            )
            .await;
            assert_eq!(
                outcome,
                runner::Outcome::Exited(0),
                "{tool}: {}",
                String::from_utf8_lossy(&log)
            );
            assert!(plan.output.is_file());
            if builder == BuilderId::Latexmk {
                assert!(!root.path().join("latexmk-side-effect").exists());
                if let Some(xelatex) = std::env::var_os("PATH").and_then(|path| {
                    std::env::split_paths(&path)
                        .map(|dir| dir.join("xelatex"))
                        .find(|path| path.is_file())
                }) {
                    tools.insert("xelatex".into(), xelatex);
                    let xe = BuildRequest {
                        builder: BuilderId::Latexmk,
                        engine: Some("xelatex".into()),
                        entrypoint: name.into(),
                        output: Output::Pdf,
                        options: BTreeMap::new(),
                    };
                    let xe_plan = super::plan(&xe, &tools, root.path()).expect("xelatex plan");
                    assert!(xe_plan.args.iter().any(|arg| arg == "-xelatex"));
                    let (xe_outcome, xe_log) = runner::run_plan_logged(
                        &xe_plan,
                        root.path(),
                        tokio::sync::watch::channel(false).1,
                        std::time::Instant::now() + std::time::Duration::from_secs(120),
                    )
                    .await;
                    assert_eq!(
                        xe_outcome,
                        runner::Outcome::Exited(0),
                        "xelatex: {}",
                        String::from_utf8_lossy(&xe_log)
                    );
                    assert!(xe_plan.output.is_file());
                }
            }
        }
    }
}
