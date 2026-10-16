//! The shell's own log. A windowed app has no terminal, so what it would
//! print goes to desktop.log in the app's log folder as well.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

static FILE: OnceLock<PathBuf> = OnceLock::new();

pub fn init(dir: PathBuf) {
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join("desktop.log");
    // Keep at most about 2 MiB: the current file and one older one.
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > 1024 * 1024) {
        let _ = std::fs::rename(&path, dir.join("desktop.previous.log"));
    }
    // Created now, so the folder shows where the shell logs before anything goes wrong.
    let _ = OpenOptions::new().create(true).append(true).open(&path);
    let _ = FILE.set(path);
}

/// Where desktop.log is, for the diagnostics report.
pub fn location() -> String {
    FILE.get()
        .map_or_else(|| "(none)".into(), |path| path.display().to_string())
}

pub fn write(line: &str) {
    eprintln!("{line}");
    let Some(path) = FILE.get() else { return };
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let _ = writeln!(file, "{seconds} {line}");
    }
}

/// The last `lines` lines of desktop.log, or a note when there is none.
pub fn tail(lines: usize) -> String {
    let Some(path) = FILE.get() else {
        return "(no desktop log)".into();
    };
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let all: Vec<&str> = text.lines().collect();
            all[all.len().saturating_sub(lines)..].join("\n")
        }
        Err(_) => "(no desktop log yet)".into(),
    }
}
