//! Native folder consent names the requesting site and project. Only an
//! opaque binding identifier returns to that site. Dialogs are cancellable,
//! time bounded, and serialized by the service.

use std::path::PathBuf;

/// The error every chooser reports when the person closes it without a choice.
pub const CANCELLED: &str = "Folder selection was cancelled.";

#[cfg(any(target_os = "macos", all(test, unix)))]
use std::time::Duration;

#[cfg(target_os = "macos")]
const PICKER_HELPER_ARG: &str = "__librepaper_pick_folder";
#[cfg(any(target_os = "macos", all(test, unix)))]
const PICKER_OUTPUT_LIMIT: usize = 16 * 1024;
#[cfg(target_os = "macos")]
const PICKER_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[cfg(any(target_os = "macos", all(test, unix)))]
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
enum PickerResponse {
    Selected { path_b64: String },
    Cancelled,
    Error { message: String },
}

/// Handle the private synchronous picker command before the main process
/// creates Tokio. Returns true whenever this invocation is the helper command.
#[cfg(target_os = "macos")]
pub fn run_folder_picker_helper() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new(PICKER_HELPER_ARG)) {
        return false;
    }
    let title = args.next().and_then(|arg| arg.into_string().ok());
    let response = match title {
        Some(title) if args.next().is_none() => {
            std::panic::catch_unwind(|| choose_directory_sync(&title))
                .unwrap_or_else(|_| Err("The folder chooser failed.".into()))
                .map(|path| match path {
                    Some(path) => {
                        use base64::Engine as _;
                        use std::os::unix::ffi::OsStrExt as _;
                        PickerResponse::Selected {
                            path_b64: base64::engine::general_purpose::STANDARD
                                .encode(path.as_os_str().as_bytes()),
                        }
                    }
                    None => PickerResponse::Cancelled,
                })
                .unwrap_or_else(|message| PickerResponse::Error { message })
        }
        _ => PickerResponse::Error {
            message: "Invalid folder chooser request.".into(),
        },
    };
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    let _ = serde_json::to_writer(&mut stdout, &response);
    let _ = std::io::Write::write_all(&mut stdout, b"\n");
    let _ = std::io::Write::flush(&mut stdout);
    true
}

#[cfg(not(target_os = "macos"))]
pub fn run_folder_picker_helper() -> bool {
    false
}

pub async fn choose_backup_directory(origin: &str) -> Result<PathBuf, String> {
    let title = format!("Choose where to save {origin} backups");
    choose_directory_with_title(&title).await
}

/// The native folder chooser, with `title` shown on it.
pub async fn choose_directory_with_title(title: &str) -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    {
        choose_directory_macos(title).await
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
    let executable = super::paths::current_executable()
        .map_err(|error| format!("Could not start the folder chooser: {error}"))?;
    let mut command = tokio::process::Command::new(executable);
    command.arg(PICKER_HELPER_ARG).arg(title);
    run_picker_process(command, PICKER_TIMEOUT).await
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

#[cfg(windows)]
async fn choose_directory_rfd(title: &str) -> Result<PathBuf, String> {
    let handle = rfd::AsyncFileDialog::new()
        .set_title(title)
        .pick_folder()
        .await;

    let Some(handle) = handle else {
        return Err(CANCELLED.into());
    };

    let path = handle.path().to_path_buf();
    if !path.is_absolute() || !path.is_dir() {
        return Err("Select an existing project folder.".into());
    }
    Ok(path)
}

#[cfg(any(target_os = "macos", all(test, unix)))]
async fn run_picker_process(
    mut command: tokio::process::Command,
    timeout: Duration,
) -> Result<PathBuf, String> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start the folder chooser: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "The folder chooser returned no response.".to_string())?;

    let result = tokio::time::timeout(timeout, async {
        let mut output = Vec::new();
        stdout
            .take((PICKER_OUTPUT_LIMIT + 1) as u64)
            .read_to_end(&mut output)
            .await
            .map_err(|error| format!("Could not read the folder chooser response: {error}"))?;
        if output.len() > PICKER_OUTPUT_LIMIT {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err("The folder chooser returned an oversized response.".into());
        }
        let status = child
            .wait()
            .await
            .map_err(|error| format!("Could not finish the folder chooser: {error}"))?;
        decode_picker_process(status.success(), &output)
    })
    .await;
    result.unwrap_or_else(|_| Err("The folder chooser timed out.".into()))
}

#[cfg(any(target_os = "macos", all(test, unix)))]
fn decode_picker_response(output: &[u8]) -> Result<PathBuf, String> {
    let response: PickerResponse = serde_json::from_slice(output)
        .map_err(|_| "The folder chooser returned an invalid response.".to_string())?;
    match response {
        PickerResponse::Selected { path_b64 } => {
            use base64::Engine as _;
            use std::os::unix::ffi::OsStringExt as _;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(path_b64)
                .map_err(|_| "The folder chooser returned an invalid path.".to_string())?;
            let path = PathBuf::from(std::ffi::OsString::from_vec(bytes));
            if !path.is_absolute() || !path.is_dir() {
                Err("Select an existing project folder.".into())
            } else {
                Ok(path)
            }
        }
        PickerResponse::Cancelled => Err(CANCELLED.into()),
        PickerResponse::Error { message } => Err(message),
    }
}

