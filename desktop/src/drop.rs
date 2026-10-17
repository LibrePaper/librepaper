// WebKitGTK hides dropped files from the page, so on Linux the shell takes the
// drop itself. It lists the files, keeps their paths, and tells the page
// through the scripts in main.rs. The page then fetches each file from the
// shell by token and index. Absolute paths never reach the page.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde_json::json;
use tauri::{DragDropEvent, Manager, WebviewWindow, WindowEvent};

use crate::{shell_log, zoom};

// Older drops are forgotten beyond this many.
const KEPT: usize = 8;

#[derive(Default)]
pub struct Dropped {
    next: AtomicU64,
    drops: Mutex<HashMap<u64, Vec<Option<PathBuf>>>>,
}

impl Dropped {
    fn store(&self, paths: Vec<PathBuf>) -> u64 {
        let token = self.next.fetch_add(1, Ordering::Relaxed);
        let mut drops = self.drops.lock().unwrap_or_else(|err| err.into_inner());
        drops.insert(token, paths.into_iter().map(Some).collect());
        while drops.len() > KEPT {
            let Some(oldest) = drops.keys().min().copied() else {
                break;
            };
            drops.remove(&oldest);
        }
        token
    }

    // Hands out one file. Each file is served once.
    pub fn take(&self, token: u64, index: usize) -> Option<PathBuf> {
        let mut drops = self.drops.lock().unwrap_or_else(|err| err.into_inner());
        drops.get_mut(&token)?.get_mut(index)?.take()
    }
}

struct Listing {
    files: Vec<(String, PathBuf)>,
    folders: Vec<String>,
}

// Lists dropped paths. A file keeps its name. A folder is walked, and its
// files and subfolders get paths relative to the drop, joined with "/".
// Symlinks and names that are not UTF-8 are skipped.
fn list(dropped: &[PathBuf]) -> Listing {
    let mut listing = Listing {
        files: Vec::new(),
        folders: Vec::new(),
    };
    for path in dropped {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            continue;
        };
        if meta.is_file() {
            listing.files.push((name.to_string(), path.clone()));
        } else if meta.is_dir() {
            walk(path, name, &mut listing);
        }
    }
    listing
}

fn walk(dir: &Path, relative: &str, listing: &mut Listing) {
    listing.folders.push(relative.to_string());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
        .collect();
    children.sort();
    for (name, path) in children {
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let child = format!("{relative}/{name}");
        if meta.is_file() {
            listing.files.push((child, path));
        } else if meta.is_dir() {
            walk(&path, &child, listing);
        }
    }
}

fn call(window: &WebviewWindow, script: String) {
    if let Err(err) = window.eval(script) {
        shell_log::write(&format!("could not tell the page about a drop: {err}"));
    }
}

// Turns a GTK widget position into the page's CSS pixels.
fn page_point(window: &WebviewWindow, x: f64, y: f64) -> (f64, f64) {
    let zoom = window.app_handle().state::<zoom::Zoom>().get();
    (x / zoom, y / zoom)
}

// Runs for every window event of a window. Only file drops matter.
pub fn handle(window: &WebviewWindow, event: &WindowEvent) {
    let WindowEvent::DragDrop(event) = event else {
        return;
    };
    match event {
        DragDropEvent::Enter { position, .. } | DragDropEvent::Over { position } => {
            let (x, y) = page_point(window, position.x, position.y);
            call(
                window,
                format!("window.__librepaperDrag?.(\"over\", {x}, {y})"),
            );
        }
        DragDropEvent::Leave => call(window, "window.__librepaperDrag?.(\"leave\")".to_string()),
        DragDropEvent::Drop { paths, position } => {
            let (x, y) = page_point(window, position.x, position.y);
            let window = window.clone();
            let paths = paths.clone();
            // Walking a large folder must not block the main thread.
            tauri::async_runtime::spawn_blocking(move || {
                let listing = list(&paths);
                let entries: Vec<_> = listing
                    .files
                    .iter()
                    .enumerate()
                    .map(|(index, (path, _))| json!({ "path": path, "index": index }))
                    .collect();
                let page = json!({ "files": entries, "folders": listing.folders });
                let token = window
                    .app_handle()
                    .state::<Dropped>()
                    .store(listing.files.into_iter().map(|(_, path)| path).collect());
                call(
                    &window,
                    format!("window.__librepaperDrop?.({token}, {x}, {y}, {page})"),
                );
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lp-drop-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn walks_nested_folders_in_name_order() {
        let root = scratch("nested");
        let docs = root.join("docs");
        std::fs::create_dir_all(docs.join("sub")).unwrap();
        std::fs::write(docs.join("b.txt"), "b").unwrap();
        std::fs::write(docs.join("a.txt"), "a").unwrap();
        std::fs::write(docs.join("sub").join("c.txt"), "c").unwrap();
        std::fs::write(root.join("loose.txt"), "l").unwrap();

        let listing = list(&[docs.clone(), root.join("loose.txt")]);
        let names: Vec<&str> = listing
            .files
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(
            names,
            ["docs/a.txt", "docs/b.txt", "docs/sub/c.txt", "loose.txt"]
        );
        assert_eq!(listing.folders, ["docs", "docs/sub"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn skips_symlinks() {
        let root = scratch("links");
        let dir = root.join("dir");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("real.txt"), "r").unwrap();
        std::os::unix::fs::symlink(&dir, dir.join("loop")).unwrap();
        std::os::unix::fs::symlink(dir.join("real.txt"), dir.join("alias.txt")).unwrap();

        let listing = list(&[dir]);
        let names: Vec<&str> = listing
            .files
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        assert_eq!(names, ["dir/real.txt"]);
        assert_eq!(listing.folders, ["dir"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn serves_each_file_once() {
        let dropped = Dropped::default();
        let token = dropped.store(vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(dropped.take(token, 1), Some(PathBuf::from("/b")));
        assert_eq!(dropped.take(token, 1), None);
        assert_eq!(dropped.take(token, 0), Some(PathBuf::from("/a")));
        assert_eq!(dropped.take(token, 2), None);
        assert_eq!(dropped.take(token + 1, 0), None);
    }

    #[test]
    fn forgets_old_drops() {
        let dropped = Dropped::default();
        let first = dropped.store(vec![PathBuf::from("/a")]);
        for _ in 0..KEPT {
            dropped.store(vec![PathBuf::from("/x")]);
        }
        assert_eq!(dropped.take(first, 0), None);
    }
}
