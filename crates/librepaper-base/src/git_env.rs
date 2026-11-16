//! The environment every `git` that librepaper starts shares, so the sync
//! crate and the desktop companion clear and set the same variables.

use std::process::Command;

/// Variables that would send Git to another repository than the one a command
/// names, or into another object store. Every runner clears these: a process
/// started from inside a Git hook inherits them.
pub const PLACEMENT: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_QUARANTINE_PATH",
];

/// Variables that inject configuration or an identity the person did not set
/// in the repository. The desktop app clears these, since its copy of a
/// repository is out of sight. The command line keeps them, because a person
/// who runs `librepaper pull` in their own repository may have set their
/// identity this way.
pub const IDENTITY: &[&str] = &[
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
];

/// Variables every runner sets: C-locale messages the callers can match, no
/// prompt on a terminal, and no editor for an automatic merge message.
pub const FIXED: &[(&str, &str)] = &[
    ("LC_ALL", "C"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_MERGE_AUTOEDIT", "no"),
];

/// The variables to clear: the placement ones, and the identity ones unless
/// `keep_identity`.
pub fn ambient(keep_identity: bool) -> impl Iterator<Item = &'static str> {
    let identity: &'static [&'static str] = if keep_identity { &[] } else { IDENTITY };
    PLACEMENT.iter().chain(identity.iter()).copied()
}

/// Clears the ambient variables from `command` and sets the fixed ones. Set
/// variables the caller needs after this call.
pub fn scrub(command: &mut Command, keep_identity: bool) {
    for name in ambient(keep_identity) {
        command.env_remove(name);
    }
    for (name, value) in FIXED {
        command.env(name, value);
    }
}
