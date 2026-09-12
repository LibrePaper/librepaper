//! Integration tests for the bounded native controller (`local::native`),
//! run against the real TeX Live 2025 this development machine has on
//! `PATH`: pdflatex, xelatex, lualatex, bibtex, biber, makeindex. No
//! `bwrap` is installed here, so confinement is expected to detect as
//! unavailable and every job below runs unconfined, which is itself part
//! of what is being checked -- `native.rs` must still work, and still
//! report `confinement: "none"` honestly, when the platform sandbox is
//! missing.
//!
//! A machine without those tools has nothing to run these against: each
//! test names the tool it is missing and returns, rather than failing on
//! the absence of TeX Live. CI installs TeX Live so they run there.
//!
//! Each test stages a fresh temporary workspace (`project/` + `out/`,
//! `native::run_job`'s own shape) from a corpus fixture or a small fixture
//! written inline, and drives `local::native::run_job` the same way the
//! service would: a `JobRequest`, a cancellation watch channel, and a
//! progress channel it does not have to read from.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::{mpsc, watch};

use crate::local::native;
use crate::local::protocol::{JobOptions, JobRequest, Workspace};

/// A workspace under a `TempDir` that outlives the returned `Workspace`
/// (dropping the guard removes the directory), with `project/` populated
/// from `src` -- every file except a `logs/` directory and `expected.json`,
/// which are the corpus's own fixtures for the *browser* log parser, not
/// project sources.
fn stage(src: &Path) -> (Workspace, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = Workspace {
        root: dir.path().to_path_buf(),
    };
    copy_tree(src, &workspace.project());
    (workspace, dir)
}

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create dst");
    for entry in std::fs::read_dir(src).expect("read src") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name();
        if name == "logs" || name == "expected.json" {
            continue;
        }
        let path = entry.path();
        let target = dst.join(&name);
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap_or_else(|e| panic!("copy {path:?}: {e}"));
        }
    }
}

fn corpus_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/latex/corpus")
        .join(name)
}

fn tex_request(engine: &str, main: &str, deadline_seconds: u64, max_passes: u32) -> JobRequest {
    JobRequest {
        protocol: 1,
        kind: "tex".to_string(),
        project: "test-project".to_string(),
        origin: "https://librepaper.example".to_string(),
        snapshot: "test-snapshot".to_string(),
        generation: 1,
        engine: engine.to_string(),
        main: main.to_string(),
        stem: String::new(),
        quarto: None,
        manifest: vec![],
        options: JobOptions {
            deadline_seconds,
            max_passes,
        },
        builder: None,
        workspace: None,
        entrypoint: None,
        output: None,
        builder_options: None,
        preset: None,
    }
}

fn biber_request(stem: &str, deadline_seconds: u64) -> JobRequest {
    JobRequest {
        protocol: 1,
        kind: "biber".to_string(),
        project: "test-project".to_string(),
        origin: "https://librepaper.example".to_string(),
        snapshot: "test-snapshot".to_string(),
        generation: 1,
        engine: String::new(),
        main: String::new(),
        stem: stem.to_string(),
        quarto: None,
        manifest: vec![],
        options: JobOptions {
            deadline_seconds,
            max_passes: 1,
        },
        builder: None,
        workspace: None,
        entrypoint: None,
        output: None,
        builder_options: None,
        preset: None,
    }
}

/// A no-op cancel channel and a progress channel nobody drains -- fine, it
/// is unbounded and short-lived.
fn channels() -> (
    watch::Sender<bool>,
    watch::Receiver<bool>,
    mpsc::UnboundedSender<crate::local::protocol::JobStatus>,
) {
    let (cancel_tx, cancel_rx) = watch::channel(false);
    let (progress_tx, _progress_rx) = mpsc::unbounded_channel();
    (cancel_tx, cancel_rx, progress_tx)
}

