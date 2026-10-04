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

#[test]
fn version_reports_the_release_version() {
    let output = cli(&["--version"]);
    assert!(output.status.success(), "{output:?}");
    let expected = format!(
        "librepaper {}",
        option_env!("LIBREPAPER_VERSION").unwrap_or(concat!("v", env!("CARGO_PKG_VERSION")))
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
}

fn lists_command(help: &str, command: &str) -> bool {
    help.lines()
        .any(|line| line.split_whitespace().next() == Some(command))
}

#[test]
fn top_level_help_lists_exactly_the_public_commands() {
    let help = help_of(&["--help"]);
    for command in [
        "start",
        "stop",
        "status",
        "desktop",
        "install-desktop",
        "agent",
        "login",
        "logout",
        "list",
        "export",
        "local",
        "admin",
    ] {
        assert!(
            lists_command(&help, command),
            "public command {command:?} is absent from top-level help:\n{help}"
        );
    }
    assert!(
        !lists_command(&help, "mcp"),
        "mcp should not be listed in top-level help:\n{help}"
    );
}

#[test]
fn admin_help_lists_the_admin_commands() {
    let help = help_of(&["admin", "--help"]);
    for command in ["serve", "backup", "restore"] {
        assert!(
            lists_command(&help, command),
            "admin command {command:?} is absent from admin help:\n{help}"
        );
    }
    assert!(
        !lists_command(&help, "sweep"),
        "sweep should not be listed in admin help:\n{help}"
    );
}

#[test]
fn local_help_lists_only_approval_and_pairing_controls() {
    let help = help_of(&["local", "--help"]);
    for command in ["approve", "disconnect"] {
        assert!(
            lists_command(&help, command),
            "local command {command:?} is absent from local help:\n{help}"
        );
    }
    for hidden_command in ["start", "stop", "status", "agent", "open", "tray"] {
        assert!(
            !lists_command(&help, hidden_command),
            "compatibility command {hidden_command:?} should be hidden from local help:\n{help}"
        );
    }
}

/// `local open` answers `librepaper://` links for the operating system, and
/// the sidebar assistant's own chosen agent spawns `mcp`. Both parse, neither
/// is advertised.
#[test]
fn machine_invoked_commands_parse_but_are_not_listed() {
    for (parent, command) in [("local", "open"), ("", "mcp")] {
        let mut path: Vec<&str> = [parent].into_iter().filter(|p| !p.is_empty()).collect();
        let mut help_args = path.clone();
        help_args.push("--help");
        assert!(
            !lists_command(&help_of(&help_args), command),
            "{command:?} should not be listed under {parent:?}"
        );
        path.extend([command, "--help"]);
        assert!(cli(&path).status.success(), "{path:?} should parse");
    }
}

/// `librepaper mcp` accepts only `--connection NAME`; a bare positional link
/// is no longer a way to invoke it, since the companion (the only thing that
/// ever passes credentials to a session) always names a connection.
#[test]
fn mcp_requires_a_connection_and_rejects_a_bare_link() {
    assert!(
        !cli(&["mcp", "https://example.test/docs/a#k=x"])
            .status
            .success(),
        "mcp with a positional link should fail to parse"
    );
    assert!(
        !cli(&["mcp"]).status.success(),
        "mcp with no --connection should fail to parse"
    );
}

#[test]
fn root_and_legacy_agent_paths_list_their_subcommands() {
    let help = help_of(&["agent", "--help"]);
    let legacy_help = help_of(&["local", "agent", "--help"]);
    for command in ["add", "list", "remove"] {
        assert!(
            lists_command(&help, command),
            "agent command {command:?} is absent from root agent help:\n{help}"
        );
        assert!(
            lists_command(&legacy_help, command),
            "agent command {command:?} is absent from legacy local agent help:\n{legacy_help}"
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
    assert!(
        help.contains("--at <LABEL>"),
        "--at <LABEL> missing from export help:\n{help}"
    );
    assert!(
        help.contains("--key <LINK>"),
        "--key <LINK> missing from export help:\n{help}"
    );
    assert!(
        help.contains("--server"),
        "--server missing from export help:\n{help}"
    );
    assert!(
        help.contains("--token"),
        "--token missing from export help:\n{help}"
    );
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
fn root_and_legacy_start_flags_are_correct() {
    let help = help_of(&["start", "--help"]);
    let legacy_help = help_of(&["local", "start", "--help"]);
    assert!(
        help.contains("--foreground"),
        "--foreground missing from root start help:\n{help}"
    );
    assert!(
        help.contains("--at-login"),
        "--at-login missing from root start help:\n{help}"
    );
    assert!(help.contains("--port"));
    assert!(help.contains("--tool-path"));
    assert!(legacy_help.contains("--foreground"));
    assert!(legacy_help.contains("--at-login"));
    assert!(legacy_help.contains("--port"));
    assert!(legacy_help.contains("--tool-path"));
}

#[test]
fn inherited_companion_env_does_not_reject_other_subcommands() {
    let state_home = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .args(["agent", "list"])
        .env_remove("LIBREPAPER_SERVER")
        .env_remove("LIBREPAPER_TOKEN")
        .env("LIBREPAPER_LOCAL_PORT", "9123")
        .env("LIBREPAPER_TOOL_PATH", "/tmp")
        .env("XDG_STATE_HOME", state_home.path())
        .output()
        .expect("CLI starts");
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn root_launch_flags_cannot_be_combined_with_other_commands() {
    for args in [
        &["--port", "9123", "agent", "list"][..],
        &["--foreground", "list"][..],
        &["--at-login", "admin", "serve"][..],
    ] {
        assert!(!cli(args).status.success(), "{args:?} should be rejected");
    }
}

#[test]
fn deployment_flags_are_scoped() {
    let export_help = help_of(&["export", "--help"]);
    assert!(
        export_help.contains("--server"),
        "--server should be in export help"
    );

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
    assert!(
        !cli(&["local", "launch"]).status.success(),
        "local launch should fail"
    );
    assert!(
        !cli(&["local", "doctor"]).status.success(),
        "local doctor should fail"
    );
    assert!(
        !cli(&["agent", "mcp", "x"]).status.success(),
        "agent mcp should fail"
    );
    assert!(
        !cli(&["export", "paper", "--project"]).status.success(),
        "export with --project should fail"
    );
    assert!(
        !cli(&["admin", "seed"]).status.success(),
        "admin seed should fail"
    );
}

/// `admin sweep` and `admin seed` (when it existed) are hidden from help but
/// still parse for backwards compatibility or operational needs.
#[test]
fn hidden_admin_commands_parse_but_are_not_listed() {
    let admin_help = help_of(&["admin", "--help"]);
    assert!(
        !lists_command(&admin_help, "sweep"),
        "sweep should not be listed in admin help"
    );
    assert!(
        cli(&["admin", "sweep", "--help"]).status.success(),
        "admin sweep --help should parse"
    );
}
