//! Short notices from the shell, shown at the bottom of every window: the
//! local service restarting, an update downloading. The page is a remote
//! site, so the shell draws the notice itself.

use tauri::{AppHandle, Manager};

/// Shows `text` in every window. It disappears after `millis`, or stays
/// until the next notice when `millis` is 0.
pub fn show(app: &AppHandle, text: &str, millis: u64) {
    let text = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
    let script = format!(
        r#"(() => {{
  if (window.top !== window) return;
  let note = document.getElementById("lp-shell-notice");
  if (!note) {{
    note = document.createElement("div");
    note.id = "lp-shell-notice";
    note.setAttribute("role", "status");
    note.style.cssText = "position:fixed;left:50%;bottom:16px;transform:translateX(-50%);z-index:2147483647;max-width:80vw;padding:8px 14px;border-radius:8px;background:#1f2937;color:#fff;font:14px/1.4 system-ui,sans-serif;box-shadow:0 4px 16px rgba(0,0,0,.25);pointer-events:none";
    document.documentElement.appendChild(note);
  }}
  note.textContent = {text};
  clearTimeout(window.__lpNoticeTimer);
  if ({millis} > 0) window.__lpNoticeTimer = setTimeout(() => note.remove(), {millis});
}})()"#
    );
    for window in app.webview_windows().values() {
        if let Err(err) = window.eval(&script) {
            crate::shell_log::write(&format!("could not show a notice: {err}"));
        }
    }
}