/// True, having said which tool is missing, when this machine cannot run a
/// test that needs every one of `tools` on `PATH`.
async fn skipped_without(tools: &[&str]) -> bool {
    let paths = crate::local::discovery::tool_paths(&[]).await;
    match tools.iter().find(|tool| paths.get(tool).is_none()) {
        Some(tool) => {
            eprintln!("skipped: {tool} is not on PATH; install TeX Live to run this test");
            true
        }
        None => false,
    }
}

// (a) discovery finds the real tools and parses their versions.
#[tokio::test]
async fn discovery_finds_tex_live_and_parses_versions() {
    // Rescan first: `tool_paths` answers from the cache, and this is the
    // test that refreshes it.
    let caps = crate::local::discovery::discover(true, &[]).await;
    if skipped_without(&["pdflatex", "biber"]).await {
        return;
    }

    assert!(caps.tools.pdflatex.available, "{:?}", caps.tools.pdflatex);
    assert!(
        caps.tools
            .pdflatex
            .version
            .as_deref()
            .unwrap_or_default()
            .contains("pdfTeX"),
        "{:?}",
        caps.tools.pdflatex
    );
    assert!(caps.tools.xelatex.available);
    assert!(caps.tools.lualatex.available);
    assert!(caps.tools.bibtex.available);
    assert!(caps.tools.makeindex.available);

    assert!(caps.tools.biber.available, "{:?}", caps.tools.biber);
    // The bare number out of Biber's "biber version: 2.21 (beta)" banner,
    // whichever release this machine has: 2.21 here, 2.19 on the runner.
    let biber_version = caps.tools.biber.version.as_deref().unwrap_or_default();
    assert!(
        regex::Regex::new(r"^\d+\.\d+$")
            .unwrap()
            .is_match(biber_version),
        "{:?}",
        caps.tools.biber
    );

    // Finding Biber does not imply finding a complete TeX installation, and
    // vice versa: every tool is reported on its own.
    assert_ne!(caps.tools.biber, Default::default());
    assert_ne!(caps.tools.pdflatex, Default::default());
}

// (b) a real pdfLaTeX + BibTeX job on the multi-file `paper` corpus
// converges, produces a PDF and a SyncTeX file, cites correctly, and
// reports project-relative diagnostics.
#[tokio::test]
async fn paper_corpus_compiles_cites_and_converges() {
    if skipped_without(&["pdflatex", "bibtex"]).await {
        return;
    }
    let (workspace, _guard) = stage(&corpus_dir("paper"));
    let request = tex_request("pdflatex", "main.tex", 120, 8);
    let (_cancel_tx, cancel_rx, progress) = channels();

    let outcome = native::run_job(&[], request, workspace.clone(), cancel_rx, progress).await;

    assert_eq!(outcome.status.status, "done", "{:#?}", outcome.status);
    assert!(
        outcome.files.contains_key("pdf"),
        "{:#?}",
        outcome.status.outputs
    );
    assert!(
        outcome.files.contains_key("synctex"),
        "{:#?}",
        outcome.status.outputs
    );
    assert!(!outcome.files["pdf"].is_empty());

    let bbl_path = workspace.out().join("main.bbl");
    let bbl = std::fs::read_to_string(&bbl_path).expect("main.bbl written");
    assert!(!bbl.trim().is_empty(), "bbl should not be empty");

    let final_log = String::from_utf8_lossy(&outcome.files["log"]);
    assert!(
        !final_log.contains("Citation") || !final_log.contains("undefined"),
        "final log still has undefined citations:\n{final_log}"
    );

    for diagnostic in &outcome.status.diagnostics {
        assert!(
            !diagnostic.file.starts_with('/'),
            "diagnostic file should be project-relative: {diagnostic:?}"
        );
        assert!(!diagnostic.file.contains(".."), "{diagnostic:?}");
    }

    assert_eq!(outcome.status.provenance.backend, "local");
    assert_eq!(outcome.status.provenance.engine, "pdflatex");
}

