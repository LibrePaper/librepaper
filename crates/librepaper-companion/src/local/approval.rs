//! Per-service approval prompts.
//!
//! A request that changes what the service runs on this computer is confirmed
//! in a native operating system dialog, never in a web page. Each running
//! local service owns its broker; closing it denies everything waiting.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{watch, Mutex};

const DEFAULT_TTL: Duration = Duration::from_secs(300);

#[cfg(test)]
thread_local! {
    static SCRIPTED: std::cell::RefCell<Option<bool>> = const { std::cell::RefCell::new(None) };
}

#[derive(Clone, Debug)]
pub(crate) struct Approval {
    pub title: String,
    pub message: String,
    pub allow_label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    Allowed,
    Denied,
}

/// Asks one question at a time through the operating system. A question the
/// user does not answer within its time limit, or one still open when the
/// service shuts down, is denied.
#[derive(Clone)]
pub(crate) struct ApprovalBroker {
    one_at_a_time: Arc<Mutex<()>>,
    closed: Arc<watch::Sender<bool>>,
}

impl Default for ApprovalBroker {
    fn default() -> Self {
        Self {
            one_at_a_time: Arc::new(Mutex::new(())),
            closed: Arc::new(watch::channel(false).0),
        }
    }
}

impl ApprovalBroker {
    pub(crate) async fn ask(&self, approval: &Approval, ttl: Duration) -> Decision {
        #[cfg(test)]
        let scripted = SCRIPTED.with(|script| *script.borrow());

        let mut closed = self.closed.subscribe();
        if *closed.borrow() {
            return Decision::Denied;
        }
        #[cfg(test)]
        if let Some(allowed) = scripted {
            return if allowed {
                Decision::Allowed
            } else {
                Decision::Denied
            };
        }
        let question = async {
            let _turn = self.one_at_a_time.lock().await;
            native_confirm(approval).await
        };
        let decision = tokio::select! {
            decision = question => decision,
            _ = closed.wait_for(|closed| *closed) => Decision::Denied,
            _ = tokio::time::sleep(ttl.min(DEFAULT_TTL)) => Decision::Denied,
        };
        if decision == Decision::Denied {
            tracing::info!("denied: {}", approval.title);
        }
        decision
    }

    /// Close this broker to new requests and deny everything waiting. A
    /// request racing shutdown sees the closed flag and is denied at once.
    pub(crate) fn deny_all(&self) {
        self.closed.send_replace(true);
    }
}

#[cfg(windows)]
async fn native_confirm(approval: &Approval) -> Decision {
    let answer = rfd::AsyncMessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title(approval.title.as_str())
        .set_description(format!(
            "{}\n\nYes: {}\nNo: deny",
            approval.message, approval.allow_label
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        .await;
    if answer == rfd::MessageDialogResult::Yes {
        Decision::Allowed
    } else {
        Decision::Denied
    }
}

#[cfg(not(windows))]
async fn native_confirm(approval: &Approval) -> Decision {
    eprintln!(
        "LibrePaper cannot show a confirmation dialog on this platform, so it denied: {}",
        approval.title
    );
    Decision::Denied
}

#[cfg(test)]
pub(crate) fn script(allowed: bool) {
    SCRIPTED.with(|script| *script.borrow_mut() = Some(allowed));
}

#[cfg(test)]
pub(crate) fn unscript() {
    SCRIPTED.with(|script| *script.borrow_mut() = None);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Approval {
        Approval {
            title: "Test".into(),
            message: "Allow?".into(),
            allow_label: "Allow".into(),
        }
    }

    #[tokio::test]
    async fn shutdown_rejects_later_approval_requests() {
        let broker = ApprovalBroker::default();
        broker.deny_all();
        let after_shutdown = tokio::time::timeout(
            Duration::from_millis(50),
            broker.ask(&request(), Duration::from_secs(300)),
        )
        .await
        .expect("an approval created after shutdown must not wait for its TTL");
        assert_eq!(after_shutdown, Decision::Denied);
    }

    #[cfg(not(windows))]
    #[tokio::test]
    async fn a_platform_without_a_dialog_denies() {
        let broker = ApprovalBroker::default();
        assert_eq!(
            broker.ask(&request(), Duration::from_secs(5)).await,
            Decision::Denied
        );
    }
}
