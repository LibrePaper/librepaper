//! Where a sign-in is kept: one token per origin, the legacy file, and the
//! environment that overrides both.

/// R03: two deployments must never share a cached token. Signing in to one
/// origin and asking for the other's must come back empty, not with the
/// first one's bearer.
#[test]
fn two_origins_get_two_different_tokens() {
    let base = tempfile::tempdir().unwrap();
    let base = base.path();
    let default = "https://default.example";
    crate::cli::store_token_at(base, "https://a.example", default, "token-a").unwrap();
    crate::cli::store_token_at(base, "https://b.example", default, "token-b").unwrap();

    assert_eq!(
        crate::cli::stored_token_at(base, "https://a.example", default),
        "token-a"
    );
    assert_eq!(
        crate::cli::stored_token_at(base, "https://b.example", default),
        "token-b"
    );
    // Neither leaks to a server nothing was ever cached for.
    assert_eq!(
        crate::cli::stored_token_at(base, "https://c.example", default),
        ""
    );
}

/// The same server named two different ways -- with and without an explicit
/// default port, with and without a trailing slash -- must resolve to the
/// same cached token.
#[test]
fn the_same_origin_written_two_ways_shares_one_token() {
    let base = tempfile::tempdir().unwrap();
    let base = base.path();
    let default = "https://default.example";
    crate::cli::store_token_at(base, "https://x.example", default, "the-token").unwrap();
    assert_eq!(
        crate::cli::stored_token_at(base, "https://x.example:443", default),
        "the-token"
    );
    assert_eq!(
        crate::cli::stored_token_at(base, "https://x.example/", default),
        "the-token"
    );
}

/// R03 migration: the legacy unscoped file `login` used to write is honoured
/// only when the server asked about is the default server it predates --
/// never forwarded to a different deployment, which was the bug.
#[test]
fn the_legacy_token_file_is_honoured_only_for_the_default_server() {
    let base = tempfile::tempdir().unwrap();
    let base = base.path();
    let legacy = crate::cli::legacy_token_path(base);
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, "legacy-token\n").unwrap();

    let default = "https://default.example";
    assert_eq!(
        crate::cli::stored_token_at(base, default, default),
        "legacy-token",
        "the legacy file must still work for the server it was written for"
    );
    assert_eq!(
        crate::cli::stored_token_at(base, "https://someone-elses-deployment.example", default),
        "",
        "the legacy file must never be sent to a server it was not signed into"
    );
}

/// R03 migration: signing in to the default server rewrites the legacy file
/// into the scoped cache and removes it, so a later scoped read reflects the
/// new token and the legacy file is not left around to be misread again.
#[test]
fn signing_in_to_the_default_server_migrates_and_removes_the_legacy_file() {
    let base = tempfile::tempdir().unwrap();
    let base = base.path();
    let legacy = crate::cli::legacy_token_path(base);
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, "old-token\n").unwrap();

    let default = "https://default.example";
    crate::cli::store_token_at(base, default, default, "new-token").unwrap();

    assert!(
        !legacy.exists(),
        "the legacy file should be migrated away once its server has a scoped entry"
    );
    assert_eq!(
        crate::cli::stored_token_at(base, default, default),
        "new-token"
    );
}

/// R03: signing in to a non-default server must not touch the legacy file at
/// all -- it might still be the only record of the default server's token.
#[test]
fn signing_in_to_a_non_default_server_leaves_the_legacy_file_alone() {
    let base = tempfile::tempdir().unwrap();
    let base = base.path();
    let legacy = crate::cli::legacy_token_path(base);
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, "default-token\n").unwrap();

    let default = "https://default.example";
    crate::cli::store_token_at(base, "https://other.example", default, "other-token").unwrap();

    assert!(legacy.exists());
    assert_eq!(
        crate::cli::stored_token_at(base, default, default),
        "default-token"
    );
    assert_eq!(
        crate::cli::stored_token_at(base, "https://other.example", default),
        "other-token"
    );
}

/// R03: an explicit `KOMODOC_TOKEN` wins over whatever is cached, for
/// whichever server was selected -- it is a bearer supplied on purpose, not
/// a credential this scoping is meant to protect against misdirection. Uses
/// `stored_token_with`'s explicit `env_token` parameter rather than the real
/// process environment, so this does not race other tests over a shared
/// variable.
#[test]
fn komodoc_token_env_wins_over_the_scoped_cache() {
    let base = tempfile::tempdir().unwrap();
    let base = base.path();
    let default = "https://default.example";
    crate::cli::store_token_at(base, "https://a.example", default, "cached-token").unwrap();

    assert_eq!(
        crate::cli::stored_token_with(base, "https://a.example", default, Some("env-token")),
        "env-token"
    );
    // No env override: falls back to the scoped cache as before.
    assert_eq!(
        crate::cli::stored_token_with(base, "https://a.example", default, None),
        "cached-token"
    );
    // A blank env value is not a real override.
    assert_eq!(
        crate::cli::stored_token_with(base, "https://a.example", default, Some("   ")),
        "cached-token"
    );
}

/// The token cache (and the legacy file it can migrate) must never be
/// readable by anyone but its owner, from the moment the file is created.
#[cfg(unix)]
#[test]
fn the_scoped_token_file_is_created_private() {
    use std::os::unix::fs::PermissionsExt;
    let base = tempfile::tempdir().unwrap();
    let base = base.path();
    crate::cli::store_token_at(base, "https://a.example", "https://default.example", "t").unwrap();
    let mode = std::fs::metadata(crate::cli::tokens_path(base))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "the token cache is readable by others");
}
