//! Native OS confirmation dialogs for permission requests. Serializes dialogs
//! so only one appears at a time, with a headless fallback for environments
//! without a display (Linux without DISPLAY or WAYLAND_DISPLAY).

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;
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

    if cfg!(target_os = "macos") || cfg!(windows) {
        ask_rfd(approval).await
    } else {
        ask_linux(approval).await
    }
}

#[cfg(any(target_os = "macos", windows))]
async fn ask_rfd(approval: &Approval) -> Decision {
    let dialog = rfd::AsyncMessageDialog::new()
        .set_title(&approval.title)
        .set_description(&approval.message)
        .set_level(rfd::MessageLevel::Warning)
        .set_buttons(rfd::MessageButtons::OkCancelCustom(
            approval.allow_label.clone(),
            "Deny".to_string(),
        ));

    match dialog.show().await {
        rfd::MessageDialogResult::Custom(label) if label == approval.allow_label => {
            Decision::Allowed
        }
        _ => Decision::Denied,
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
async fn ask_rfd(_approval: &Approval) -> Decision {
    unreachable!("ask_rfd is only called on macOS and Windows")
}

async fn ask_linux(approval: &Approval) -> Decision {
    #[cfg(target_os = "linux")]
    {
        let has_display =
            std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok();

        if has_display {
            let title = approval.title.clone();
            let message = approval.message.clone();
            let allow_label = approval.allow_label.clone();

            let result = tokio::time::timeout(
                Duration::from_secs(300),
                tokio::task::spawn_blocking(move || {
                    crate::local::dialog::ask(&title, &message, &allow_label)
                }),
            )
            .await;

            return match result {
                Ok(Ok(Ok(true))) => Decision::Allowed,
                Ok(Ok(Ok(false))) => Decision::Denied,
                Ok(Ok(Err(_))) => ask_headless(approval).await,
                Ok(Err(_)) => ask_headless(approval).await,
                Err(_) => Decision::Denied,
            };
        }
    }

    ask_headless(approval).await
}

async fn ask_headless(approval: &Approval) -> Decision {
    let code = super::pairing::generate_code();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    {
        let mut pending = PENDING.lock().unwrap();
        pending.insert(code.clone(), tx);
    }

    eprintln!(
        "LibrePaper wants approval: {}. No display is available; run: librepaper local approve {}",
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