// (c) the `broken` corpus fails with the diagnostic at chapters/01.tex:7
// and produces no PDF.
#[tokio::test]
async fn broken_corpus_fails_with_the_expected_diagnostic_and_no_pdf() {
    if skipped_without(&["pdflatex"]).await {
        return;
    }
    let (workspace, _guard) = stage(&corpus_dir("broken"));
    let request = tex_request("pdflatex", "main.tex", 120, 8);
    let (_cancel_tx, cancel_rx, progress) = channels();

    let outcome = native::run_job(&[], request, workspace, cancel_rx, progress).await;

    assert_eq!(outcome.status.status, "failed", "{:#?}", outcome.status);
    assert!(
        !outcome.files.contains_key("pdf"),
        "no PDF should be produced"
    );

    let hit = outcome
        .status
        .diagnostics
        .iter()
        .find(|d| d.file == "chapters/01.tex" && d.line == 7);
    assert!(
        hit.is_some(),
        "expected chapters/01.tex:7, got {:#?}",
        outcome.status.diagnostics
    );
    assert_eq!(hit.unwrap().severity, "error");
}

// (d) + (e): a small biblatex fixture, run through Biber directly. First a
// normal run returns Unicode-intact BBL bytes; second, the same BCF with
// its control-file version rewritten reports `incompatible: true`.
fn biblatex_fixture() -> (Workspace, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = Workspace {
        root: dir.path().to_path_buf(),
    };
    let project = workspace.project();
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join("main.tex"),
        r"\documentclass{article}
\usepackage[backend=biber]{biblatex}
\addbibresource{refs.bib}
\begin{document}
\cite{knuth1984}
\cite{cafe2020}
\printbibliography
\end{document}
",
    )
    .unwrap();
    std::fs::write(
        project.join("refs.bib"),
        "@article{knuth1984,\n  author = {Knuth, Donald E.},\n  title = {Literate Programming},\n  journal = {The Computer Journal},\n  year = {1984},\n}\n\
         @book{cafe2020,\n  author = {José García},\n  title = {Café Ünïcode: Sørting Tëst},\n  year = {2020},\n}\n",
    )
    .unwrap();
    (workspace, dir)
}

/// Runs a first pdfLaTeX pass by hand (outside `native.rs`) to produce
/// `main.bcf` in the project directory, the shape a `kind: "biber"` job
/// expects its inputs already in -- this step is the browser's job in the
/// real system (a browser TeX pass), stood in for here with the real engine
/// since the point of these two tests is Biber, not TeX.
async fn produce_bcf(project: &Path) {
    let tool_paths = crate::local::discovery::tool_paths(&[]).await;
    let pdflatex = tool_paths
        .get("pdflatex")
        .expect("pdflatex discovered")
        .to_path_buf();
    let status = tokio::process::Command::new(&pdflatex)
        .args([
            "-fmt=pdflatex",
            "-interaction=nonstopmode",
            "-no-shell-escape",
            "-halt-on-error",
            "main.tex",
        ])
        .current_dir(project)
        .env_clear()
        .env(
            "PATH",
            tool_paths
                .bin_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
        )
        .env("HOME", project)
        .status()
        .await
        .expect("spawn pdflatex");
    let _ = status; // biblatex halts before \printbibliography resolves; a .bcf is still written.
    assert!(
        project.join("main.bcf").is_file(),
        "main.bcf should have been written"
    );
}

#[tokio::test]
async fn biber_job_produces_unicode_intact_bbl_bytes() {
    if skipped_without(&["pdflatex", "biber"]).await {
        return;
    }
    let (workspace, _guard) = biblatex_fixture();
    produce_bcf(&workspace.project()).await;

    let request = biber_request("main", 60);
    let (_cancel_tx, cancel_rx, progress) = channels();
    let outcome = native::run_job(&[], request, workspace, cancel_rx, progress).await;

    assert_eq!(outcome.status.status, "done", "{:#?}", outcome.status);
    assert!(!outcome.status.incompatible);
    let bbl = outcome.files.get("bbl").expect("bbl bytes");
    let text = String::from_utf8(bbl.clone()).expect("bbl is valid UTF-8");
    assert!(
        text.contains("Café Ünïcode"),
        "unicode should survive intact:\n{text}"
    );
    assert!(text.contains("Sørting Tëst"));
}

