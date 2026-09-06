//! Regression tests for the findings of REVIEW-codex-crates.md (cli group).
#![allow(unused_imports)]
use super::*;

/* --------------------------------------------------------------- R05 */

/// R05: a relative publish root with a root-anchored `.gitignore` pattern
/// used to be evaluated against the wrong working directory, so
/// `/private.txt` never matched and the file was published. `git_ignores`
/// now hands `check-ignore` a path relative to the root it ran `-C` into,
/// which is what makes an anchored pattern mean what the author wrote.
#[test]
fn a_relative_root_honours_a_root_anchored_gitignore_pattern() {
    let dir = tempfile::Builder::new()
        .prefix("komodoc-gitignore-")
        .tempdir_in(".")
        .expect("a scratch directory next to the crate");
    let git = std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(dir.path())
        .status();
    if !matches!(git, Ok(status) if status.success()) {
        eprintln!("skipping: no git on this machine");
        return;
    }
    std::fs::write(dir.path().join(".gitignore"), "/private.txt\n").unwrap();
    std::fs::write(dir.path().join("private.txt"), "DO NOT PUBLISH").unwrap();
    std::fs::write(dir.path().join("main.typ"), "= A paper\n").unwrap();

    let relative = dir
        .path()
        .strip_prefix(std::env::current_dir().unwrap())
        .unwrap_or(dir.path());
    let found = crate::cli::files_under(relative, "", &crate::cli::git_ignores(relative));
    assert!(
        !found.contains(&"private.txt".to_string()),
        "an anchored .gitignore pattern was not honoured through a relative root: {found:?}"
    );
    assert!(found.contains(&"main.typ".to_string()));
}

/* --------------------------------------------------------------- R06 */

/// R06: a symlink cycle used to be walked forever, because directories were
/// never remembered by their resolved location. Two links back into each
/// other's chain must terminate the walk rather than recurse without bound.
#[test]
fn a_symlink_cycle_terminates() {
    let outer = tempfile::tempdir().unwrap();
    let inner = outer.path().join("inner");
    std::fs::create_dir(&inner).unwrap();
    std::fs::write(inner.join("leaf.typ"), "leaf").unwrap();
    // inner/loop -> outer, outer/into -> inner: a cycle two hops long.
    std::os::unix::fs::symlink(outer.path(), inner.join("loop")).unwrap();
    std::os::unix::fs::symlink(&inner, outer.path().join("into")).unwrap();

    let found = crate::cli::files_under(outer.path(), "", &|_| false);
    // Termination is the point of the test; a hang would fail it by timeout
    // rather than by assertion. The leaf is still reached exactly once,
    // through whichever path the walk finds first.
    let leaves: Vec<&String> = found.iter().filter(|p| p.ends_with("leaf.typ")).collect();
    assert_eq!(leaves.len(), 1, "{found:?}");
}

/// R06: content a symlink resolves to outside the publish root must not be
/// published, while a symlink that stays inside the root still works -- the
/// review's decision was to keep following symlinks, just never outside the
/// root and never in a cycle.
#[test]
fn a_symlink_outside_the_root_is_excluded_but_an_inside_one_still_works() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("private.txt"), "PRIVATE OUTSIDE ROOT").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("linked")).unwrap();

    // An in-root symlink whose target is otherwise unreachable by the walk
    // (it lives under a dot-directory, which is skipped by name): the only
    // way to it is the symlink, so this exercises "is it followed" without
    // the also-correct dedup of two names for the one directory getting in
    // the way (several links to the same place must publish it once, not
    // once per link -- that is a different assertion, not this one).
    let real = root.path().join(".vault");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("kept.typ"), "kept").unwrap();
    std::os::unix::fs::symlink(&real, root.path().join("alias")).unwrap();

    let found = crate::cli::files_under(root.path(), "", &|_| false);
    assert!(
        !found.contains(&"linked/private.txt".to_string()),
        "a symlink resolving outside the root was published: {found:?}"
    );
    assert!(
        found.contains(&"alias/kept.typ".to_string()),
        "an in-root symlink to a real directory was not followed: {found:?}"
    );
}

/// R06: several symlinks to the same directory must publish its content once,
/// not once per link -- otherwise a handful of links to a shared assets
/// folder would multiply the upload rather than just reaching it.
#[test]
fn several_symlinks_to_the_same_directory_publish_it_once() {
    let root = tempfile::tempdir().unwrap();
    let real = root.path().join(".vault");
    std::fs::create_dir(&real).unwrap();
    std::fs::write(real.join("kept.typ"), "kept").unwrap();
    std::os::unix::fs::symlink(&real, root.path().join("a")).unwrap();
    std::os::unix::fs::symlink(&real, root.path().join("b")).unwrap();

    let found = crate::cli::files_under(root.path(), "", &|_| false);
    let hits: Vec<&String> = found.iter().filter(|p| p.ends_with("kept.typ")).collect();
    assert_eq!(
        hits.len(),
        1,
        "the same directory was published once per link instead of once: {found:?}"
    );
}

/* --------------------------------------------------------------- R03 */

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

/* --------------------------------------------------------------- credential-at-rest */

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
