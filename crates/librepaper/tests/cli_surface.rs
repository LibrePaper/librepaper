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
fn top_level_help_lists_exactly_the_public_commands() {
    let help = help_of(&["--help"]);
    for command in ["login", "logout", "list", "export", "mcp", "local", "admin"] {
        assert!(
            lists_command(&help, command),
            "public command {command:?} is absent from top-level help:\n{help}"
        );
    }
    assert!(
        !lists_command(&help, "agent"),
        "machine command 'agent' should not be listed in top-level help:\n{help}"
    );
    assert!(
        !lists_command(&help, "run-agent"),
        "machine command 'run-agent' should not be listed in top-level help:\n{help}"
    );
}

#[test]
fn admin_help_lists_the_admin_commands() {
    let help = help_of(&["admin", "--help"]);
    for command in ["serve", "seed", "backup", "restore", "sweep"] {
        assert!(
            lists_command(&help, command),
            "admin command {command:?} is absent from admin help:\n{help}"
        );
    }
}

#[test]
fn local_help_lists_the_local_commands() {
    let help = help_of(&["local", "--help"]);
    for command in ["start", "stop", "status", "settings", "agent"] {
        assert!(
            lists_command(&help, command),
            "local command {command:?} is absent from local help:\n{help}"
        );
    }
    for removed_command in ["launch", "manage", "doctor", "connections", "disconnect", "startup", "preset"] {
        assert!(
            !lists_command(&help, removed_command),
            "removed command {removed_command:?} should not be listed in local help:\n{help}"
        );
    }
}

/// `local open` answers `librepaper://` links for the operating system and
/// `run-agent` is spawned by the local app. Both parse, neither is
/// advertised.
#[test]
fn machine_invoked_commands_parse_but_are_not_listed() {
    assert!(
        !lists_command(&help_of(&["local", "--help"]), "open"),
        "machine command 'open' should not be listed in local help"
    );
    assert!(
        !lists_command(&help_of(&["--help"]), "run-agent"),
        "machine command 'run-agent' should not be listed in top-level help"
    );
    assert!(
        cli(&["local", "open", "--help"]).status.success(),
        "local open --help should succeed"
    );
    assert!(
        cli(&["run-agent", "--help"]).status.success(),
        "run-agent --help should succeed"
    );
}

#[test]
fn local_agent_lists_its_subcommands() {
    let help = help_of(&["local", "agent", "--help"]);
    for command in ["add", "list", "remove"] {
        assert!(
            lists_command(&help, command),
            "agent command {command:?} is absent from local agent help:\n{help}"
        );
    }
}

#[test]
fn operands_are_positional() {
    for args in [
        &["admin", "backup", "--help"][..],
        &["admin", "restore", "--help"][..],
        &["export", "--help"][..],
    ] {
        let help = help_of(args);
        assert!(
            help.contains("Arguments:"),
            "no positional operands in:\n{help}"
        );
    }
}

#[test]
fn export_flags_are_correct() {
    let help = help_of(&["export", "--help"]);
    assert!(help.contains("--at <LABEL>"), "--at <LABEL> missing from export help:\n{help}");
    assert!(help.contains("--key <LINK>"), "--key <LINK> missing from export help:\n{help}");
    assert!(help.contains("--server"), "--server missing from export help:\n{help}");
    assert!(help.contains("--token"), "--token missing from export help:\n{help}");
    assert!(
        !help.contains("--project"),
        "--project should not be in export help:\n{help}"
    );
    assert!(
        !help.contains("--format"),
        "--format should not be in export help:\n{help}"
    );
    assert!(
        !help.contains("--since"),
        "--since should not be in export help:\n{help}"
    );
    assert!(
        !help.contains("--output"),
        "--output should not be in export help:\n{help}"
    );
}

#[test]
fn local_start_flags_are_correct() {
    let help = help_of(&["local", "start", "--help"]);
    assert!(help.contains("--foreground"), "--foreground missing from local start help:\n{help}");
    assert!(help.contains("--at-login"), "--at-login missing from local start help:\n{help}");
}

#[test]
fn deployment_flags_are_scoped() {
    let export_help = help_of(&["export", "--help"]);
    assert!(export_help.contains("--server"), "--server should be in export help");

    let admin_help = help_of(&["admin", "--help"]);
    assert!(
        !admin_help.contains("--server <"),
        "--server should not be in admin help:\n{admin_help}"
    );
    assert!(
        !admin_help.contains("--token"),
        "--token should not be in admin help:\n{admin_help}"
    );

    let local_help = help_of(&["local", "--help"]);
    assert!(
        !local_help.contains("--server <"),
        "--server should not be in local help:\n{local_help}"
    );
    assert!(
        !local_help.contains("--token"),
        "--token should not be in local help:\n{local_help}"
    );

    let admin_serve_help = help_of(&["admin", "serve", "--help"]);
    assert!(
        !admin_serve_help.contains("--server <"),
        "--server should not be in admin serve help:\n{admin_serve_help}"
    );
    assert!(
        !admin_serve_help.contains("--token"),
        "--token should not be in admin serve help:\n{admin_serve_help}"
    );
}

#[test]
fn removed_commands_fail_to_parse() {
    assert!(!cli(&["local", "launch"]).status.success(), "local launch should fail");
    assert!(!cli(&["local", "doctor"]).status.success(), "local doctor should fail");
    assert!(!cli(&["agent", "mcp", "x"]).status.success(), "agent mcp should fail");
    assert!(!cli(&["export", "paper", "--project"]).status.success(), "export with --project should fail");
}
