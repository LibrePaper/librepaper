//! Downloads ask where to save the file, like a browser does.
//!
//! The request callback runs synchronously on the main thread, so it cannot
//! show a dialog without blocking the event loop. The file goes to the app
//! cache first, and the save dialog opens once the download has finished.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use tauri::webview::DownloadEvent;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, FilePath};
use url::Url;

use crate::shell_log;

const DIR: &str = "downloads";
const FALLBACK: &str = "download";

struct Pending {
    url: String,
    temp: PathBuf,
    name: String,
}

#[derive(Default)]
pub struct Downloads {
    next: AtomicU64,
    pending: Mutex<Vec<Pending>>,
}

// Returns true so the download goes ahead in every case.
pub fn handle(app: &AppHandle, event: DownloadEvent<'_>) -> bool {
    match event {
        DownloadEvent::Requested { url, destination } => requested(app, &url, destination),
        DownloadEvent::Finished { url, success, .. } => finished(app, &url, success),
        _ => {}
    }
    true
}

// Points the download at a temp file. The platform default still saves the
// file when the temp folder cannot be made.
fn requested(app: &AppHandle, url: &Url, destination: &mut PathBuf) {
    let name = file_name(destination.as_path(), url);
    let dir = match app.path().app_cache_dir() {
        Ok(dir) => dir.join(DIR),
        Err(err) => {
            shell_log::write(&format!("could not find the cache folder: {err}"));
            return;
        }
    };
    if let Err(err) = std::fs::create_dir_all(&dir) {
        shell_log::write(&format!("could not create {}: {err}", dir.display()));
        return;
    }
    let downloads = app.state::<Downloads>();
    let n = downloads.next.fetch_add(1, Ordering::Relaxed);
    let temp = dir.join(format!("{n}-{name}"));
    *destination = temp.clone();
    let mut pending = downloads.pending.lock().unwrap_or_else(|err| err.into_inner());
    pending.push(Pending {
        url: url.to_string(),
        temp,
        name,
    });
}

// Opens the save dialog for a finished download. A failed one is discarded.
fn finished(app: &AppHandle, url: &Url, success: bool) {
    let downloads = app.state::<Downloads>();
    let entry = {
        let mut pending = downloads.pending.lock().unwrap_or_else(|err| err.into_inner());
        take(&mut pending, url.as_str())
    };
    let Some(entry) = entry else {
        shell_log::write(&format!("finished download was not tracked: {url}"));
        return;
    };
    if !success {
        shell_log::write(&format!("download failed: {url}"));
        discard(&entry.temp);
        return;
    }
    let temp = entry.temp;
    app.dialog()
        .file()
        .set_file_name(&entry.name)
        .save_file(move |chosen| save(&temp, chosen));
}

// Removes the entry for a url. Without a match, the only pending entry is used.
fn take(pending: &mut Vec<Pending>, url: &str) -> Option<Pending> {
    if let Some(index) = pending.iter().position(|entry| entry.url == url) {
        return Some(pending.remove(index));
    }
    if pending.len() == 1 {
        return pending.pop();
    }
    None
}

fn save(temp: &Path, chosen: Option<FilePath>) {
    let target = match chosen.map(FilePath::into_path) {
        Some(Ok(target)) => target,
        Some(Err(err)) => {
            shell_log::write(&format!("could not read the chosen path: {err}"));
            discard(temp);
            return;
        }
        None => {
            discard(temp);
            return;
        }
    };
    if let Err(err) = move_file(temp, &target) {
        shell_log::write(&format!(
            "could not save the download to {}: {err}",
            target.display()
        ));
    }
}

// Rename is quick but fails across filesystems, so copy is the fallback.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

fn discard(temp: &Path) {
    if let Err(err) = std::fs::remove_file(temp) {
        shell_log::write(&format!("could not remove {}: {err}", temp.display()));
    }
}

// The suggested name wins, then the last url segment, then a fixed word.
pub fn file_name(destination: &Path, url: &Url) -> String {
    destination
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| {
            url.path_segments()
                .and_then(|mut segments| segments.next_back())
                .filter(|segment| !segment.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| FALLBACK.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_prefers_the_destination() {
        let url = Url::parse("https://example.org/a/report.pdf").unwrap();
        let destination = Path::new("/home/u/Downloads/saved.pdf");
        assert_eq!(file_name(destination, &url), "saved.pdf");
    }

    #[test]
    fn file_name_falls_back_to_the_url_then_a_word() {
        let url = Url::parse("https://example.org/a/report.pdf").unwrap();
        assert_eq!(file_name(Path::new("/"), &url), "report.pdf");

        let blob = Url::parse("blob:https://x/uuid").unwrap();
        assert_eq!(file_name(Path::new("/"), &blob), "download");
    }
}