#[cfg(any(target_os = "macos", all(test, unix)))]
fn decode_picker_process(success: bool, output: &[u8]) -> Result<PathBuf, String> {
    if output.len() > PICKER_OUTPUT_LIMIT {
        return Err("The folder chooser returned an oversized response.".into());
    }
    if !success {
        return Err("The folder chooser process failed.".into());
    }
    decode_picker_response(output)
}

#[cfg(all(test, unix))]
mod picker_protocol_tests {
    use super::*;

    #[test]
    fn helper_protocol_handles_cancel_error_and_malformed_output() {
        assert_eq!(
            decode_picker_response(br#"{"result":"cancelled"}"#).unwrap_err(),
            CANCELLED
        );
        assert_eq!(
            decode_picker_response(br#"{"result":"error","message":"native failure"}"#)
                .unwrap_err(),
            "native failure"
        );
        assert!(decode_picker_response(b"not json")
            .unwrap_err()
            .contains("invalid response"));
    }

    #[test]
    fn helper_protocol_accepts_only_an_existing_absolute_path() {
        use base64::Engine as _;
        use std::os::unix::ffi::OsStrExt as _;

        let path = std::env::current_dir().unwrap();
        let response = serde_json::json!({
            "result": "selected",
            "path_b64": base64::engine::general_purpose::STANDARD
                .encode(path.as_os_str().as_bytes())
        });
        assert_eq!(
            decode_picker_response(&serde_json::to_vec(&response).unwrap()).unwrap(),
            std::env::current_dir().unwrap()
        );
        let relative = serde_json::json!({
            "result": "selected",
            "path_b64": base64::engine::general_purpose::STANDARD.encode(b"relative")
        });
        assert!(decode_picker_response(&serde_json::to_vec(&relative).unwrap()).is_err());

        use std::os::unix::ffi::OsStringExt as _;
        let directory = tempfile::tempdir().unwrap();
        let non_utf8 = directory
            .path()
            .join(std::ffi::OsString::from_vec(b"folder-\xff".to_vec()));
        std::fs::create_dir(&non_utf8).unwrap();
        let response = serde_json::json!({
            "result": "selected",
            "path_b64": base64::engine::general_purpose::STANDARD
                .encode(non_utf8.as_os_str().as_bytes())
        });
        assert_eq!(
            decode_picker_response(&serde_json::to_vec(&response).unwrap()).unwrap(),
            non_utf8
        );
    }

    #[test]
    fn helper_process_failures_are_reported_without_parsing_output() {
        assert_eq!(
            decode_picker_process(false, br#"{"result":"cancelled"}"#).unwrap_err(),
            "The folder chooser process failed."
        );
        assert_eq!(
            decode_picker_process(true, &vec![b'x'; PICKER_OUTPUT_LIMIT + 1]).unwrap_err(),
            "The folder chooser returned an oversized response."
        );
    }

    #[tokio::test]
    async fn fake_child_timeout_cancels_the_picker_process() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("timeout.pid");
        let command = fake_sleep_child(&pid_file);
        assert_eq!(
            run_picker_process(command, Duration::from_secs(2))
                .await
                .unwrap_err(),
            "The folder chooser timed out."
        );
        wait_until_pid_file_exists(&pid_file).await;
        wait_until_process_exits(read_pid(&pid_file)).await;
    }

    #[tokio::test]
    async fn dropping_picker_future_kills_its_child() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("cancelled.pid");
        let task = tokio::spawn(run_picker_process(
            fake_sleep_child(&pid_file),
            Duration::from_secs(30),
        ));
        wait_until_pid_file_exists(&pid_file).await;
        let pid = read_pid(&pid_file);
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        wait_until_process_exits(pid).await;
    }

    #[tokio::test]
    async fn fake_child_cancel_and_nonzero_exit_are_reported() {
        let mut cancelled = tokio::process::Command::new("/bin/sh");
        cancelled.args(["-c", "printf '%s' '{\"result\":\"cancelled\"}'"]);
        assert_eq!(
            run_picker_process(cancelled, Duration::from_secs(1))
                .await
                .unwrap_err(),
            CANCELLED
        );

        let mut failed = tokio::process::Command::new("/bin/sh");
        failed.args(["-c", "exit 3"]);
        assert_eq!(
            run_picker_process(failed, Duration::from_secs(1))
                .await
                .unwrap_err(),
            "The folder chooser process failed."
        );
    }

    fn fake_sleep_child(pid_file: &std::path::Path) -> tokio::process::Command {
        let mut command = tokio::process::Command::new("/bin/sh");
        command.args(["-c", "echo $$ > \"$1\"; exec sleep 30", "picker-test"]);
        command.arg(pid_file);
        command
    }

    async fn wait_until_pid_file_exists(path: &std::path::Path) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while !path.exists() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(path.exists(), "fake child did not write its PID");
    }

    fn read_pid(path: &std::path::Path) -> u32 {
        std::fs::read_to_string(path)
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    async fn wait_until_process_exits(pid: u32) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            let status = std::process::Command::new("/bin/sh")
                .args(["-c", "kill -0 \"$1\"", "kill-check", &pid.to_string()])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            if !status.success() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("picker child {pid} remained alive after cancellation");
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
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
                CANCELLED.to_string()
            } else {
                "Could not open the folder chooser: no desktop portal is running.".to_string()
            }
        })?;

        let uri = selected
            .uris()
            .first()
            .ok_or_else(|| CANCELLED.to_string())?;

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
