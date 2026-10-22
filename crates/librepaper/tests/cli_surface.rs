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
    for command in ["login", "logout", "list", "export", "admin"] {
        assert!(
            lists_command(&help, command),
            "public command {command:?} is absent from top-level help:\n{help}"
        );
    }
    for hidden in [
        "agent",
        "local",
        "mcp",
        "start",
        "stop",
        "status",
        "install-desktop",
    ] {
        assert!(
            !lists_command(&help, hidden),
            "{hidden:?} should not be listed in top-level help:\n{help}"
        );
    }
    assert!(
        !help.to_lowercase().contains("companion"),
        "help should not mention the companion:\n{help}"
    );
    assert!(
        !cli(&[]).status.success(),
        "bare librepaper should print help and not launch anything"
    );
    assert!(
        !cli(&["desktop"]).status.success(),
        "the removed desktop subcommand should be rejected"
    );
    assert!(
        !cli(&["local", "desktop"]).status.success(),
        "the removed local desktop subcommand should be rejected"
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
fn moderate_help_lists_the_moderation_commands() {
    let help = help_of(&["admin", "moderate", "--help"]);
    for command in [
        "block-account",
        "unblock-account",
        "erase-account",
        "hide-project",
        "unhide-project",
    ] {
        assert!(
            lists_command(&help, command),
            "moderation command {command:?} is absent from moderate help:\n{help}"
        );
    }
}

/// `local` is the desktop app's interface to the background service, and the
/// sidebar assistant's own chosen agent spawns `mcp`. Both parse, neither is
/// advertised in root help.
#[test]
fn machine_invoked_commands_parse_but_are_not_listed() {
    let root = help_of(&["--help"]);
    for command in ["local", "mcp"] {
        assert!(
            !lists_command(&root, command),
            "{command:?} should not be listed in top-level help"
        );
        assert!(
            cli(&[command, "--help"]).status.success(),
            "{command:?} --help should parse"
        );
    }
    for command in ["start", "stop", "status", "endpoint", "diagnostics"] {
        assert!(
            cli(&["local", command, "--help"]).status.success(),
            "local {command} --help should parse"
        );
    }
}

#[test]
fn local_start_flags_are_correct() {
    let help = help_of(&["local", "start", "--help"]);
    assert!(help.contains("--foreground"), "{help}");
    assert!(help.contains("--port"), "{help}");
    assert!(!help.contains("--at-login"), "{help}");
    assert!(!help.contains("--tool-path"), "{help}");
}

/// `librepaper mcp` takes no positional link. Without `--connection` it needs
/// `LIBREPAPER_MCP_CONNECTION` and otherwise fails with a clear message.
#[test]
fn mcp_requires_a_connection_and_rejects_a_bare_link() {
    assert!(
        !cli(&["mcp", "https://example.test/docs/a#k=x"])
            .status
            .success(),
        "mcp with a positional link should fail to parse"
    );
    let output = Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .args(["mcp"])
        .env_remove("LIBREPAPER_MCP_CONNECTION")
        .output()
        .expect("CLI starts");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("LIBREPAPER_MCP_CONNECTION"),
        "{output:?}"
    );
}

/// `librepaper mcp --document URL` takes its token from the environment only.
/// It cannot be combined with `--connection`, and without the variable it
/// fails and names it.
#[test]
fn mcp_document_takes_its_token_from_the_environment() {
    assert!(
        !cli(&[
            "mcp",
            "--document",
            "https://example.test/docs/a",
            "--connection",
            "x"
        ])
        .status
        .success(),
        "mcp --document with --connection should fail to parse"
    );
    let output = Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .args(["mcp", "--document", "https://example.test/docs/a"])
        .env_remove("LIBREPAPER_AGENT_TOKEN")
        .output()
        .expect("CLI starts");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("LIBREPAPER_AGENT_TOKEN"),
        "{output:?}"
    );
}

