//! Native OS confirmation dialogs for permission requests. Serializes dialogs
//! so only one appears at a time, with a headless fallback for environments
//! without a display (Linux without DISPLAY or WAYLAND_DISPLAY).

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;

#[cfg(test)]
thread_local! {
    static SCRIPTED: std::cell::RefCell<Option<bool>> = const { std::cell::RefCell::new(None) };
}

pub(crate) struct Approval {
    pub title: String,
    pub message: String,
    pub allow_label: String,
}

pub(crate) enum Decision {
    Allowed,
    Denied,
    Unavailable(String),
}

static DIALOG_LOCK: Mutex<()> = Mutex::const_new(());
static PENDING: LazyLock<std::sync::Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

async fn output(command: &mut Command) -> Result<std::process::Output, String> {
    command.kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(300), command.output())
        .await
        .map_err(|_| "Approval request timed out.".to_string())?
        .map_err(|_| "Could not open the approval dialog.".to_string())
}

/// Ask for approval via a native OS dialog. The dialog is serialized so only
/// one appears at a time. Deny is the default button. On headless systems
/// (Linux without DISPLAY/WAYLAND_DISPLAY), generates a code and waits for it
/// to be approved via approve_code().
pub(crate) async fn ask(approval: &Approval) -> Decision {
    #[cfg(test)]
    {
        if let Some(allowed) = SCRIPTED.with(|s| *s.borrow()) {
            return if allowed {
                Decision::Allowed
            } else {
                Decision::Denied
            };
        }
    }

    let _dialog_guard = DIALOG_LOCK.lock().await;

    if cfg!(target_os = "macos") {
        ask_macos(approval).await
    } else if cfg!(windows) {
        ask_windows(approval).await
    } else {
        ask_linux(approval).await
    }
}

async fn ask_macos(approval: &Approval) -> Decision {
    let title = &approval.title;
    let message = &approval.message;
    let allow_label = &approval.allow_label;

    let script = "on run argv\ndisplay dialog (item 2 of argv) with title (item 1 of argv) buttons {\"Deny\", (item 3 of argv)} default button \"Deny\"\nif button returned of result = (item 3 of argv) then\nreturn \"allowed\"\nelse\nreturn \"denied\"\nend if\nend run";

    let result =
        output(Command::new("osascript").args(["-e", script, title, message, allow_label])).await;

    match result {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout);
            if text.trim() == "allowed" {
                Decision::Allowed
            } else {
                Decision::Denied
            }
        }
        Err(e) => Decision::Unavailable(e),
    }
}

async fn ask_windows(approval: &Approval) -> Decision {
    let title = &approval.title;
    let message = &approval.message;
    let allow_label = &approval.allow_label;

    let mut command = Command::new("powershell");
    command.env("LIBREPAPER_APPROVAL_TITLE", title);
    command.env("LIBREPAPER_APPROVAL_MESSAGE", message);
    command.env("LIBREPAPER_APPROVAL_ALLOW", allow_label);
    command.args([
        "-NoProfile",
        "-STA",
        "-Command",
        "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new(); Add-Type -AssemblyName System.Windows.Forms; $result = [System.Windows.Forms.MessageBox]::Show($env:LIBREPAPER_APPROVAL_MESSAGE, $env:LIBREPAPER_APPROVAL_TITLE, [System.Windows.Forms.MessageBoxButtons]::YesNo, [System.Windows.Forms.MessageBoxIcon]::Question, [System.Windows.Forms.MessageBoxDefaultButton]::No); if ($result -eq 'Yes') { 'allowed' } else { 'denied' }",
    ]);

    let result = output(&mut command).await;

    match result {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout);
            if text.trim() == "allowed" {
                Decision::Allowed
            } else {
                Decision::Denied
            }
        }
        Err(e) => Decision::Unavailable(e),
    }
}

async fn ask_linux(approval: &Approval) -> Decision {
    let title = &approval.title;
    let message = &approval.message;
    let allow_label = &approval.allow_label;

    let has_display = std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok();

    if has_display {
        let mut zenity = Command::new("zenity");
        zenity.args([
            "--question",
            "--no-markup",
            "--title",
            title,
            "--text",
            message,
            "--ok-label",
            allow_label,
            "--cancel-label",
            "Deny",
        ]);
        zenity.kill_on_drop(true);

        match tokio::time::timeout(Duration::from_secs(300), zenity.output()).await {
            Ok(Ok(result)) => {
                if result.status.success() {
                    Decision::Allowed
                } else {
                    Decision::Denied
                }
            }
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut kdialog = Command::new("kdialog");
                kdialog.args([
                    "--title",
                    title,
                    "--yesno",
                    message,
                    "--yes-label",
                    allow_label,
                    "--no-label",
                    "Deny",
                ]);
                kdialog.kill_on_drop(true);

                match tokio::time::timeout(Duration::from_secs(300), kdialog.output()).await {
                    Ok(Ok(result)) => {
                        if result.status.success() {
                            Decision::Allowed
                        } else {
                            Decision::Denied
                        }
                    }
                    Ok(Err(_)) => ask_headless(approval).await,
                    Err(_) => Decision::Denied,
                }
            }
            Ok(Err(_)) => ask_headless(approval).await,
            Err(_) => Decision::Denied,
        }
    } else {
        ask_headless(approval).await
    }
}

async fn ask_headless(approval: &Approval) -> Decision {
    let code = super::pairing::generate_code();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    {
        let mut pending = PENDING.lock().unwrap();
        pending.insert(code.clone(), tx);
    }

    eprintln!(
        "LibrePaper wants approval: {}. Install zenity for a dialog, or run: librepaper local approve {}",
        approval.title, code
    );

    let code_clone = code.clone();
    match tokio::time::timeout(Duration::from_secs(300), rx).await {
        Ok(Ok(())) => Decision::Allowed,
        Ok(Err(_)) => {
            let mut pending = PENDING.lock().unwrap();
            pending.remove(&code_clone);
            Decision::Denied
        }
        Err(_) => {
            let mut pending = PENDING.lock().unwrap();
            pending.remove(&code_clone);
            Decision::Denied
        }
    }
}

/// Called from the CLI to approve a pending approval request. Returns true if
/// the code matched a pending request and it was approved, false otherwise.
/// Each code is single-use.
pub(crate) fn approve_code(code: &str) -> bool {
    let code = code.trim();
    let mut pending = match PENDING.lock() {
        Ok(guard) => guard,
        Err(_) => return false,
    };
    if let Some(tx) = pending.remove(code) {
        let _ = tx.send(());
        true
    } else {
        false
    }
}

#[cfg(test)]
pub(crate) fn script(allowed: bool) {
    SCRIPTED.with(|s| {
        *s.borrow_mut() = Some(allowed);
    });
}

#[cfg(test)]
pub(crate) fn unscript() {
    SCRIPTED.with(|s| {
        *s.borrow_mut() = None;
    });
}
#[cfg(test)]
mod tests {
    use super::*;

    fn register(code: &str) -> tokio::sync::oneshot::Receiver<()> {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let mut pending = PENDING.lock().unwrap();
        pending.insert(code.to_string(), tx);
        rx
    }

    #[test]
    fn unknown_code_returns_false() {
        assert!(!approve_code("000000"));
    }

    #[test]
    fn registered_code_resolves_once() {
        let code = "123456";
        let rx = register(code);

        let result_first = approve_code(code);
        assert!(result_first);

        let result_second = approve_code(code);
        assert!(!result_second);

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            assert!(rx.await.is_ok());
        });
    }
}