#[tokio::test]
async fn a_bcf_with_the_wrong_control_file_version_is_reported_incompatible() {
    if skipped_without(&["pdflatex", "biber"]).await {
        return;
    }
    let (workspace, _guard) = biblatex_fixture();
    let project = workspace.project();
    produce_bcf(&project).await;

    let bcf_path = project.join("main.bcf");
    let original = std::fs::read_to_string(&bcf_path).unwrap();
    let rewritten = {
        let re = regex::Regex::new(r#"controlfile version="[0-9.]+""#).unwrap();
        re.replace(&original, r#"controlfile version="9.9""#)
            .into_owned()
    };
    assert_ne!(
        original, rewritten,
        "the version attribute should have been rewritten"
    );
    std::fs::write(&bcf_path, rewritten).unwrap();

    let request = biber_request("main", 60);
    let (_cancel_tx, cancel_rx, progress) = channels();
    let outcome = native::run_job(&[], request, workspace, cancel_rx, progress).await;

    assert!(outcome.status.incompatible, "{:#?}", outcome.status);
    assert_eq!(outcome.status.status, "failed");
}

// (f) `\write18` and path-escaping `\input`/`\openout` do not escape the
// workspace: the shell command never runs and the file outside the project
// is never written.
#[tokio::test]
async fn shell_escape_and_path_escape_attempts_are_refused() {
    let (workspace, _guard) = stage_from_source(
        r"\documentclass{article}
\begin{document}
\immediate\write18{touch /tmp/librepaper-native-test-pwned}
\newwrite\myout
\immediate\openout\myout=../librepaper-native-test-escape.txt
\immediate\write\myout{leaked}
\immediate\closeout\myout
\end{document}
",
    );

    let marker = std::path::Path::new("/tmp/librepaper-native-test-pwned");
    let _ = std::fs::remove_file(marker);
    let escape_target = workspace.root.join("librepaper-native-test-escape.txt");
    let _ = std::fs::remove_file(&escape_target);

    let request = tex_request("pdflatex", "main.tex", 60, 4);
    let (_cancel_tx, cancel_rx, progress) = channels();
    let outcome = native::run_job(&[], request, workspace, cancel_rx, progress).await;

    assert!(
        !marker.exists(),
        "\\write18 must not run with -no-shell-escape and a cleared environment"
    );
    assert!(
        !escape_target.exists(),
        "openout_any=p must refuse a path outside the project"
    );
    // The document cannot finish once its \openout is refused in
    // nonstopmode -- that is the observable proof the escape did not
    // quietly succeed.
    assert_eq!(outcome.status.status, "failed", "{:#?}", outcome.status);
    let _ = std::fs::remove_file(marker);
}

fn stage_from_source(source: &str) -> (Workspace, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let workspace = Workspace {
        root: dir.path().to_path_buf(),
    };
    std::fs::create_dir_all(workspace.project()).unwrap();
    std::fs::write(workspace.project().join("main.tex"), source).unwrap();
    (workspace, dir)
}

// (g) cancellation of an infinitely-expanding document kills the process
// within 3 s and leaves no pdflatex child behind.
#[tokio::test]
async fn cancellation_kills_the_process_tree_within_three_seconds() {
    if skipped_without(&["pdflatex"]).await {
        return;
    }
    let (workspace, _guard) = stage_from_source(
        r"\documentclass{article}
\newcount\ctr
\begin{document}
\loop
\advance\ctr by 1
\ifnum\ctr<2000000000
\repeat
\end{document}
",
    );
    let workspace_marker = workspace.root.display().to_string();

    let request = tex_request("pdflatex", "main.tex", 60, 4);
    let (cancel_tx, cancel_rx, progress) = channels();

    let handle = tokio::spawn(native::run_job(
        &[],
        request,
        workspace,
        cancel_rx,
        progress,
    ));
    tokio::time::sleep(Duration::from_millis(700)).await;
    cancel_tx.send(true).expect("cancel channel open");

    let outcome = tokio::time::timeout(Duration::from_secs(3), handle)
        .await
        .expect("job should react to cancellation within 3s")
        .expect("task did not panic");

    assert_eq!(outcome.status.status, "canceled", "{:#?}", outcome.status);

    // Give the OS a moment to finish reaping, then check nothing referring
    // to this workspace is still alive.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !any_process_cmdline_contains(&workspace_marker),
        "a pdflatex process for this job is still running after cancellation"
    );
}

/// Whether any process on this machine has `needle` in its command line,
/// read from `/proc` -- the portable-enough way to check "nothing left
/// running" without depending on `pgrep` being installed.
fn any_process_cmdline_contains(needle: &str) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        let pid_name = entry.file_name();
        let Some(pid_str) = pid_name.to_str() else {
            continue;
        };
        if pid_str.chars().all(|c| c.is_ascii_digit()) {
            let cmdline_path = entry.path().join("cmdline");
            if let Ok(cmdline) = std::fs::read(&cmdline_path) {
                let text = String::from_utf8_lossy(&cmdline);
                if text.contains(needle) {
                    return true;
                }
            }
        }
    }
    false
}

