//! X11 backend for the approval dialog.
//!
//! Uses `x11rb`'s default, pure-Rust `RustConnection`, which speaks the X11
//! wire protocol directly over a Unix socket. Nothing here enables x11rb's
//! `allow-unsafe-code` feature (the one that would link libxcb), so no C
//! library is ever touched.

use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask,
    ImageFormat, ImageOrder, PropMode, WindowClass,
};
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::COPY_DEPTH_FROM_PARENT;

use super::render::{Dialog, Key, Outcome};
use super::Attempt;

/// The dialog closes itself, as a denial, if nobody answers within this long.
const TIMEOUT: Duration = Duration::from_secs(300);
/// How often we poll the connection for events while waiting. The dialog is
/// a short-lived, rarely shown window, so a plain poll-and-sleep loop is
/// simpler than wiring up raw fd polling for the sake of a five-minute cap.
const POLL_INTERVAL: Duration = Duration::from_millis(30);

// X11 keysyms named in the task brief.
const KEYSYM_ESCAPE: u32 = 0xff1b;
const KEYSYM_RETURN: u32 = 0xff0d;
const KEYSYM_KP_ENTER: u32 = 0xff8d;
const KEYSYM_SPACE: u32 = 0x20;
const KEYSYM_TAB: u32 = 0xff09;
const KEYSYM_LEFT: u32 = 0xff51;
const KEYSYM_RIGHT: u32 = 0xff53;

pub(super) fn attempt(title: &str, message: &str, allow_label: &str) -> Attempt {
    match run(title, message, allow_label) {
        Ok(decision) => Attempt::Ok(decision),
        Err(reason) => Attempt::Unavailable(reason),
    }
}

/// Translates raw X11 keycodes to keysyms using the first column of
/// `GetKeyboardMapping`, which is all we need for the handful of keys this
/// dialog reacts to (none of them have a shift-dependent meaning).
struct KeyMap {
    min_keycode: u8,
    keysyms_per_keycode: u8,
    keysyms: Vec<u32>,
}

impl KeyMap {
    fn load(conn: &impl Connection) -> Result<KeyMap, String> {
        let setup = conn.setup();
        let count = setup
            .max_keycode
            .saturating_sub(setup.min_keycode)
            .saturating_add(1);
        let reply = conn
            .get_keyboard_mapping(setup.min_keycode, count)
            .map_err(|e| e.to_string())?
            .reply()
            .map_err(|e| e.to_string())?;
        Ok(KeyMap {
            min_keycode: setup.min_keycode,
            keysyms_per_keycode: reply.keysyms_per_keycode,
            keysyms: reply.keysyms,
        })
    }

    fn keysym(&self, keycode: u8) -> Option<u32> {
        if keycode < self.min_keycode || self.keysyms_per_keycode == 0 {
            return None;
        }
        let row = (keycode - self.min_keycode) as usize * self.keysyms_per_keycode as usize;
        self.keysyms.get(row).copied()
    }

    fn lookup(&self, keycode: u8) -> Option<Key> {
        match self.keysym(keycode)? {
            KEYSYM_ESCAPE => Some(Key::Escape),
            KEYSYM_RETURN | KEYSYM_KP_ENTER => Some(Key::Enter),
            KEYSYM_SPACE => Some(Key::Space),
            KEYSYM_TAB => Some(Key::Tab),
            KEYSYM_LEFT => Some(Key::Left),
            KEYSYM_RIGHT => Some(Key::Right),
            _ => None,
        }
    }
}

fn atom(conn: &impl Connection, name: &str) -> Result<u32, String> {
    Ok(conn
        .intern_atom(false, name.as_bytes())
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?
        .atom)
}

fn is_delete_window(event: &ClientMessageEvent, wm_protocols: u32, wm_delete_window: u32) -> bool {
    event.type_ == wm_protocols
        && event.format == 32
        && event.data.as_data32()[0] == wm_delete_window
}

