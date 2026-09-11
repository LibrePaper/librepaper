//! Executable coverage for the deliberately small public CLI surface.

use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .args(args)
        .env_remove("LIBREPAPER_SERVER")
        .env_remove("LIBREPAPER_TOKEN")
        .output()
        .expect("CLI starts")
}

fn lists_command(help: &str, command: &str) -> bool {
    help.lines()
        .any(|line| line.split_whitespace().next() == Some(command))
}

#[test]
fn top_level_help_exposes_the_author_and_integration_workflows() {
    let output = cli(&["--help"]);
    assert!(output.status.success(), "{output:?}");
    let help = String::from_utf8_lossy(&output.stdout);

    for command in [
        "skills", "login", "logout", "publish", "admin", "list", "open", "sync", "export", "local",
        "quarto", "agent",
    ] {
        assert!(
            lists_command(&help, command),
            "public command {command:?} is absent from top-level help:\n{help}"
        );
    }

    for removed in [
        "comment", "edit", "share", "transfer", "history", "diff", "restore", "label", "suggest",
        "accept", "reject", "destroy",
    ] {
        assert!(
            !lists_command(&help, removed),
            "removed command {removed:?} remains in top-level help:\n{help}"
        );
    }
}

#[test]
fn operator_commands_are_only_exposed_under_admin() {
    let output = cli(&["admin", "--help"]);
    assert!(output.status.success(), "{output:?}");
    let help = String::from_utf8_lossy(&output.stdout);

    for command in [
        "serve",
        "status",
        "rotate-link-key",
        "seed",
        "backup",
        "restore-backup",
    ] {
        assert!(
            lists_command(&help, command),
            "operator command {command:?} is absent from admin help:\n{help}"
        );
    }
}

#[test]
fn removed_top_level_commands_are_rejected_by_the_parser() {
    for removed in [
        "comment", "edit", "share", "transfer", "history", "diff", "restore", "label", "suggest",
        "accept", "reject", "destroy",
    ] {
        let output = cli(&[removed]);
        assert!(
            !output.status.success(),
            "removed command {removed:?} was accepted:\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
