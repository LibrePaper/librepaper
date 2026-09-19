//! Durable private-file publication shared by local subsystems.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

fn temporary_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("state");
    target.with_file_name(format!(".{name}.{}.tmp", crate::util::new_id()))
}

pub(crate) fn publish(target: &Path, bytes: &[u8], what: &str) -> Result<(), String> {
    let parent = target
        .parent()
        .ok_or_else(|| format!("{what} has no parent directory"))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("could not create {what} directory: {error}"))?;
    let temporary = temporary_path(target);
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|error| format!("could not create private {what}: {error}"))?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("could not durable-write {what}: {error}"))?;
        fs::rename(&temporary, target)
            .map_err(|error| format!("could not publish {what}: {error}"))?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("could not durable-sync {what} directory: {error}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
