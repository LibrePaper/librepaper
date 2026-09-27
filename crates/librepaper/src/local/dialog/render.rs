//! Backend-independent layout and drawing for the approval dialog.
//!
//! Nothing here knows about Wayland or X11: this module lays the dialog out
//! at a given scale factor, paints it into an ARGB8888 buffer, and answers
//! hit-testing and keyboard-focus questions. Each windowing backend only has
//! to turn its own raw input events into the small [`Key`] enum and forward
//! pointer coordinates; everything else (wrapping, colours, focus rings,
//! which button a click or a keystroke lands on) lives here so it can be
//! unit tested without a display server.

use ab_glyph::{Font, FontRef, GlyphId, PxScale, ScaleFont};

/// The two buttons the dialog can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Button {
    Deny,
    Allow,
}

/// A key, already translated by the backend from its own raw representation
/// (evdev keycodes on Wayland, keysyms on X11) into this display-independent
/// form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Escape,
    Enter,
    Space,
    Tab,
    Left,
    Right,
}

/// What a backend should do after feeding an input event to the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Nothing visible changed.
    Unchanged,
    /// Focus or hover moved: repaint and re-present the buffer.
    Repaint,
    /// The user made a decision. The window should close.
    Decided(bool),
}

const LOGICAL_WIDTH: f32 = 460.0;
const PADDING: f32 = 24.0;
const TITLE_SIZE: f32 = 17.0;
const TEXT_SIZE: f32 = 14.0;
const LINE_SPACING: f32 = 6.0;
const TITLE_GAP: f32 = 14.0;
const BUTTON_GAP: f32 = 12.0;
const BUTTON_HEIGHT: f32 = 36.0;
const BUTTON_TOP_GAP: f32 = 22.0;
const BUTTON_RADIUS: f32 = 6.0;
const BUTTON_PAD_X: f32 = 18.0;
const BUTTON_MIN_WIDTH: f32 = 84.0;
const FOCUS_RING_PAD: f32 = 3.0;

const BG: (u8, u8, u8) = (0x2b, 0x2e, 0x33);
const TITLE_COLOR: (u8, u8, u8) = (0xf3, 0xf4, 0xf6);
const TEXT_COLOR: (u8, u8, u8) = (0xc9, 0xcd, 0xd3);
const ACCENT: (u8, u8, u8) = (0x3d, 0x7e, 0xf7);
const ACCENT_HOVER: (u8, u8, u8) = (0x5a, 0x92, 0xf8);
const ACCENT_TEXT: (u8, u8, u8) = (0xff, 0xff, 0xff);
const DENY_FILL: (u8, u8, u8) = (0x2b, 0x2e, 0x33);
const DENY_FILL_HOVER: (u8, u8, u8) = (0x3a, 0x3e, 0x45);
const DENY_BORDER: (u8, u8, u8) = (0x6b, 0x70, 0x78);
const DENY_TEXT: (u8, u8, u8) = (0xe6, 0xe8, 0xec);
const FOCUS_RING: (u8, u8, u8) = (0x9c, 0xc4, 0xff);

/// Loads the bundled Ubuntu Light face. There is no bold weight bundled, so
/// a "bold" title is drawn by rendering the outline twice with a one pixel
/// offset (a standard faux-bold trick), see [`draw_text`].
fn font() -> FontRef<'static> {
    FontRef::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT)
        .expect("bundled font bytes are a valid TrueType font")
}

/// Greedy word wrap: splits `text` on ASCII spaces (and existing newlines)
/// and packs words onto lines no wider than `max_width` logical pixels at
/// `size` px. A pure function so it is easy to unit test.
pub(crate) fn wrap_text(font: &FontRef<'_>, size: f32, max_width: f32, text: &str) -> Vec<String> {
    let scale = font.pt_to_px_scale(size).unwrap_or(PxScale::from(size));
    let scaled = font.as_scaled(scale);
    let width_of = |s: &str| -> f32 {
        let mut width = 0.0;
        let mut prev: Option<GlyphId> = None;
        for ch in s.chars() {
            let id = scaled.glyph_id(ch);
            if let Some(p) = prev {
                width += scaled.kern(p, id);
            }
            width += scaled.h_advance(id);
            prev = Some(id);
        }
        width
    };

    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        for word in paragraph.split(' ') {
            if word.is_empty() {
                continue;
            }
            let candidate = if current.is_empty() {
                word.to_string()
            } else {
                format!("{current} {word}")
            };
            if current.is_empty() || width_of(&candidate) <= max_width {
                current = candidate;
            } else {
                lines.push(current);
                current = word.to_string();
            }
        }
        lines.push(current);
    }
    lines
}

