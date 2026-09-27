//! Wayland backend for the approval dialog.
//!
//! Built on `smithay-client-toolkit` and `wayland-client` with the pure-Rust
//! `wayland-backend` (no `client_system` feature, so `wayland-sys` never
//! dlopens libwayland-client.so). `smithay-client-toolkit` is used with
//! `default-features = false, features = ["calloop"]`: its default
//! `xkbcommon` feature is what would pull in a C library (xkbcommon,
//! located via pkg-config), so it is left off entirely. Without it, sctk's
//! `seat::keyboard` module does not even exist in the crate, so this file
//! never touches it: instead it binds the raw `wl_keyboard` object itself
//! and reads its `key` events as plain evdev keycodes.

use std::time::Duration;

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_registry,
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    reexports::calloop::{
        timer::{TimeoutAction, Timer},
        EventLoop,
    },
    reexports::calloop_wayland_source::WaylandSource,
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        xdg::{
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
            XdgShell,
        },
        WaylandSurface,
    },
    shm::{
        slot::{Buffer, SlotPool},
        Shm, ShmHandler,
    },
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle, WEnum,
};

use super::render::{Dialog, Key, Outcome};
use super::Attempt;

/// The dialog closes itself, as a denial, if nobody answers within this long.
const TIMEOUT: Duration = Duration::from_secs(300);

// Raw evdev keycodes, as sent in `wl_keyboard::Event::Key`.
const KEY_ESC: u32 = 1;
const KEY_ENTER: u32 = 28;
const KEY_KPENTER: u32 = 96;
const KEY_SPACE: u32 = 57;
const KEY_TAB: u32 = 15;
const KEY_LEFT: u32 = 105;
const KEY_RIGHT: u32 = 106;

/// The left mouse button, as reported in `wl_pointer` button events (this is
/// the Linux input event code `BTN_LEFT`).
const BTN_LEFT: u32 = 0x110;

fn evdev_key(code: u32) -> Option<Key> {
    match code {
        KEY_ESC => Some(Key::Escape),
        KEY_ENTER | KEY_KPENTER => Some(Key::Enter),
        KEY_SPACE => Some(Key::Space),
        KEY_TAB => Some(Key::Tab),
        KEY_LEFT => Some(Key::Left),
        KEY_RIGHT => Some(Key::Right),
        _ => None,
    }
}

pub(super) fn attempt(title: &str, message: &str, allow_label: &str) -> Attempt {
    match run(title, message, allow_label) {
        Ok(decision) => Attempt::Ok(decision),
        Err(reason) => Attempt::Unavailable(reason),
    }
}

struct State {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,
    window: Window,
    buffer: Option<Buffer>,
    buffer_size: (u32, u32),
    dialog: Dialog,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    decision: Option<bool>,
    needs_redraw: bool,
}

impl State {
    fn apply_outcome(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Unchanged => {}
            Outcome::Repaint => self.needs_redraw = true,
            Outcome::Decided(value) => {
                self.decision = Some(value);
            }
        }
    }

    /// Paints the dialog and presents it, (re)creating the shm buffer if the
    /// dialog's pixel size changed (a scale factor update, most likely).
    fn draw(&mut self) {
        let width = self.dialog.width();
        let height = self.dialog.height();
        let stride = width as i32 * 4;

        if self.buffer_size != (width, height) {
            self.buffer = None;
            self.buffer_size = (width, height);
        }

        let pool = &mut self.pool;
        let buffer = self.buffer.get_or_insert_with(|| {
            pool.create_buffer(
                width as i32,
                height as i32,
                stride,
                wl_shm::Format::Argb8888,
            )
            .expect("create wayland shm buffer")
            .0
        });

        let canvas = match pool.canvas(buffer) {
            Some(canvas) => canvas,
            None => {
                let (second, canvas) = pool
                    .create_buffer(
                        width as i32,
                        height as i32,
                        stride,
                        wl_shm::Format::Argb8888,
                    )
                    .expect("create wayland shm buffer");
                *buffer = second;
                canvas
            }
        };

        let mut pixels = vec![0u32; (width * height) as usize];
        self.dialog.paint(&mut pixels, width);
        for (chunk, pixel) in canvas.as_chunks_mut::<4>().0.iter_mut().zip(pixels.iter()) {
            chunk.copy_from_slice(&pixel.to_le_bytes());
        }

        let surface = self.window.wl_surface();
        surface.damage_buffer(0, 0, width as i32, height as i32);
        buffer.attach_to(surface).expect("attach wayland buffer");
        surface.commit();
        self.needs_redraw = false;
    }
}

