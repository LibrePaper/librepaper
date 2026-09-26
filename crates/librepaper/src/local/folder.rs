//! Native folder consent names the requesting site and document. Only an
//! opaque binding identifier returns to that site. Dialogs are cancellable,
//! time bounded, and serialized by the service.

use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;

async fn output(command: &mut Command) -> Result<std::process::Output, String> {
    command.kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(300), command.output())
        .await
        .map_err(|_| "Folder selection timed out. Choose the project folder again.".to_string())?
        .map_err(|_| {
            "Could not open the folder chooser. On Linux, install zenity or kdialog.".to_string()
        })
}

pub async fn choose_directory(origin: &str, project: &str) -> Result<PathBuf, String> {
    let title =
        format!("Allow {origin} to render {project} using this folder (contents are not uploaded)");
    let selected = if cfg!(target_os = "macos") {
        // Pass the prompt as data, never splice website text into AppleScript.
        output(Command::new("osascript").args(["-e", "on run argv\nreturn POSIX path of (choose folder with prompt (item 1 of argv))\nend run", &title])).await?
    } else if cfg!(windows) {
        let mut command = Command::new("powershell");
        command.env("LIBREPAPER_FOLDER_PROMPT", &title).args([
            "-NoProfile", "-STA", "-Command",
            "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new(); Add-Type -AssemblyName System.Windows.Forms; $d=New-Object System.Windows.Forms.FolderBrowserDialog; $d.Description=$env:LIBREPAPER_FOLDER_PROMPT; if($d.ShowDialog() -eq 'OK'){ $d.SelectedPath }",
        ]);
        output(&mut command).await?
    } else {
        let mut zenity = Command::new("zenity");
        zenity
            .args(["--file-selection", "--directory", "--title", &title])
            .kill_on_drop(true);
        match tokio::time::timeout(Duration::from_secs(300), zenity.output()).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                output(Command::new("kdialog").args([
                    "--title",
                    &title,
                    "--getexistingdirectory",
                    ".",
                ]))
                .await?
            }
            Ok(Err(_)) => return Err("Could not open the folder chooser.".into()),
            Err(_) => return Err("Folder selection timed out.".into()),
        }
    };
    if !selected.status.success() {
        return Err("Folder selection was cancelled.".into());
    }
    let text = String::from_utf8(selected.stdout)
        .map_err(|_| "The selected folder name is not valid UTF-8.")?;
    // Remove the dialog's line terminator, not valid spaces in a directory name.
    let text = text.trim_end_matches(['\r', '\n']);
    if text.is_empty() {
        return Err("Folder selection was cancelled.".into());
    }
    let path = PathBuf::from(text);
    if !path.is_absolute() || !path.is_dir() {
        return Err("Select an existing project folder.".into());
    }
    Ok(path)
}