#[derive(Debug, Clone, Copy)]
struct LogicalRect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl LogicalRect {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// A fully laid out dialog: wrapped text, button geometry, and the current
/// focus/hover state. Backends own one of these, feed it input events, and
/// ask it to paint into their pixel buffer.
pub(crate) struct Dialog {
    title: String,
    lines: Vec<String>,
    allow_label: String,
    scale: f32,
    logical_height: f32,
    title_baseline: f32,
    line_baselines: Vec<f32>,
    deny_rect: LogicalRect,
    allow_rect: LogicalRect,
    focus: Button,
    hover: Option<Button>,
}

impl Dialog {
    pub(crate) fn new(title: &str, message: &str, allow_label: &str, scale: f32) -> Dialog {
        let font = font();
        let content_x = PADDING;
        let content_width = LOGICAL_WIDTH - 2.0 * PADDING;

        let title_scaled = font.as_scaled(PxScale::from(TITLE_SIZE));
        let title_ascent = title_scaled.ascent();
        let title_descent = title_scaled.descent();
        let title_baseline = PADDING + title_ascent;
        let title_bottom = PADDING + title_ascent - title_descent;

        let lines = wrap_text(&font, TEXT_SIZE, content_width, message);
        let text_scaled = font.as_scaled(PxScale::from(TEXT_SIZE));
        let text_ascent = text_scaled.ascent();
        let text_descent = text_scaled.descent();
        let text_line_gap = text_scaled.line_gap();
        let line_height = text_ascent - text_descent + text_line_gap + LINE_SPACING;

        let mut line_baselines = Vec::with_capacity(lines.len());
        let mut cursor_y = title_bottom + TITLE_GAP + text_ascent;
        for _ in &lines {
            line_baselines.push(cursor_y);
            cursor_y += line_height;
        }
        let content_bottom = if lines.is_empty() {
            title_bottom
        } else {
            cursor_y - line_height + (-text_descent)
        };

        let buttons_y = content_bottom + BUTTON_TOP_GAP;

        let measure = |s: &str| -> f32 {
            let scaled = font.as_scaled(PxScale::from(TEXT_SIZE));
            let mut width = 0.0;
            let mut prev: Option<GlyphId> = None;
            for ch in s.chars() {
                let id = scaled.glyph_id(ch);
                if let Some(p) = prev {
                    width += scaled.kern(p, id);
                }
                width += scaled.h_advance(id);
                prev = Some(id);
            }
            width
        };
        let deny_w = (measure("Deny") + 2.0 * BUTTON_PAD_X).max(BUTTON_MIN_WIDTH);
        let allow_w = (measure(allow_label) + 2.0 * BUTTON_PAD_X).max(BUTTON_MIN_WIDTH);
        let buttons_total = deny_w + BUTTON_GAP + allow_w;
        let buttons_start_x = content_x + content_width - buttons_total;

        let deny_rect = LogicalRect {
            x: buttons_start_x,
            y: buttons_y,
            w: deny_w,
            h: BUTTON_HEIGHT,
        };
        let allow_rect = LogicalRect {
            x: buttons_start_x + deny_w + BUTTON_GAP,
            y: buttons_y,
            w: allow_w,
            h: BUTTON_HEIGHT,
        };

        let logical_height = buttons_y + BUTTON_HEIGHT + PADDING;

        Dialog {
            title: title.to_string(),
            lines,
            allow_label: allow_label.to_string(),
            scale,
            logical_height,
            title_baseline,
            line_baselines,
            deny_rect,
            allow_rect,
            focus: Button::Deny,
            hover: None,
        }
    }

