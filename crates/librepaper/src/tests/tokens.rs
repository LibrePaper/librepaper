//! Where a sign-in is kept: one token per origin, and the explicit token
//! (flag or environment) that overrides it.

/// R03: two deployments must never share a cached token. Signing in to one
/// origin and asking for the other's must come back empty, not with the
/// first one's bearer.
#[test]
fn two_origins_get_two_different_tokens() {
    let base = tempfile::tempdir().unwrap();
    let base = base.path();
    crate::cli::store_token_at(base, "https://a.example", "token-a").unwrap();
    crate::cli::store_token_at(base, "https://b.example", "token-b").unwrap();

    assert_eq!(
        crate::cli::stored_token_at(base, "https://a.example"),
        "token-a"
    );
    assert_eq!(
        crate::cli::stored_token_at(base, "https://b.example"),
        "token-b"
    );
    // Neither leaks to a server nothing was ever cached for.
    assert_eq!(crate::cli::stored_token_at(base, "https://c.example"), "");
}

/// The same server named two different ways -- with and without an explicit
/// default port, with and without a trailing slash -- must resolve to the
/// same cached token.
#[test]
fn the_same_origin_written_two_ways_shares_one_token() {
    let base = tempfile::tempdir().unwrap();
    let base = base.path();
    crate::cli::store_token_at(base, "https://x.example", "the-token").unwrap();
    assert_eq!(
        crate::cli::stored_token_at(base, "https://x.example:443"),
        "the-token"
    );
    assert_eq!(
        crate::cli::stored_token_at(base, "https://x.example/"),
        "the-token"
    );
}

/// R03: an explicit token (from `--token` or `$LIBREPAPER_TOKEN`, already
/// merged by clap) wins over whatever is cached, for whichever server was
/// selected -- it is a bearer supplied on purpose, not a credential this
/// scoping is meant to protect against misdirection. Uses `stored_token_with`'s
/// explicit `token` parameter rather than the real process environment, so
/// this does not race other tests over a shared variable.
#[test]
fn an_explicit_token_wins_over_the_scoped_cache() {
    let base = tempfile::tempdir().unwrap();
    let base = base.path();
    crate::cli::store_token_at(base, "https://a.example", "cached-token").unwrap();

    assert_eq!(
        crate::cli::stored_token_with(base, "https://a.example", Some("explicit-token")),
        "explicit-token"
    );
    // No explicit token: falls back to the scoped cache as before.
    assert_eq!(
        crate::cli::stored_token_with(base, "https://a.example", None),
        "cached-token"
    );
    // A blank explicit value is not a real override.
    assert_eq!(
        crate::cli::stored_token_with(base, "https://a.example", Some("   ")),
        "cached-token"
    );
}

/// The token cache must never be readable by anyone but its owner, from the
/// moment the file is created.
#[cfg(unix)]
#[test]
fn the_scoped_token_file_is_created_private() {
    use std::os::unix::fs::PermissionsExt;
    let base = tempfile::tempdir().unwrap();
    let base = base.path();
    crate::cli::store_token_at(base, "https://a.example", "t").unwrap();
    let mode = std::fs::metadata(crate::cli::tokens_path(base))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "the token cache is readable by others");
}
