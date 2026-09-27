//! A native, dependency-light approval dialog for Linux.
//!
//! The release binary is static musl (`x86_64`/`aarch64-unknown-linux-musl`)
//! and must not link or dlopen a C library: no libwayland-client, no
//! libX11/libxcb, no GTK, no fontconfig, no GL. So this dialog is written
//! entirely in Rust: [`render`] lays the dialog out and draws it into a
//! plain pixel buffer, and [`wayland`] / [`x11`] each open a raw protocol
//! connection and drive that buffer through whichever windowing system is
//! running.

mod render;
mod wayland;
mod x11;

/// What a backend attempt produced.
enum Attempt {
    /// The user made a decision.
    Ok(bool),
    /// This backend could not be used (no display, connection refused,
    /// missing global, or some other unrecoverable protocol error). The
    /// string is a short, human readable reason for the final error message
    /// if every backend fails.
    Unavailable(String),
}

/// Blocking. Shows a modal-style window with `title`, a wrapped `message`,
/// and two buttons: "Deny" (default, focused, Escape and window close
/// choose it) and `allow_label`. `Ok(true)` means allowed, `Ok(false)` means
/// denied or closed, `Err(msg)` means no display could be opened (the
/// caller then falls back to a terminal prompt).
pub(crate) fn ask(title: &str, message: &str, allow_label: &str) -> Result<bool, String> {
    let mut reasons = Vec::new();

    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        match wayland::attempt(title, message, allow_label) {
            Attempt::Ok(decision) => return Ok(decision),
            Attempt::Unavailable(reason) => reasons.push(format!("wayland: {reason}")),
        }
    } else {
        reasons.push("wayland: WAYLAND_DISPLAY is not set".to_string());
    }

    if std::env::var_os("DISPLAY").is_some() {
        match x11::attempt(title, message, allow_label) {
            Attempt::Ok(decision) => return Ok(decision),
            Attempt::Unavailable(reason) => reasons.push(format!("x11: {reason}")),
        }
    } else {
        reasons.push("x11: DISPLAY is not set".to_string());
    }

    Err(format!("no display could be opened ({})", reasons.join("; ")))
}