    /// Rebuilds the layout at a new scale factor (a Wayland output change,
    /// for example). The wrapped text and button sizes are recomputed since
    /// they are measured in logical pixels and do not depend on scale, but
    /// callers should re-fetch [`Dialog::width`] / [`Dialog::height`] and
    /// resize their surface's buffer afterwards.
    pub(crate) fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
    }

    pub(crate) fn width(&self) -> u32 {
        (LOGICAL_WIDTH * self.scale).round().max(1.0) as u32
    }

    pub(crate) fn height(&self) -> u32 {
        (self.logical_height * self.scale).round().max(1.0) as u32
    }

    /// Hit-tests a point given in physical pixel coordinates.
    pub(crate) fn hit(&self, x: f64, y: f64) -> Option<Button> {
        let lx = (x as f32) / self.scale;
        let ly = (y as f32) / self.scale;
        if self.deny_rect.contains(lx, ly) {
            Some(Button::Deny)
        } else if self.allow_rect.contains(lx, ly) {
            Some(Button::Allow)
        } else {
            None
        }
    }

    pub(crate) fn on_pointer_move(&mut self, x: f64, y: f64) -> Outcome {
        let hit = self.hit(x, y);
        if hit == self.hover {
            Outcome::Unchanged
        } else {
            self.hover = hit;
            Outcome::Repaint
        }
    }

    pub(crate) fn on_pointer_leave(&mut self) -> Outcome {
        if self.hover.is_none() {
            Outcome::Unchanged
        } else {
            self.hover = None;
            Outcome::Repaint
        }
    }

    pub(crate) fn on_pointer_release(&mut self, x: f64, y: f64) -> Outcome {
        match self.hit(x, y) {
            Some(button) => Outcome::Decided(button == Button::Allow),
            None => Outcome::Unchanged,
        }
    }

    pub(crate) fn on_key(&mut self, key: Key) -> Outcome {
        match key {
            Key::Escape => Outcome::Decided(false),
            Key::Enter | Key::Space => Outcome::Decided(self.focus == Button::Allow),
            Key::Tab | Key::Left | Key::Right => {
                self.focus = match self.focus {
                    Button::Deny => Button::Allow,
                    Button::Allow => Button::Deny,
                };
                Outcome::Repaint
            }
        }
    }

    /// Paints the dialog into `buf`, an ARGB8888 buffer (native-endian
    /// `0xAARRGGBB` per pixel, i.e. what Wayland's `Argb8888` shm format and
    /// X11's 32 bit ZPixmap both expect) with the given row `stride` in
    /// pixels. `buf` must hold at least `stride * height()` pixels.
    pub(crate) fn paint(&self, buf: &mut [u32], stride: u32) {
        let width_px = self.width();
        let height_px = self.height();
        if stride == 0 || buf.len() < (stride as usize) * (height_px as usize) {
            return;
        }

        let mut pixmap = tiny_skia::Pixmap::new(width_px.max(1), height_px.max(1))
            .expect("dialog dimensions are non-zero");

        // Background.
        fill_rect(
            &mut pixmap,
            0.0,
            0.0,
            width_px as f32,
            height_px as f32,
            0.0,
            BG,
        );

        // Deny button: outlined, filled only on hover.
        let deny_fill = if self.hover == Some(Button::Deny) {
            DENY_FILL_HOVER
        } else {
            DENY_FILL
        };
        fill_rounded_rect(
            &mut pixmap,
            &self.deny_rect,
            self.scale,
            BUTTON_RADIUS,
            deny_fill,
        );
        stroke_rounded_rect(
            &mut pixmap,
            &self.deny_rect,
            self.scale,
            BUTTON_RADIUS,
            DENY_BORDER,
            1.0,
        );
        if self.focus == Button::Deny {
            stroke_rounded_rect(
                &mut pixmap,
                &self.deny_rect,
                self.scale,
                BUTTON_RADIUS + FOCUS_RING_PAD,
                FOCUS_RING,
                2.0,
            );
        }

        // Allow button: filled with the accent colour.
        let allow_fill = if self.hover == Some(Button::Allow) {
            ACCENT_HOVER
        } else {
            ACCENT
        };
        fill_rounded_rect(
            &mut pixmap,
            &self.allow_rect,
            self.scale,
            BUTTON_RADIUS,
            allow_fill,
        );
        if self.focus == Button::Allow {
            stroke_rounded_rect(
                &mut pixmap,
                &self.allow_rect,
                self.scale,
                BUTTON_RADIUS + FOCUS_RING_PAD,
                FOCUS_RING,
                2.0,
            );
        }

        convert_pixmap(&pixmap, buf, stride);

        let font = font();
        draw_text(
            buf,
            stride,
            height_px,
            &font,
            TITLE_SIZE * self.scale,
            PADDING * self.scale,
            self.title_baseline * self.scale,
            &self.title,
            TITLE_COLOR,
            true,
        );
        for (line, baseline) in self.lines.iter().zip(self.line_baselines.iter()) {
            draw_text(
                buf,
                stride,
                height_px,
                &font,
                TEXT_SIZE * self.scale,
                PADDING * self.scale,
                baseline * self.scale,
                line,
                TEXT_COLOR,
                false,
            );
        }

        let deny_text_color = DENY_TEXT;
        draw_centered_label(
            buf,
            stride,
            height_px,
            &font,
            TEXT_SIZE * self.scale,
            &self.deny_rect,
            self.scale,
            "Deny",
            deny_text_color,
        );
        draw_centered_label(
            buf,
            stride,
            height_px,
            &font,
            TEXT_SIZE * self.scale,
            &self.allow_rect,
            self.scale,
            &self.allow_label,
            ACCENT_TEXT,
        );
    }
}