#[test]
fn agents_are_managed_in_the_desktop_app_not_the_cli() {
    assert!(
        !cli(&["agent", "list"]).status.success(),
        "agent should fail"
    );
    assert!(
        !cli(&["local", "agent", "list"]).status.success(),
        "local agent should fail"
    );
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
fn inherited_local_port_env_does_not_reject_other_subcommands() {
    let state_home = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .args(["logout"])
        .env("HOME", state_home.path())
        .env("XDG_CONFIG_HOME", state_home.path())
        .env("XDG_DATA_HOME", state_home.path())
        .env_remove("LIBREPAPER_SERVER")
        .env_remove("LIBREPAPER_TOKEN")
        .env("LIBREPAPER_LOCAL_PORT", "9123")
        .env("XDG_STATE_HOME", state_home.path())
        .output()
        .expect("CLI starts");
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn removed_root_launch_controls_fail_to_parse() {
    for args in [
        &["start"][..],
        &["stop"][..],
        &["status"][..],
        &["install-desktop"][..],
        &["--foreground"][..],
        &["--at-login"][..],
        &["--port", "9123", "agent", "list"][..],
        &["--foreground", "list"][..],
        &["local", "approve", "123456"][..],
        &[
            "local",
            "open",
            "librepaper://open?url=https://app.librepaper.org/",
        ][..],
        &["local", "tray"][..],
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
    assert!(admin_serve_help.contains("--config"));
    assert!(!admin_serve_help.contains("--port"));
    assert!(!admin_serve_help.contains("--bind"));
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
fn server_flags_are_rejected_and_server_environment_overrides_are_ignored() {
    for args in [
        &["admin", "serve", "--port", "9123"][..],
        &["admin", "serve", "--origin", "https://paper.example"][..],
        &[
            "admin",
            "backup",
            "--database-url",
            "postgresql:///other",
            "backup-dir",
        ][..],
        &[
            "admin",
            "moderate",
            "hide-project",
            "paper",
            "--database-url",
            "postgresql:///other",
            "--actor",
            "operator",
            "--reason",
            "review",
        ][..],
        &[
            "admin",
            "moderate",
            "erase-account",
            "paper",
            "--database-url",
            "postgresql:///other",
            "--actor",
            "operator",
            "--reason",
            "review",
        ][..],
    ] {
        assert!(!cli(args).status.success(), "{args:?} should be rejected");
    }

    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    std::fs::write(&config, "[server]\naddress = \"0.0.0.0:8179\"\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_librepaper"))
        .args([
            "admin",
            "config",
            "show",
            "--config",
            config.to_str().unwrap(),
        ])
        .env("LIBREPAPER_PORT", "9999")
        .env("LIBREPAPER_DATABASE_URL", "postgresql:///ambient")
        .env("LIBREPAPER_CONFIG", "/tmp/ignored-config.toml")
        .output()
        .expect("CLI starts");
    assert!(output.status.success(), "{output:?}");
    let shown = String::from_utf8_lossy(&output.stdout);
    assert!(shown.contains("address = \"0.0.0.0:8179\""), "{shown}");
    assert!(!shown.contains("9999"), "{shown}");
    assert!(!shown.contains("ambient"), "{shown}");
}

#[test]
fn config_commands_accept_explicit_paths() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("missing.toml");
    let config = config.to_str().unwrap();
    for args in [
        &["admin", "serve", "--config", config][..],
        &["admin", "config", "check", "--config", config][..],
        &["admin", "config", "show", "--config", config][..],
        &["admin", "backup", "backup-dir", "--config", config][..],
        &[
            "admin",
            "restore",
            "backup-dir",
            "target-dir",
            "--config",
            config,
        ][..],
        &[
            "admin",
            "moderate",
            "hide-project",
            "paper",
            "--actor",
            "operator",
            "--reason",
            "review",
            "--config",
            config,
        ][..],
        &[
            "admin",
            "moderate",
            "erase-account",
            "paper",
            "--actor",
            "operator",
            "--reason",
            "review",
            "--config",
            config,
        ][..],
    ] {
        let output = cli(args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?}: {output:?}");
        assert!(
            stderr.contains("could not read server configuration"),
            "{args:?}: {output:?}"
        );
        assert!(stderr.contains(config), "{args:?}: {output:?}");
    }
}

#[test]
fn config_check_is_side_effect_free_and_validates_serve_inputs() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = occupied.local_addr().unwrap().port();
    std::fs::write(
        &config,
        format!("[server]\naddress = \"127.0.0.1:{port}\"\n"),
    )
    .unwrap();
    let config_arg = config.to_str().unwrap();

    let valid = cli(&["admin", "config", "check", "--config", config_arg]);
    assert!(valid.status.success(), "{valid:?}");
    assert!(String::from_utf8_lossy(&valid.stdout).contains("configuration is valid"));
    assert!(!temp.path().join("librepaper-data").exists());

    std::fs::write(&config, "[origins]\napp = \"javascript:alert(1)\"\n").unwrap();
    let invalid = cli(&["admin", "config", "check", "--config", config_arg]);
    assert!(!invalid.status.success(), "{invalid:?}");
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("origins.app"));
    assert!(!temp.path().join("librepaper-data").exists());
}

#[test]
fn config_check_never_echoes_database_url_secrets() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let config_arg = config.to_str().unwrap();

    std::fs::write(
        &config,
        "[storage]\ndatabase_url = \"postgres://user:QUERY_SECRET@localhost/paper?future_option=QUERY_SECRET\"\n",
    )
    .unwrap();
    let accepted = cli(&["admin", "config", "check", "--config", config_arg]);
    let accepted_output = format!(
        "{}{}",
        String::from_utf8_lossy(&accepted.stdout),
        String::from_utf8_lossy(&accepted.stderr)
    );
    assert!(accepted.status.success(), "{accepted_output}");
    assert!(
        !accepted_output.contains("QUERY_SECRET"),
        "{accepted_output}"
    );

    std::fs::write(
        &config,
        "[storage]\ndatabase_url = \"postgres://user:PASSWORD_SECRET@localhost:invalid/paper\"\n",
    )
    .unwrap();
    let rejected = cli(&["admin", "config", "check", "--config", config_arg]);
    let rejected_output = format!(
        "{}{}",
        String::from_utf8_lossy(&rejected.stdout),
        String::from_utf8_lossy(&rejected.stderr)
    );
    assert!(!rejected.status.success(), "{rejected_output}");
    assert!(
        !rejected_output.contains("PASSWORD_SECRET"),
        "{rejected_output}"
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
