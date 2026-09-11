//! The installed executable supplies skills without a checkout or network.

use std::process::{Command, Output};

fn cli(directory: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .current_dir(directory)
        .args(args)
        .env("LIBREPAPER_SERVER", "http://127.0.0.1:1")
        .env_remove("LIBREPAPER_TOKEN")
        .output()
        .expect("CLI starts")
}

#[test]
fn bundled_skills_work_outside_the_checkout_and_export_references() {
    let temp = tempfile::tempdir().unwrap();
    let list = cli(temp.path(), &["skills", "list"]);
    assert!(list.status.success());
    let listing = String::from_utf8(list.stdout).unwrap();
    for name in ["librepaper-document", "librepaper-pair", "librepaper-write"] {
        assert!(listing.contains(name));
        let shown = cli(temp.path(), &["skills", "show", name]);
        assert!(shown.status.success());
        assert!(String::from_utf8(shown.stdout)
            .unwrap()
            .contains(&format!("name: {name}")));
    }
    let reference = cli(
        temp.path(),
        &[
            "skills",
            "show",
            "librepaper-document",
            "--file",
            "references/editing.md",
        ],
    );
    assert!(reference.status.success());
    assert_eq!(
        reference.stdout,
        include_bytes!("../../../skills/librepaper-document/references/editing.md")
    );
    let exported = cli(temp.path(), &["skills", "export", "bundle"]);
    assert!(exported.status.success(), "{:?}", exported);
    assert_eq!(
        std::fs::read(
            temp.path()
                .join("bundle/librepaper-document/references/editing.md")
        )
        .unwrap(),
        reference.stdout
    );
    assert!(temp
        .path()
        .join("bundle/librepaper-pair/references/install.md")
        .is_file());
    let custom = temp.path().join("bundle/librepaper-write/SKILL.md");
    std::fs::write(&custom, "custom instructions").unwrap();
    assert!(
        !cli(temp.path(), &["skills", "export", "bundle"])
            .status
            .success()
    );
    assert_eq!(
        std::fs::read_to_string(custom).unwrap(),
        "custom instructions"
    );
}

#[test]
fn unknown_skills_and_escaping_paths_fail() {
    let temp = tempfile::tempdir().unwrap();
    for args in [
        vec!["skills", "show", "unknown"],
        vec!["skills", "show", "../README.md"],
        vec![
            "skills",
            "show",
            "librepaper-document",
            "--file",
            "../librepaper-write/SKILL.md",
        ],
        vec![
            "skills",
            "show",
            "librepaper-document",
            "--file",
            "/etc/passwd",
        ],
    ] {
        let output = cli(temp.path(), &args);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}