fn rounded_rect_path(x: f32, y: f32, w: f32, h: f32, radius: f32) -> tiny_skia::Path {
    let r = radius.min(w / 2.0).min(h / 2.0).max(0.0);
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish().expect("rounded rect path is well formed")
}

fn fill_rect(
    pixmap: &mut tiny_skia::Pixmap,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    radius: f32,
    color: (u8, u8, u8),
) {
    let path = if radius > 0.0 {
        rounded_rect_path(x, y, w, h, radius)
    } else {
        tiny_skia::PathBuilder::from_rect(
            tiny_skia::Rect::from_xywh(x, y, w, h).expect("dialog background has positive size"),
        )
    };
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(color.0, color.1, color.2, 0xff);
    paint.anti_alias = true;
    pixmap.fill_path(
        &path,
        &paint,
        tiny_skia::FillRule::Winding,
        tiny_skia::Transform::identity(),
        None,
    );
}

fn fill_rounded_rect(
    pixmap: &mut tiny_skia::Pixmap,
    rect: &LogicalRect,
    scale: f32,
    radius: f32,
    color: (u8, u8, u8),
) {
    fill_rect(
        pixmap,
        rect.x * scale,
        rect.y * scale,
        rect.w * scale,
        rect.h * scale,
        radius * scale,
        color,
    );
}

fn stroke_rounded_rect(
    pixmap: &mut tiny_skia::Pixmap,
    rect: &LogicalRect,
    scale: f32,
    radius: f32,
    color: (u8, u8, u8),
    width: f32,
) {
    let pad = width / 2.0;
    let path = rounded_rect_path(
        rect.x * scale + pad,
        rect.y * scale + pad,
        rect.w * scale - width,
        rect.h * scale - width,
        (radius * scale - pad).max(0.0),
    );
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(color.0, color.1, color.2, 0xff);
    paint.anti_alias = true;
    let stroke = tiny_skia::Stroke {
        width,
        ..Default::default()
    };
    pixmap.stroke_path(
        &path,
        &paint,
        &stroke,
        tiny_skia::Transform::identity(),
        None,
    );
}

/// Converts a fully opaque tiny-skia pixmap (RGBA8, premultiplied, but alpha
/// is always 0xff here) into the caller's ARGB8888 pixel buffer.
fn convert_pixmap(pixmap: &tiny_skia::Pixmap, buf: &mut [u32], stride: u32) {
    let width = pixmap.width();
    let height = pixmap.height();
    let data = pixmap.data();
    for y in 0..height {
        let row_start = (y * width * 4) as usize;
        for x in 0..width {
            let i = row_start + (x * 4) as usize;
            let r = data[i];
            let g = data[i + 1];
            let b = data[i + 2];
            let a = data[i + 3];
            let out = (y * stride + x) as usize;
            if out < buf.len() {
                buf[out] = ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
            }
        }
    }
}