fn run(title: &str, message: &str, allow_label: &str) -> Result<bool, String> {
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, event_queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
    let qh = event_queue.handle();

    let mut event_loop: EventLoop<State> = EventLoop::try_new().map_err(|e| e.to_string())?;
    let loop_handle = event_loop.handle();
    WaylandSource::new(conn.clone(), event_queue)
        .insert(loop_handle.clone())
        .map_err(|e| e.to_string())?;

    let compositor = CompositorState::bind(&globals, &qh).map_err(|e| e.to_string())?;
    let xdg_shell = XdgShell::bind(&globals, &qh).map_err(|e| e.to_string())?;
    let shm = Shm::bind(&globals, &qh).map_err(|e| e.to_string())?;

    let surface = compositor.create_surface(&qh);
    // `RequestServer`: ask the compositor to draw the decoration if it
    // supports xdg-decoration. If it does not, the compositor falls back to
    // client-side decorations (or none); the dialog's own title text stands
    // in either way, so nothing extra is drawn for that case.
    let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, &qh);
    window.set_title(title);
    window.set_app_id("org.librepaper.ApprovalDialog");
    window.commit();

    let dialog = Dialog::new(title, message, allow_label, 1.0);
    let pool_size = ((dialog.width() * dialog.height() * 4) as usize).max(4096);
    let pool = SlotPool::new(pool_size, &shm).map_err(|e| e.to_string())?;

    let mut state = State {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        shm,
        pool,
        window,
        buffer: None,
        buffer_size: (0, 0),
        dialog,
        keyboard: None,
        pointer: None,
        decision: None,
        needs_redraw: false,
    };

    loop_handle
        .insert_source(
            Timer::from_duration(TIMEOUT),
            |_deadline, _meta, state: &mut State| {
                state.decision.get_or_insert(false);
                TimeoutAction::Drop
            },
        )
        .map_err(|e| e.to_string())?;

    while state.decision.is_none() {
        event_loop
            .dispatch(Duration::from_millis(200), &mut state)
            .map_err(|e| e.to_string())?;
        if state.needs_redraw {
            state.draw();
        }
    }

    Ok(state.decision.unwrap_or(false))
}

/// The `wl_keyboard` user data. `smithay-client-toolkit`'s own keyboard
/// module needs the `xkbcommon` feature (a C library located through
/// pkg-config), which is off, so this crate is the object's only handler:
/// implementing [`Dispatch2`] slots it into sctk's blanket
/// `delegate_dispatch2!` dispatch instead of needing a hand-rolled
/// `wayland_client::Dispatch` impl (which would conflict with that blanket
/// impl).
struct RawKeyboard;

impl Dispatch2<wl_keyboard::WlKeyboard, State> for RawKeyboard {
    fn event(
        &self,
        state: &mut State,
        _proxy: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _conn: &Connection,
        _qh: &QueueHandle<State>,
    ) {
        if let wl_keyboard::Event::Key {
            key,
            state: key_state,
            ..
        } = event
        {
            if key_state == WEnum::Value(wl_keyboard::KeyState::Pressed) {
                if let Some(mapped) = evdev_key(key) {
                    let outcome = state.dialog.on_key(mapped);
                    state.apply_outcome(outcome);
                }
            }
        }
    }
}

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        if surface == self.window.wl_surface() {
            self.dialog.set_scale(new_factor.max(1) as f32);
            surface.set_buffer_scale(new_factor.max(1));
            self.buffer = None;
            self.needs_redraw = true;
        }
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        // The dialog only repaints in response to input, not a frame clock.
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
    }
}

impl WindowHandler for State {
    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, window: &Window) {
        if window == &self.window {
            self.decision.get_or_insert(false);
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        window: &Window,
        _configure: WindowConfigure,
        _serial: u32,
    ) {
        if window == &self.window {
            self.needs_redraw = true;
        }
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = Some(seat.get_keyboard(qh, RawKeyboard));
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            if let Ok(pointer) = self.seat_state.get_pointer(qh, &seat) {
                self.pointer = Some(pointer);
            }
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            if let Some(keyboard) = self.keyboard.take() {
                keyboard.release();
            }
        }
        if capability == Capability::Pointer {
            if let Some(pointer) = self.pointer.take() {
                pointer.release();
            }
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {
    }
}

impl PointerHandler for State {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        // Wayland reports pointer positions in logical surface coordinates,
        // while the dialog hit-tests physical buffer pixels.
        let scale = self.dialog.scale() as f64;
        for event in events {
            if &event.surface != self.window.wl_surface() {
                continue;
            }
            let (x, y) = (event.position.0 * scale, event.position.1 * scale);
            let outcome = match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    self.dialog.on_pointer_move(x, y)
                }
                PointerEventKind::Leave { .. } => self.dialog.on_pointer_leave(),
                PointerEventKind::Release { button, .. } if button == BTN_LEFT => {
                    self.dialog.on_pointer_release(x, y)
                }
                _ => Outcome::Unchanged,
            };
            self.apply_outcome(outcome);
        }
    }
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_registry!(State);

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(State);
