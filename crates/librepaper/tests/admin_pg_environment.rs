//! Deployment administration must not inherit libpq/SQLx connection settings
//! from the process environment. Only TOML leaves that explicitly reference a
//! PG* variable may read its original startup value.

use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Command,
    thread,
    time::{Duration, Instant},
};

const CLI: &str = env!("CARGO_BIN_EXE_librepaper");

struct ChildGuard(Option<std::process::Child>);

impl ChildGuard {
    fn child_mut(&mut self) -> &mut std::process::Child {
        self.0.as_mut().unwrap()
    }

    fn wait_with_output(mut self) -> std::process::Output {
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn write_config(directory: &std::path::Path, database_url: &str) -> std::path::PathBuf {
    let path = directory.join("config.toml");
    fs::write(
        &path,
        format!(
            "[server]\naddress = \"127.0.0.1:43123\"\n[origins]\napp = \"https://paper.example\"\n\
             [storage]\ndatabase_url = {database_url}\n\
             [access]\npublishers = [\"alice\"]\ncommenters = [\"anyone\"]\n"
        ),
    )
    .unwrap();
    path
}

fn show(path: &std::path::Path, pgdatabase: &str) -> std::process::Output {
    Command::new(CLI)
        .args([
            "admin",
            "config",
            "show",
            "--config",
            path.to_str().unwrap(),
        ])
        .env("PGHOST", "poison.pg.invalid")
        .env("PGPORT", "not-a-port")
        .env("PGUSER", "poison-user")
        .env("PGDATABASE", pgdatabase)
        .env("PGPASSWORD", "poison-password")
        .env("PGOPTIONS", "-c application_name=poison-option")
        .env("PGSSLMODE", "not-a-libpq-mode")
        .output()
        .expect("CLI starts")
}

#[test]
fn admin_config_ignores_implicit_libpq_environment_but_honors_explicit_refs() {
    let directory = tempfile::tempdir().unwrap();
    let literal = write_config(
        directory.path(),
        "\"postgresql://paper:config-secret@127.0.0.1:5432/paper?sslmode=disable\"",
    );
    let output = show(&literal, "poison-database");
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("address = \"127.0.0.1:43123\""), "{stdout}");
    assert!(stdout.contains("database_url = \"<redacted>\""), "{stdout}");
    for secret in [
        "config-secret",
        "poison.pg.invalid",
        "not-a-port",
        "poison-user",
        "poison-database",
        "poison-password",
        "poison-option",
        "not-a-libpq-mode",
    ] {
        assert!(
            !stdout.contains(secret),
            "configuration output leaked {secret}"
        );
        assert!(!stderr.contains(secret), "diagnostics leaked {secret}");
    }

    let referenced = write_config(directory.path(), "{ env = \"PGDATABASE\" }");
    let output = show(
        &referenced,
        "postgresql://explicit:reference@127.0.0.1:5432/reference?sslmode=disable",
    );
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("# source: env:PGDATABASE"), "{stdout}");
    assert!(stdout.contains("database_url = \"<redacted>\""), "{stdout}");
}

#[test]
fn explicit_reference_does_not_fall_through_to_scrubbed_empty_password() {
    let directory = tempfile::tempdir().unwrap();
    let path = write_config(directory.path(), "{ env = \"PGPASSWORD\" }");
    let output = Command::new(CLI)
        .args([
            "admin",
            "config",
            "show",
            "--config",
            path.to_str().unwrap(),
        ])
        .env_remove("PGPASSWORD")
        .output()
        .expect("CLI starts");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("referenced environment variable \"PGPASSWORD\" is unavailable"));
    assert!(!stderr.contains("<redacted>"));
}