fn blend_pixel(pixel: &mut u32, color: (u8, u8, u8), coverage: f32) {
    let coverage = coverage.clamp(0.0, 1.0);
    if coverage <= 0.0 {
        return;
    }
    let existing = *pixel;
    let er = ((existing >> 16) & 0xff) as f32;
    let eg = ((existing >> 8) & 0xff) as f32;
    let eb = (existing & 0xff) as f32;
    let r = color.0 as f32 * coverage + er * (1.0 - coverage);
    let g = color.1 as f32 * coverage + eg * (1.0 - coverage);
    let b = color.2 as f32 * coverage + eb * (1.0 - coverage);
    *pixel =
        0xff00_0000 | ((r.round() as u32) << 16) | ((g.round() as u32) << 8) | (b.round() as u32);
}

/// Draws `text` starting at `origin_x` with its baseline at `baseline_y`
/// (both in physical pixels), blending glyph coverage onto whatever is
/// already in `buf`. When `bold` is set the outline is drawn twice, offset
/// by one physical pixel, since the bundled font has no bold weight.
#[allow(clippy::too_many_arguments)]
fn draw_text(
    buf: &mut [u32],
    stride: u32,
    buf_height: u32,
    font: &FontRef<'_>,
    px_size: f32,
    origin_x: f32,
    baseline_y: f32,
    text: &str,
    color: (u8, u8, u8),
    bold: bool,
) {
    let scale = PxScale::from(px_size);
    let scaled = font.as_scaled(scale);
    let mut cursor = origin_x;
    let mut prev: Option<GlyphId> = None;
    let offsets: &[f32] = if bold { &[0.0, 1.0] } else { &[0.0] };
    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(p) = prev {
            cursor += scaled.kern(p, id);
        }
        let glyph = id.with_scale_and_position(scale, ab_glyph::point(cursor, baseline_y));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            for &dx in offsets {
                outlined.draw(|gx, gy, coverage| {
                    let px = bounds.min.x as i32 + gx as i32 + dx as i32;
                    let py = bounds.min.y as i32 + gy as i32;
                    if px < 0 || py < 0 {
                        return;
                    }
                    let (px, py) = (px as u32, py as u32);
                    if px >= stride || py >= buf_height {
                        return;
                    }
                    let idx = (py * stride + px) as usize;
                    if let Some(pixel) = buf.get_mut(idx) {
                        blend_pixel(pixel, color, coverage);
                    }
                });
            }
        }
        cursor += scaled.h_advance(id);
        prev = Some(id);
    }
}

fn text_width(font: &FontRef<'_>, px_size: f32, text: &str) -> f32 {
    let scaled = font.as_scaled(PxScale::from(px_size));
    let mut width = 0.0;
    let mut prev: Option<GlyphId> = None;
    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(p) = prev {
            width += scaled.kern(p, id);
        }
        width += scaled.h_advance(id);
        prev = Some(id);
    }
    width
}

