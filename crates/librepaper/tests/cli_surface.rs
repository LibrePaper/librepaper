//! Executable coverage for the public CLI surface: what `--help` lists at
//! each level, which commands stay off the listing because only a machine
//! runs them, and the operand and modifier conventions the commands follow.

use std::process::{Command, Output};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .args(args)
        .env_remove("LIBREPAPER_SERVER")
        .env_remove("LIBREPAPER_TOKEN")
        .output()
        .expect("CLI starts")
}

fn help_of(args: &[&str]) -> String {
    let output = cli(args);
    assert!(output.status.success(), "{output:?}");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn lists_command(help: &str, command: &str) -> bool {
    help.lines()
        .any(|line| line.split_whitespace().next() == Some(command))
}

#[test]
fn top_level_help_lists_the_retained_workflows() {
    let help = help_of(&["--help"]);
    for command in [
        "login", "logout", "admin", "list", "export", "local", "agent",
    ] {
        assert!(
            lists_command(&help, command),
            "public command {command:?} is absent from top-level help:\n{help}"
        );
    }
}

#[test]
fn operator_commands_live_under_admin() {
    let help = help_of(&["admin", "--help"]);
    for command in ["serve", "seed", "backup"] {
        assert!(
            lists_command(&help, command),
            "operator command {command:?} is absent from admin help:\n{help}"
        );
    }
}

#[test]
fn companion_commands_people_type_are_listed() {
    let help = help_of(&["local", "--help"]);
    for command in [
        "launch",
        "start",
        "stop",
        "status",
        "doctor",
        "manage",
        "startup",
        "disconnect",
        "connections",
        "agent",
        "preset",
    ] {
        assert!(
            lists_command(&help, command),
            "companion command {command:?} is absent from local help:\n{help}"
        );
    }
    assert!(lists_command(&help_of(&["agent", "--help"]), "mcp"));
}

/// `local open` answers `librepaper://` links for the operating system and
/// `agent connect` is spawned by the local app. Both parse, neither is
/// advertised.
#[test]
fn machine_invoked_commands_parse_but_are_not_listed() {
    assert!(!lists_command(&help_of(&["local", "--help"]), "open"));
    assert!(!lists_command(&help_of(&["agent", "--help"]), "connect"));
    assert!(cli(&["local", "open", "--help"]).status.success());
    assert!(cli(&["agent", "connect", "--help"]).status.success());
}

#[test]
fn compound_resources_use_subcommand_namespaces() {
    for (path, commands) in [
        (&["admin", "backup"][..], &["create", "restore"][..]),
        (&["local", "agent"][..], &["add", "list", "remove"][..]),
        (
            &["local", "preset"][..],
            &["list", "create", "update", "remove", "grant", "revoke"][..],
        ),
    ] {
        let mut args = path.to_vec();
        args.push("--help");
        let help = help_of(&args);
        for command in commands {
            assert!(
                lists_command(&help, command),
                "command {command:?} is absent from `{}`:\n{help}",
                path.join(" ")
            );
        }
    }
}

#[test]
fn operands_are_positional_and_modifiers_are_named() {
    for args in [
        &["admin", "backup", "create", "--help"][..],
        &["admin", "backup", "restore", "--help"][..],
        &["local", "preset", "grant", "--help"][..],
    ] {
        let help = help_of(args);
        assert!(
            help.contains("Arguments:"),
            "no positional operands in:\n{help}"
        );
    }

    let help = help_of(&["export", "--help"]);
    assert!(help.contains("--output <PATH>"), "{help}");
    assert!(help.contains("--project"), "{help}");
}
