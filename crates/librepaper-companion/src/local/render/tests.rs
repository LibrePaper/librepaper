//! The pipeline end to end, with `sh` scripts standing in for a renderer.
//! Unix only, because the scripts are shell.

use super::*;
use crate::local::protocol::{JobState, ManifestEntry};

const ORIGIN: &str = "https://paper.example";

struct Fixture {
    _state: tempfile::TempDir,
    _hosted: tempfile::TempDir,
    staged: tempfile::TempDir,
    bindings: BindingStore,
    request: JobRequest,
}

fn fixture(format: &str, output: &str, entrypoint: &str) -> Fixture {
    let state = tempfile::tempdir().unwrap();
    let hosted = tempfile::tempdir().unwrap();
    let staged = tempfile::tempdir().unwrap();
    let bytes = b"\\documentclass{article}";
    let staged_entry = staged.path().join(entrypoint);
    std::fs::create_dir_all(staged_entry.parent().unwrap()).unwrap();
    std::fs::write(staged_entry, bytes).unwrap();
    records::init(state.path());
    let bindings = BindingStore::new(hosted.path().into());
    let request = JobRequest {
        protocol: 3,
        project: "paper".into(),
        origin: ORIGIN.into(),
        snapshot: "s".into(),
        generation: 1,
        format: format.into(),
        output: output.into(),
        entrypoint: entrypoint.into(),
        binding_id: "hosted".into(),
        manifest: vec![ManifestEntry {
            path: entrypoint.into(),
            sha256: librepaper_base::digest::Digest::of(bytes).to_hex(),
            size: bytes.len() as u64,
        }],
        options: Default::default(),
    };
    Fixture {
        _state: state,
        _hosted: hosted,
        staged,
        bindings,
        request,
    }
}

fn script(key: &str, body: &str, result: &str) {
    records::set(
        Key::parse(key).unwrap(),
        Record {
            command: vec!["sh".into(), "-c".into(), body.into()],
            result: result.into(),
        },
    )
    .unwrap();
}

async fn run_fixture(fixture: &Fixture, cancelled: bool) -> JobOutcome {
    let cancel = Shutdown::new();
    if cancelled {
        cancel.cancel();
    }
    let (progress, _updates) = mpsc::unbounded_channel();
    run(
        fixture.request.clone(),
        fixture.staged.path().to_path_buf(),
        &fixture.bindings,
        cancel,
        progress,
    )
    .await
}

