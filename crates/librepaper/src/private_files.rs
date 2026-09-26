//! Durable private-file publication shared by local subsystems.
//!
//! One implementation, because the requirements only look small. The file
//! has to be *created* private rather than made private after the fact --
//! bytes written to a default-mode file are readable by anyone on the host
//! for however long the `chmod` takes to land, and a crash in that window
//! leaves them readable for good. It has to reach disk before the rename,
//! and the rename has to reach disk too, or a crash can publish a name with
//! no bytes behind it. And a failed publication has to take its temporary
//! file with it rather than leaving litter beside a secret.
//!
//! Callers that want create-only semantics -- a backup that must never
//! overwrite, an auth key that is written once -- do not belong here: this
//! replaces whatever is at `target`.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// A temporary beside the target, named so that two publications of two
/// different files -- or of the same one -- can never pick the same path.
/// A stable per-purpose name was enough only while every writer held the
/// same lock.
fn temporary_path(parent: &Path, target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    parent.join(format!(".{name}.{}.tmp", crate::util::new_id()))
}

/// The directory a target lives in. A bare relative name such as
/// `tokens.json` has an empty parent, which is the current directory and
/// not, as `create_dir_all("")` would have it, an error.
fn parent_of(target: &Path) -> &Path {
    target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}

pub(crate) fn publish(target: &Path, bytes: &[u8], what: &str) -> Result<(), String> {
    let parent = parent_of(target);
    fs::create_dir_all(parent)
        .map_err(|error| format!("could not create {what} directory: {error}"))?;
    let temporary = temporary_path(parent, target);
    // Removes the temporary however this function leaves, including on the
    // early return of a `?`. Nothing after a successful rename can fail in a
    // way that would make removing the -- by then nonexistent -- temporary
    // wrong.
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(temporary.clone());

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Created at 0600, not chmodded to it afterwards: the window
        // between the two is when the bytes are world-readable.
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("could not create private {what}: {error}"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("could not durable-write {what}: {error}"))?;
    // Closed before the rename: a handle held across it is a Windows
    // sharing violation, and on every platform it is one fewer thing that
    // can still be buffering.
    drop(file);
    fs::rename(&temporary, target).map_err(|error| format!("could not publish {what}: {error}"))?;
    // The rename itself is only a directory entry until the directory is
    // synced. Unix-only: opening a directory as a file is not portable, and
    // the platforms that do not allow it do not need it.
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("could not durable-sync {what} directory: {error}"))?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// The permissions have to hold at creation, not after the write. A
    /// writer that wrote first and chmodded second left the bytes readable
    /// for the length of that window, and forever if it crashed inside it.
    #[test]
    fn the_file_is_private_from_the_moment_it_exists() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("nested").join("secret.json");
        publish(&target, b"a token", "test secret").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"a token");
        assert_eq!(mode(&target), 0o600);

        // A replacement is private too, and does not inherit a loose mode
        // from whatever it replaced.
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        publish(&target, b"a second token", "test secret").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"a second token");
        assert_eq!(mode(&target), 0o600);
    }

    /// A publication that cannot finish leaves nothing behind: no temporary
    /// beside the secret, and the previous contents still in place.
    #[test]
    fn a_failed_publication_leaves_no_temporary_and_does_not_destroy_the_target() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("secret.json");
        publish(&target, b"the surviving value", "test secret").unwrap();

        // Renaming onto a directory fails, which is a failure after the
        // temporary has been written and synced -- the window that used to
        // leave litter.
        let occupied = directory.path().join("occupied");
        fs::create_dir(&occupied).unwrap();
        assert!(publish(&occupied, b"never published", "test secret").is_err());

        let leftovers: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "left temporaries behind: {leftovers:?}"
        );
        assert_eq!(fs::read(&target).unwrap(), b"the surviving value");

        // And the next publication still works: recovery is just retrying.
        publish(&target, b"after the failure", "test secret").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"after the failure");
        assert_eq!(mode(&target), 0o600);
    }

    /// A bare relative name has an empty parent, which is the current
    /// directory rather than an error.
    #[test]
    fn a_relative_target_publishes_into_the_current_directory() {
        assert_eq!(parent_of(Path::new("tokens.json")), Path::new("."));
        assert_eq!(parent_of(Path::new("a/tokens.json")), Path::new("a"));
    }
}
