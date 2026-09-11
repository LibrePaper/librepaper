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

    for command in ["serve", "status", "key", "seed", "backup"] {
        assert!(
            lists_command(&help, command),
            "operator command {command:?} is absent from admin help:\n{help}"
        );
    }
}

#[test]
fn compound_resources_use_subcommand_namespaces() {
    for (path, commands) in [
        (&["admin", "key"][..], &["rotate"][..]),
        (&["admin", "backup"][..], &["create", "restore"][..]),
        (&["local", "quarto"][..], &["bind", "unbind", "list"][..]),
    ] {
        let mut args = path.to_vec();
        args.push("--help");
        let output = cli(&args);
        assert!(output.status.success(), "{output:?}");
        let help = String::from_utf8_lossy(&output.stdout);
        for command in commands {
            assert!(
                lists_command(&help, command),
                "command {command:?} is absent from `{}`:\n{help}",
                path.join(" ")
            );
        }
    }

    for removed in ["rotate-link-key", "restore-backup"] {
        let output = cli(&["admin", removed]);
        assert!(
            !output.status.success(),
            "legacy command {removed:?} survived"
        );
    }
    for removed in ["bind-quarto", "unbind-quarto", "quarto-bindings"] {
        let output = cli(&["local", removed]);
        assert!(
            !output.status.success(),
            "legacy command {removed:?} survived"
        );
    }
}

#[test]
fn operands_are_positional_and_modifiers_are_named() {
    for args in [
        &["skills", "export", "--help"][..],
        &["admin", "backup", "create", "--help"][..],
        &["admin", "backup", "restore", "--help"][..],
        &["local", "quarto", "bind", "--help"][..],
    ] {
        let output = cli(args);
        assert!(output.status.success(), "{output:?}");
        let help = String::from_utf8_lossy(&output.stdout);
        assert!(
            help.contains("Arguments:"),
            "no positional operands in:\n{help}"
        );
    }

    let output = cli(&["export", "--help"]);
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--output <FILE>"), "{help}");
    assert!(!help.contains("--out "), "legacy --out survived:\n{help}");
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
