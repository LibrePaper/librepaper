//! Native folder consent names the requesting site and document. Only an
//! opaque binding identifier returns to that site. Dialogs are cancellable,
//! time bounded, and serialized by the service.

use std::path::PathBuf;

pub async fn choose_directory(origin: &str, project: &str) -> Result<PathBuf, String> {
    let title =
        format!("Allow {origin} to render {project} using this folder (contents are not uploaded)");

    choose_directory_with_title(&title).await
}

pub async fn choose_backup_directory(origin: &str) -> Result<PathBuf, String> {
    let title = format!("Choose where to save {origin} backups");
    choose_directory_with_title(&title).await
}

async fn choose_directory_with_title(title: &str) -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    {
        return choose_directory_macos(title).await;
    }
    #[cfg(windows)]
    {
        choose_directory_rfd(title).await
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        choose_directory_portal(title).await
    }
}

#[cfg(target_os = "macos")]
async fn choose_directory_macos(title: &str) -> Result<PathBuf, String> {
    super::cli::choose_directory_in_helper(title).await
}

/// Synchronous picker used only by the private helper process. It must run on
/// that process's main thread, before its async runtime is created.
#[cfg(target_os = "macos")]
pub(crate) fn choose_directory_sync(title: &str) -> Result<Option<PathBuf>, String> {
    let selected = rfd::FileDialog::new().set_title(title).pick_folder();
    if let Some(path) = selected {
        if !path.is_absolute() || !path.is_dir() {
            return Err("Select an existing project folder.".into());
        }
        Ok(Some(path))
    } else {
        Ok(None)
    }
}

#[cfg(any(target_os = "macos", windows))]
async fn choose_directory_rfd(title: &str) -> Result<PathBuf, String> {
    let handle = rfd::AsyncFileDialog::new()
        .set_title(title)
        .pick_folder()
        .await;

    let Some(handle) = handle else {
        return Err("Folder selection was cancelled.".into());
    };

    let path = handle.path().to_path_buf();
    if !path.is_absolute() || !path.is_dir() {
        return Err("Select an existing project folder.".into());
    }
    Ok(path)
}

#[cfg(not(any(target_os = "macos", windows)))]
async fn choose_directory_rfd(_title: &str) -> Result<PathBuf, String> {
    unreachable!("choose_directory_rfd is only called on macOS and Windows")
}

async fn choose_directory_portal(_title: &str) -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    {
        use ashpd::desktop::file_chooser::SelectedFiles;

        let request = SelectedFiles::open_file()
            .title(Some(_title))
            .directory(true)
            .modal(true)
            .send()
            .await
            .map_err(|_| {
                "Could not open the folder chooser: no desktop portal is running.".to_string()
            })?;

        let selected = request.response().map_err(|error| {
            if matches!(
                error,
                ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)
            ) {
                "Folder selection was cancelled.".to_string()
            } else {
                "Could not open the folder chooser: no desktop portal is running.".to_string()
            }
        })?;

        let uri = selected
            .uris()
            .first()
            .ok_or_else(|| "Folder selection was cancelled.".to_string())?;

        let path = uri
            .to_file_path()
            .map_err(|_| "The selected folder is not a local path.".to_string())?;

        if !path.is_absolute() || !path.is_dir() {
            return Err("Select an existing project folder.".into());
        }
        Ok(path)
    }

    #[cfg(not(target_os = "linux"))]
    {
        Err("The folder chooser is not available on this platform.".to_string())
    }
}