#[allow(clippy::too_many_arguments)]
fn draw_centered_label(
    buf: &mut [u32],
    stride: u32,
    buf_height: u32,
    font: &FontRef<'_>,
    px_size: f32,
    rect: &LogicalRect,
    scale: f32,
    text: &str,
    color: (u8, u8, u8),
) {
    let scaled = font.as_scaled(PxScale::from(px_size));
    let ascent = scaled.ascent();
    let descent = scaled.descent();
    let w = text_width(font, px_size, text);
    let center_x = (rect.x + rect.w / 2.0) * scale - w / 2.0;
    let center_y = (rect.y + rect.h / 2.0) * scale + (ascent + descent) / 2.0;
    draw_text(
        buf, stride, buf_height, font, px_size, center_x, center_y, text, color, false,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_text_splits_long_message() {
        let font = font();
        let lines = wrap_text(
            &font,
            14.0,
            200.0,
            "one two three four five six seven eight",
        );
        assert!(
            lines.len() > 1,
            "expected the message to wrap onto multiple lines"
        );
        for line in &lines {
            assert!(!line.is_empty());
        }
    }

    #[test]
    fn wrap_text_keeps_a_single_short_line_together() {
        let font = font();
        let lines = wrap_text(&font, 14.0, 4000.0, "a short message");
        assert_eq!(lines, vec!["a short message".to_string()]);
    }

    #[test]
    fn wrap_text_preserves_explicit_newlines() {
        let font = font();
        let lines = wrap_text(&font, 14.0, 4000.0, "first\nsecond");
        assert_eq!(lines, vec!["first".to_string(), "second".to_string()]);
    }

    #[test]
    fn hit_testing_finds_the_right_button() {
        let dialog = Dialog::new("Title", "A short message.", "Allow", 1.0);
        let deny_center = (
            (dialog.deny_rect.x + dialog.deny_rect.w / 2.0) as f64,
            (dialog.deny_rect.y + dialog.deny_rect.h / 2.0) as f64,
        );
        let allow_center = (
            (dialog.allow_rect.x + dialog.allow_rect.w / 2.0) as f64,
            (dialog.allow_rect.y + dialog.allow_rect.h / 2.0) as f64,
        );
        assert_eq!(dialog.hit(deny_center.0, deny_center.1), Some(Button::Deny));
        assert_eq!(
            dialog.hit(allow_center.0, allow_center.1),
            Some(Button::Allow)
        );
        assert_eq!(dialog.hit(-10.0, -10.0), None);
    }

    #[test]
    fn hit_testing_scales_with_the_buffer_scale() {
        let dialog = Dialog::new("Title", "A short message.", "Allow", 2.0);
        let physical_x = (dialog.allow_rect.x + dialog.allow_rect.w / 2.0) * 2.0;
        let physical_y = (dialog.allow_rect.y + dialog.allow_rect.h / 2.0) * 2.0;
        assert_eq!(
            dialog.hit(physical_x as f64, physical_y as f64),
            Some(Button::Allow)
        );
    }

    #[test]
    fn escape_always_denies() {
        let mut dialog = Dialog::new("Title", "Message.", "Allow", 1.0);
        dialog.focus = Button::Allow;
        assert_eq!(dialog.on_key(Key::Escape), Outcome::Decided(false));
    }

    #[test]
    fn enter_decides_by_focus() {
        let mut dialog = Dialog::new("Title", "Message.", "Allow", 1.0);
        assert_eq!(dialog.focus, Button::Deny);
        assert_eq!(dialog.on_key(Key::Enter), Outcome::Decided(false));
        dialog.on_key(Key::Tab);
        assert_eq!(dialog.focus, Button::Allow);
        assert_eq!(dialog.on_key(Key::Enter), Outcome::Decided(true));
    }

    #[test]
    fn tab_toggles_focus_and_reports_repaint() {
        let mut dialog = Dialog::new("Title", "Message.", "Allow", 1.0);
        assert_eq!(dialog.on_key(Key::Tab), Outcome::Repaint);
        assert_eq!(dialog.focus, Button::Allow);
        assert_eq!(dialog.on_key(Key::Left), Outcome::Repaint);
        assert_eq!(dialog.focus, Button::Deny);
    }

    #[test]
    fn pointer_move_reports_unchanged_when_hover_does_not_move() {
        let mut dialog = Dialog::new("Title", "Message.", "Allow", 1.0);
        assert_eq!(dialog.on_pointer_move(-100.0, -100.0), Outcome::Unchanged);
    }

    #[test]
    fn pointer_release_on_allow_decides_true() {
        let mut dialog = Dialog::new("Title", "Message.", "Allow", 1.0);
        let x = (dialog.allow_rect.x + dialog.allow_rect.w / 2.0) as f64;
        let y = (dialog.allow_rect.y + dialog.allow_rect.h / 2.0) as f64;
        assert_eq!(dialog.on_pointer_release(x, y), Outcome::Decided(true));
    }

    #[test]
    fn paint_fills_the_whole_buffer_opaquely() {
        let dialog = Dialog::new("Approve this tool", "This is a somewhat long message that should wrap onto more than one line to exercise layout.", "Allow", 1.0);
        let stride = dialog.width();
        let mut buf = vec![0u32; (stride * dialog.height()) as usize];
        dialog.paint(&mut buf, stride);
        assert!(
            buf.iter().all(|p| (p >> 24) & 0xff == 0xff),
            "every pixel should be fully opaque"
        );
    }
}