// (h) exceeding the deadline fails the job with a timeout, not a hang.
#[tokio::test]
async fn a_job_that_exceeds_its_deadline_fails_with_timeout() {
    if skipped_without(&["pdflatex"]).await {
        return;
    }
    let (workspace, _guard) = stage_from_source(
        r"\documentclass{article}
\newcount\ctr
\begin{document}
\loop
\advance\ctr by 1
\ifnum\ctr<2000000000
\repeat
\end{document}
",
    );
    let request = tex_request("pdflatex", "main.tex", 2, 4);
    let (_cancel_tx, cancel_rx, progress) = channels();

    let outcome = tokio::time::timeout(
        Duration::from_secs(15),
        native::run_job(&[], request, workspace, cancel_rx, progress),
    )
    .await
    .expect("run_job must return on its own once the deadline passes, not hang");

    assert_eq!(outcome.status.status, "failed", "{:#?}", outcome.status);
    assert_eq!(outcome.status.error.as_deref(), Some("timeout"));
}

// (i) the requested engine is honoured, never substituted.
#[tokio::test]
async fn the_requested_engine_is_honoured() {
    if skipped_without(&["xelatex"]).await {
        return;
    }
    let (workspace, _guard) = stage_from_source(
        r"\documentclass{article}
\begin{document}
Hello from XeLaTeX.
\end{document}
",
    );
    let request = tex_request("xelatex", "main.tex", 60, 4);
    let (_cancel_tx, cancel_rx, progress) = channels();

    let outcome = native::run_job(&[], request, workspace, cancel_rx, progress).await;

    assert_eq!(outcome.status.status, "done", "{:#?}", outcome.status);
    assert_eq!(outcome.status.provenance.engine, "xelatex");
    let log = String::from_utf8_lossy(&outcome.files["log"]);
    assert!(
        log.contains("XeTeX"),
        "log should show the XeTeX banner:\n{log}"
    );
}

// Confinement status on this machine: no bwrap, so `detect` must report
// unavailable rather than pretending.
#[test]
fn confinement_is_reported_honestly_on_this_machine() {
    let confinement = crate::local::confine::detect();
    if cfg!(target_os = "linux") && which_missing("bwrap") {
        assert!(!confinement.available);
        assert_eq!(confinement.kind, "none");
        assert!(!confinement.reason.is_empty());
    }
}

fn which_missing(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|path| !std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
        .unwrap_or(true)
}