#[tokio::test]
// The records store is process-wide; holding its test lock across awaits is the point.
#[allow(clippy::await_holding_lock)]
async fn a_run_writes_the_files_runs_the_command_and_returns_the_result() {
    let _turn = records::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = fixture("latex", "pdf", "chapters/main.tex");
    script(
        "latex.pdf",
        "cat chapters/main.tex > chapters/main.pdf; echo sync > chapters/main.synctex.gz; echo compiled",
        "{stem}.pdf",
    );
    let outcome = run_fixture(&fixture, false).await;
    assert_eq!(
        outcome.status.status,
        JobState::Done,
        "{:?}",
        outcome.status.error
    );
    assert_eq!(outcome.status.exit, 0);
    assert_eq!(
        outcome.files["artifact"],
        b"\\documentclass{article}".to_vec()
    );
    assert_eq!(outcome.files["synctex"], b"sync\n".to_vec());
    assert!(String::from_utf8_lossy(&outcome.files["log"]).contains("compiled"));
    let names: Vec<_> = outcome.status.outputs.keys().cloned().collect();
    assert_eq!(names, ["artifact", "log", "synctex"]);
    for (name, entry) in &outcome.status.outputs {
        assert_eq!(
            *entry,
            protocol::OutputEntry::from_bytes(&outcome.files[name])
        );
    }
    assert_eq!(
        outcome.status.provenance,
        Provenance {
            backend: "local".into(),
            format: "latex".into(),
            output: "pdf".into()
        }
    );
    // The workspace keeps the file the job wrote, and the command's own output.
    let root = fixture
        .bindings
        .get_scoped("hosted", ORIGIN, "paper")
        .unwrap()
        .root;
    assert!(root.join("chapters/main.tex").is_file());
    assert!(root.join("chapters/main.pdf").is_file());
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn only_a_pdf_run_reads_synctex() {
    let _turn = records::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = fixture("quarto", "html", "paper.qmd");
    script(
        "quarto.html",
        "echo '<html></html>' > paper.html; echo sync > paper.synctex",
        "{stem}.html",
    );
    let run_dir = tempfile::tempdir().unwrap();
    let run_path = std::fs::canonicalize(run_dir.path()).unwrap();
    let outcome = run_fixture_in(&fixture, Some(run_path)).await;
    assert_eq!(
        outcome.status.status,
        JobState::Done,
        "{:?}",
        outcome.status.error
    );
    assert!(!outcome.files.contains_key("synctex"));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn exit_zero_with_no_result_file_fails_and_names_the_expected_path() {
    let _turn = records::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = fixture("typst", "pdf", "docs/paper.typ");
    script("typst.pdf", "true", "{stem}.pdf");
    let outcome = run_fixture(&fixture, false).await;
    assert_eq!(outcome.status.status, JobState::Failed);
    let error = outcome.status.error.unwrap();
    assert!(error.contains("docs/paper.pdf"), "{error}");
    assert!(!outcome.files.contains_key("artifact"));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn a_failing_run_still_returns_the_result_it_wrote() {
    let _turn = records::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = fixture("typst", "pdf", "paper.typ");
    script(
        "typst.pdf",
        "cat paper.typ > paper.pdf; echo 'error: broken' >&2; exit 1",
        "{stem}.pdf",
    );
    let outcome = run_fixture(&fixture, false).await;
    assert_eq!(outcome.status.status, JobState::Failed);
    assert_eq!(outcome.status.exit, 1);
    assert!(outcome.status.log_tail.contains("error: broken"));
    assert_eq!(
        outcome.files["artifact"],
        b"\\documentclass{article}".to_vec()
    );
}

/// Dates the files `names` of the workspace a minute back, as an earlier run
/// leaves them.
fn age(fixture: &Fixture, names: &[&str]) {
    let root = fixture
        .bindings
        .get_scoped("hosted", ORIGIN, "paper")
        .unwrap()
        .root;
    for name in names {
        std::fs::File::options()
            .write(true)
            .open(root.join(name))
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(60))
            .unwrap();
    }
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn a_successful_run_that_rewrites_nothing_still_returns_the_result() {
    let _turn = records::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = fixture("latex", "pdf", "main.tex");
    script(
        "latex.pdf",
        "cat main.tex > main.pdf; echo sync > main.synctex.gz",
        "{stem}.pdf",
    );
    let first = run_fixture(&fixture, false).await;
    assert_eq!(
        first.status.status,
        JobState::Done,
        "{:?}",
        first.status.error
    );
    age(&fixture, &["main.pdf", "main.synctex.gz"]);

    // latexmk on unchanged inputs: exit 0, nothing written.
    script("latex.pdf", "true", "{stem}.pdf");
    let outcome = run_fixture(&fixture, false).await;
    assert_eq!(
        outcome.status.status,
        JobState::Done,
        "{:?}",
        outcome.status.error
    );
    assert_eq!(
        outcome.files["artifact"],
        b"\\documentclass{article}".to_vec()
    );
    assert_eq!(outcome.files["synctex"], b"sync\n".to_vec());
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn a_failing_run_does_not_return_a_result_an_earlier_run_left() {
    let _turn = records::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = fixture("typst", "pdf", "paper.typ");
    script("typst.pdf", "cat paper.typ > paper.pdf", "{stem}.pdf");
    let first = run_fixture(&fixture, false).await;
    assert_eq!(
        first.status.status,
        JobState::Done,
        "{:?}",
        first.status.error
    );
    age(&fixture, &["paper.pdf"]);

    script(
        "typst.pdf",
        "echo 'error: broken' >&2; exit 1",
        "{stem}.pdf",
    );
    let outcome = run_fixture(&fixture, false).await;
    assert_eq!(outcome.status.status, JobState::Failed);
    assert_eq!(outcome.status.exit, 1);
    assert!(!outcome.files.contains_key("artifact"));
    assert!(!outcome.files.contains_key("synctex"));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn a_failing_command_reports_its_status_and_its_log() {
    let _turn = records::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = fixture("typst", "pdf", "paper.typ");
    script(
        "typst.pdf",
        "echo 'error: broken' >&2; exit 3",
        "{stem}.pdf",
    );
    let outcome = run_fixture(&fixture, false).await;
    assert_eq!(outcome.status.status, JobState::Failed);
    assert_eq!(outcome.status.exit, 3);
    assert!(outcome.status.log_tail.contains("error: broken"));
    assert!(outcome.files.contains_key("log"));
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn a_missing_program_fails_before_anything_is_written() {
    let _turn = records::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = fixture("typst", "pdf", "paper.typ");
    records::set(
        Key::parse("typst.pdf").unwrap(),
        Record {
            command: vec!["librepaper-no-such-renderer".into(), "{input}".into()],
            result: "{stem}.pdf".into(),
        },
    )
    .unwrap();
    let outcome = run_fixture(&fixture, false).await;
    assert_eq!(outcome.status.status, JobState::Failed);
    assert!(outcome
        .status
        .error
        .unwrap()
        .contains("librepaper-no-such-renderer was not found"));
    let root = fixture
        .bindings
        .get_scoped("hosted", ORIGIN, "paper")
        .unwrap()
        .root;
    assert!(!root.join("paper.typ").exists());
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn a_canceled_job_ends_canceled() {
    let _turn = records::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = fixture("typst", "pdf", "paper.typ");
    script("typst.pdf", "sleep 30", "{stem}.pdf");
    let outcome = run_fixture(&fixture, true).await;
    assert_eq!(outcome.status.status, JobState::Canceled);
}

#[test]
fn a_result_that_resolves_outside_the_workspace_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.pdf"), b"secret").unwrap();
    assert_eq!(read_inside(&root, "absent.pdf"), Ok(None));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path().join("secret.pdf"), root.join("out.pdf"))
            .unwrap();
        assert!(read_inside(&root, "out.pdf").is_err());
    }
}

async fn run_fixture_in(fixture: &Fixture, run_dir: Option<PathBuf>) -> JobOutcome {
    let (progress, _updates) = mpsc::unbounded_channel();
    run_with(
        fixture.request.clone(),
        fixture.staged.path().to_path_buf(),
        &fixture.bindings,
        Shutdown::new(),
        progress,
        run_dir,
    )
    .await
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn quarto_without_a_run_directory_is_refused_before_anything_runs() {
    let _turn = records::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = fixture("quarto", "html", "paper.qmd");
    script(
        "quarto.html",
        "touch ran; echo '<html></html>' > paper.html",
        "{stem}.html",
    );
    let outcome = run_fixture_in(&fixture, None).await;
    assert_eq!(outcome.status.status, JobState::Failed);
    assert_eq!(
        outcome.status.error.as_deref(),
        Some("Choose a Run directory before running Quarto code.")
    );
    let root = fixture
        .bindings
        .get_scoped("hosted", ORIGIN, "paper")
        .unwrap()
        .root;
    assert!(!root.join("ran").exists());
    assert!(!root.join("paper.qmd").exists());
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn quarto_gets_execute_dir_and_the_run_directory_linked_in() {
    let _turn = records::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = fixture("quarto", "html", "paper.qmd");
    let run_dir = tempfile::tempdir().unwrap();
    let run_path = std::fs::canonicalize(run_dir.path()).unwrap();
    std::fs::write(run_path.join("data.csv"), "1,2").unwrap();
    // The script prints its own arguments (after `sh -c body sh`), so the
    // appended flag shows up in the result.
    script(
        "quarto.html",
        "echo \"$0 $@\" > paper.html; cat data.csv >> paper.html",
        "{stem}.html",
    );
    let outcome = run_fixture_in(&fixture, Some(run_path.clone())).await;
    assert_eq!(
        outcome.status.status,
        JobState::Done,
        "{:?}",
        outcome.status.error
    );
    let html = String::from_utf8_lossy(&outcome.files["artifact"]).into_owned();
    assert!(
        html.contains(&format!("--execute-dir {}", run_path.display())),
        "{html}"
    );
    assert!(html.contains("1,2"), "{html}");
    assert!(outcome.status.notices.is_empty());
}

#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn a_non_quarto_run_needs_no_run_directory_and_removes_old_links() {
    let _turn = records::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = fixture("latex", "pdf", "main.tex");
    script("latex.pdf", "echo ok > main.pdf", "{stem}.pdf");
    let root = fixture
        .bindings
        .get_scoped("hosted", ORIGIN, "paper")
        .unwrap()
        .root;
    let run_dir = tempfile::tempdir().unwrap();
    std::fs::write(run_dir.path().join("x.csv"), "x").unwrap();
    let manifest_paths = ["main.tex".to_string()].into_iter().collect();
    workspace::link_run_directory(&root, run_dir.path(), &manifest_paths).unwrap();
    assert!(root.join("x.csv").exists());
    let outcome = run_fixture_in(&fixture, None).await;
    assert_eq!(outcome.status.status, JobState::Done);
    assert!(std::fs::symlink_metadata(root.join("x.csv")).is_err());
    assert!(run_dir.path().join("x.csv").exists());
}

/// The default commands against the real tools. Run with `--ignored` on a
/// machine that has them (`tools/test/suite external`).
async fn render_with_the_default_command(
    format: &str,
    output: &str,
    entrypoint: &str,
    source: &str,
) -> JobOutcome {
    let fixture = fixture(format, output, entrypoint);
    std::fs::write(fixture.staged.path().join(entrypoint), source).unwrap();
    let mut fixture = fixture;
    fixture.request.manifest[0].sha256 =
        librepaper_base::digest::Digest::of(source.as_bytes()).to_hex();
    fixture.request.manifest[0].size = source.len() as u64;
    run_fixture(&fixture, false).await
}

#[tokio::test]
#[ignore = "requires installed Quarto"]
#[allow(clippy::await_holding_lock)]
async fn real_quarto_renders_html_with_its_default_command() {
    let _turn = records::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let outcome =
        render_with_the_default_command("quarto", "html", "paper.qmd", "# Title\n\nBody.\n").await;
    assert_eq!(
        outcome.status.status,
        JobState::Done,
        "{}",
        outcome.status.log_tail
    );
    assert!(String::from_utf8_lossy(&outcome.files["artifact"]).contains("Body."));
}

/// Whether `program` runs and, when `args` are given, exits successfully.
fn tool_works(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Every path under `dir`, relative to it, sorted.
fn listing(dir: &Path) -> Vec<String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            out.push(path.strip_prefix(base).unwrap().display().to_string());
            if path.is_dir() {
                walk(base, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

#[tokio::test]
#[ignore = "requires installed Quarto, R, knitr and rmarkdown"]
#[allow(clippy::await_holding_lock)]
async fn real_quarto_runs_code_in_the_run_directory() {
    let packages =
        r#"quit(status = !all(sapply(c("knitr","rmarkdown"), requireNamespace, quietly = TRUE)))"#;
    if !tool_works("quarto", &["--version"])
        || !tool_works("Rscript", &["--version"])
        || !tool_works("Rscript", &["-e", packages])
    {
        eprintln!("skipped: needs quarto, Rscript, knitr and rmarkdown");
        return;
    }
    let _turn = records::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // A 1x1 PNG.
    const DOT: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0xf8,
        0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    let source = "---\ntitle: Run\n---\n\n```{r}\nsource(\"R/helpers.R\"); d <- read.csv(\"data/d.csv\"); cat(\"rows\", nrow(d), helper())\n```\n\n![fig](figures/dot.png)\n";
    let fixture = fixture("quarto", "html", "paper.qmd");
    std::fs::write(fixture.staged.path().join("paper.qmd"), source).unwrap();
    let mut fixture = fixture;
    fixture.request.manifest[0].sha256 =
        librepaper_base::digest::Digest::of(source.as_bytes()).to_hex();
    fixture.request.manifest[0].size = source.len() as u64;
    let run_dir = tempfile::tempdir().unwrap();
    let run_path = std::fs::canonicalize(run_dir.path()).unwrap();
    for dir in ["R", "data", "figures"] {
        std::fs::create_dir(run_path.join(dir)).unwrap();
    }
    std::fs::write(
        run_path.join("R/helpers.R"),
        "helper <- function() \"from-run-dir\"\n",
    )
    .unwrap();
    std::fs::write(run_path.join("data/d.csv"), "a,b\n1,2\n3,4\n5,6\n").unwrap();
    std::fs::write(run_path.join("figures/dot.png"), DOT).unwrap();
    let before = listing(&run_path);

    let outcome = run_fixture_in(&fixture, Some(run_path.clone())).await;

    assert_eq!(
        outcome.status.status,
        JobState::Done,
        "{}",
        outcome.status.log_tail
    );
    let html = String::from_utf8_lossy(&outcome.files["artifact"]).into_owned();
    assert!(html.contains("rows 3"), "{html}");
    assert!(html.contains("from-run-dir"), "{html}");
    assert!(html.contains("data:image/png"), "{html}");
    assert_eq!(listing(&run_path), before);
}

#[tokio::test]
#[ignore = "requires installed Typst"]
#[allow(clippy::await_holding_lock)]
async fn real_typst_renders_a_pdf_with_its_default_command() {
    let _turn = records::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let outcome =
        render_with_the_default_command("typst", "pdf", "docs/paper.typ", "= Title\n").await;
    assert_eq!(
        outcome.status.status,
        JobState::Done,
        "{}",
        outcome.status.log_tail
    );
    assert!(outcome.files["artifact"].starts_with(b"%PDF-"));
}

#[tokio::test]
#[ignore = "requires installed latexmk and TeX Live"]
#[allow(clippy::await_holding_lock)]
async fn real_latexmk_renders_a_pdf_and_synctex_with_its_default_command() {
    let _turn = records::TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let outcome = render_with_the_default_command(
        "latex",
        "pdf",
        "chapters/main.tex",
        "\\documentclass{article}\\begin{document}Hello\\end{document}\n",
    )
    .await;
    assert_eq!(
        outcome.status.status,
        JobState::Done,
        "{}",
        outcome.status.log_tail
    );
    assert!(outcome.files["artifact"].starts_with(b"%PDF-"));
    assert!(outcome.files.contains_key("synctex"));
}