fn run(title: &str, message: &str, allow_label: &str) -> Result<bool, String> {
    let (conn, screen_num) = x11rb::connect(None).map_err(|e| e.to_string())?;
    let screen = conn.setup().roots[screen_num].clone();

    let mut dialog = Dialog::new(title, message, allow_label, 1.0);
    let width = dialog.width();
    let height = dialog.height();

    let win_id = conn.generate_id().map_err(|e| e.to_string())?;
    let x = ((screen.width_in_pixels as i32 - width as i32) / 2).max(0) as i16;
    let y = ((screen.height_in_pixels as i32 - height as i32) / 2).max(0) as i16;

    let aux = CreateWindowAux::new()
        .background_pixel(screen.black_pixel)
        .event_mask(
            EventMask::EXPOSURE
                | EventMask::KEY_PRESS
                | EventMask::BUTTON_PRESS
                | EventMask::BUTTON_RELEASE
                | EventMask::POINTER_MOTION
                | EventMask::STRUCTURE_NOTIFY,
        );

    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        win_id,
        screen.root,
        x,
        y,
        width as u16,
        height as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &aux,
    )
    .map_err(|e| e.to_string())?;

    let wm_protocols = atom(&conn, "WM_PROTOCOLS")?;
    let wm_delete_window = atom(&conn, "WM_DELETE_WINDOW")?;
    let net_wm_window_type = atom(&conn, "_NET_WM_WINDOW_TYPE")?;
    let net_wm_window_type_dialog = atom(&conn, "_NET_WM_WINDOW_TYPE_DIALOG")?;
    let net_wm_name = atom(&conn, "_NET_WM_NAME")?;
    let utf8_string = atom(&conn, "UTF8_STRING")?;

    conn.change_property32(
        PropMode::REPLACE,
        win_id,
        wm_protocols,
        AtomEnum::ATOM,
        &[wm_delete_window],
    )
    .map_err(|e| e.to_string())?;
    conn.change_property32(
        PropMode::REPLACE,
        win_id,
        net_wm_window_type,
        AtomEnum::ATOM,
        &[net_wm_window_type_dialog],
    )
    .map_err(|e| e.to_string())?;
    conn.change_property8(
        PropMode::REPLACE,
        win_id,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        title.as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    conn.change_property8(
        PropMode::REPLACE,
        win_id,
        net_wm_name,
        utf8_string,
        title.as_bytes(),
    )
    .map_err(|e| e.to_string())?;

    let gc = conn.generate_id().map_err(|e| e.to_string())?;
    conn.create_gc(gc, win_id, &CreateGCAux::new())
        .map_err(|e| e.to_string())?;

    conn.map_window(win_id).map_err(|e| e.to_string())?;
    conn.flush().map_err(|e| e.to_string())?;

    let keymap = KeyMap::load(&conn)?;

    let mut buf = vec![0u32; (width as usize) * (height as usize)];
    dialog.paint(&mut buf, width);

    let deadline = Instant::now() + TIMEOUT;
    let mut decision: Option<bool> = None;

    while decision.is_none() {
        if Instant::now() >= deadline {
            decision = Some(false);
            break;
        }

        let event = conn.poll_for_event().map_err(|e| e.to_string())?;
        let Some(event) = event else {
            std::thread::sleep(POLL_INTERVAL);
            continue;
        };

        let outcome = match event {
            Event::Expose(e) if e.count == 0 => {
                present(&conn, win_id, gc, &buf, width, height, screen.root_depth)?;
                Outcome::Unchanged
            }
            Event::MotionNotify(e) => dialog.on_pointer_move(e.event_x as f64, e.event_y as f64),
            Event::ButtonRelease(e) => {
                dialog.on_pointer_release(e.event_x as f64, e.event_y as f64)
            }
            Event::KeyPress(e) => keymap
                .lookup(e.detail)
                .map(|k| dialog.on_key(k))
                .unwrap_or(Outcome::Unchanged),
            Event::ClientMessage(e) if is_delete_window(&e, wm_protocols, wm_delete_window) => {
                Outcome::Decided(false)
            }
            _ => Outcome::Unchanged,
        };

        match outcome {
            Outcome::Unchanged => {}
            Outcome::Repaint => {
                dialog.paint(&mut buf, width);
                present(&conn, win_id, gc, &buf, width, height, screen.root_depth)?;
            }
            Outcome::Decided(value) => decision = Some(value),
        }
        conn.flush().map_err(|e| e.to_string())?;
    }

    let _ = conn.destroy_window(win_id);
    let _ = conn.flush();

    Ok(decision.unwrap_or(false))
}

/// Sends the whole buffer to the window with `PutImage`, splitting it into
/// row bands that fit under the server's maximum request length so a large,
/// HiDPI dialog cannot overflow a single X11 request.
fn present(
    conn: &impl Connection,
    window: u32,
    gc: u32,
    buf: &[u32],
    width: u32,
    height: u32,
    depth: u8,
) -> Result<(), String> {
    let byte_order = conn.setup().image_byte_order;
    let row_bytes = width as usize * 4;
    if row_bytes == 0 {
        return Ok(());
    }

    // `maximum_request_length` is in 4-byte units and does not account for
    // any BIG-REQUESTS negotiation, which we do not use. Leave headroom for
    // the PutImage request header itself.
    let max_request_bytes = (conn.setup().maximum_request_length as usize * 4).saturating_sub(256);
    let rows_per_chunk = (max_request_bytes / row_bytes).max(1);

    let mut y = 0u32;
    while y < height {
        let rows = rows_per_chunk.min((height - y) as usize) as u32;
        let mut chunk = Vec::with_capacity(rows as usize * row_bytes);
        let start = (y as usize) * (width as usize);
        let end = start + (rows as usize) * (width as usize);
        for &pixel in &buf[start..end] {
            let bytes = if byte_order == ImageOrder::MSB_FIRST {
                pixel.to_be_bytes()
            } else {
                pixel.to_le_bytes()
            };
            chunk.extend_from_slice(&bytes);
        }
        conn.put_image(
            ImageFormat::Z_PIXMAP,
            window,
            gc,
            width as u16,
            rows as u16,
            0,
            y as i16,
            0,
            depth,
            &chunk,
        )
        .map_err(|e| e.to_string())?;
        y += rows;
    }
    Ok(())
}
