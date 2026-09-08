//! What `komodoc publish` sends from a directory: what .gitignore excludes,
//! and what symlinks do.

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