#[test]
fn admin_postgres_connection_uses_only_the_configured_uri() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let directory = tempfile::tempdir().unwrap();
    let database_url = format!("\"postgresql://127.0.0.1:{port}\"");
    let path = write_config(directory.path(), &database_url);
    let mut child = ChildGuard(Some(
        Command::new(CLI)
            .args([
                "admin",
                "moderate",
                "block-account",
                "00000000-0000-0000-0000-000000000000",
                "--actor",
                "test",
                "--reason",
                "test",
                "--config",
                path.to_str().unwrap(),
            ])
            .env("PGHOST", "127.0.0.2")
            .env("PGPORT", "1")
            .env("PGUSER", "poison-user")
            .env("PGDATABASE", "poison-database")
            .env("PGPASSWORD", "poison-password")
            .env("PGOPTIONS", "-c application_name=poison-option")
            .env("PGSSLMODE", "require")
            .spawn()
            .expect("CLI starts"),
    ));

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if child.child_mut().try_wait().unwrap().is_some() || Instant::now() >= deadline {
                    let _ = child.child_mut().kill();
                    let output = child.wait_with_output();
                    panic!("CLI did not connect to its configured database: {output:?}");
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("database test listener failed: {error}"),
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();

    // SQLx's default prefers TLS. The server declines the SSL probe; an
    // inherited PGSSLMODE=require would make the client stop here.
    let mut ssl_request = [0_u8; 8];
    stream.read_exact(&mut ssl_request).unwrap();
    assert_eq!(&ssl_request[..4], &[0, 0, 0, 8]);
    assert_eq!(&ssl_request[4..], &[4, 210, 22, 47]);
    stream.write_all(b"N").unwrap();

    let mut header = [0_u8; 4];
    stream.read_exact(&mut header).unwrap();
    let length = u32::from_be_bytes(header) as usize;
    let mut startup = vec![0; length - header.len()];
    stream.read_exact(&mut startup).unwrap();
    assert_eq!(&startup[..4], &[0, 3, 0, 0], "unexpected startup packet");
    let parameters = String::from_utf8_lossy(&startup[4..]);
    assert!(
        !parameters.contains("poison-user") && !parameters.contains("poison-database"),
        "PGUSER/PGDATABASE changed configured parameters: {parameters:?}"
    );
    assert!(
        !parameters.contains("poison-option"),
        "PGOPTIONS changed startup parameters: {parameters:?}"
    );

    // Request cleartext auth so the test can verify that PGPASSWORD is the
    // empty startup override, not the inherited poison value.
    stream.write_all(&[b'R', 0, 0, 0, 8, 0, 0, 0, 3]).unwrap();
    let mut password_header = [0_u8; 5];
    stream.read_exact(&mut password_header).unwrap();
    assert_eq!(password_header[0], b'p');
    let password_length = u32::from_be_bytes(password_header[1..5].try_into().unwrap()) as usize;
    let mut password = vec![0; password_length - 4];
    stream.read_exact(&mut password).unwrap();
    assert_eq!(password.strip_suffix(&[0]).unwrap(), b"");

    drop(stream);
    let output = child.wait_with_output();
    assert!(
        !output.status.success(),
        "fake database unexpectedly authenticated"
    );
}

#[test]
#[ignore = "requires LIBREPAPER_TEST_POSTGRES_URL; destructive to its dedicated database"]
fn admin_serve_refuses_to_replace_missing_key_for_existing_deployment() {
    let database_url = std::env::var("LIBREPAPER_TEST_POSTGRES_URL")
        .expect("set LIBREPAPER_TEST_POSTGRES_URL to a disposable PostgreSQL database");
    let directory = tempfile::tempdir().unwrap();
    let database_secret = directory.path().join("database-url");
    let deployment = directory.path().join("deployment");
    let config = directory.path().join("config.toml");
    fs::write(&database_secret, format!("{database_url}\n")).unwrap();
    fs::write(
        &config,
        format!(
            "[server]\naddress = \"127.0.0.1:0\"\nmigrate = false\n\
             [storage]\ndatabase_url = {{ file = \"database-url\" }}\n\
             directory = {:?}\n",
            deployment
        ),
    )
    .unwrap();

    let migration = Command::new(CLI)
        .args(["admin", "migrate", "--config", config.to_str().unwrap()])
        .output()
        .expect("migration CLI starts");
    assert!(migration.status.success(), "{migration:?}");

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let pool = runtime
        .block_on(sqlx::postgres::PgPoolOptions::new().connect(&database_url))
        .expect("connect to the disposable PostgreSQL database");
    runtime.block_on(async {
        sqlx::query("TRUNCATE accounts CASCADE")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM server_runtime_state")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO server_runtime_state(name, value) VALUES($1, $2)")
            .bind(librepaper_engine::log::DEPLOYMENT_PEER_STATE)
            .bind(serde_json::json!({ "key": "persisted-peer" }))
            .execute(&pool)
            .await
            .unwrap();
    });
    runtime.block_on(pool.close());

    let mut child = ChildGuard(Some(
        Command::new(CLI)
            .args(["admin", "serve", "--config", config.to_str().unwrap()])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("server CLI starts"),
    ));
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut timed_out = false;
    loop {
        if child.child_mut().try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = child.child_mut().kill();
            break;
        }
        thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output();

    let pool = runtime
        .block_on(sqlx::postgres::PgPoolOptions::new().connect(&database_url))
        .expect("reconnect to the disposable PostgreSQL database");
    runtime.block_on(async {
        sqlx::query("DELETE FROM server_runtime_state WHERE name=$1")
            .bind(librepaper_engine::log::DEPLOYMENT_PEER_STATE)
            .execute(&pool)
            .await
            .unwrap();
    });
    runtime.block_on(pool.close());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !timed_out,
        "server kept running after missing-key startup: {output:?}"
    );
    assert!(
        !output.status.success(),
        "server accepted a missing key: {output:?}"
    );
    assert!(
        stderr.contains("restore the matching secrets/session.key"),
        "{stderr}"
    );
    assert!(
        !deployment.join("secrets/session.key").exists(),
        "startup must not replace a missing key for an existing deployment"
    );
}
