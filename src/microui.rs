//! Settings overlay — drawn with DrawOp primitives, styled like microui.
//!
//! Every coordinate is in logical space (640×480).  The RenderMetrics
//! scaling handles Retina via sx/sy.

use crate::videoio_displays::{Color, DrawOp, RenderMetrics, Renderer};

// ── microui colour palette ────────────────────────────────────────
const WIN_BG: Color = Color(50, 50, 50, 255);
const TITLE_BG: Color = Color(25, 25, 25, 255);
const TITLE_FG: Color = Color(240, 240, 240, 255);
const TEXT: Color = Color(230, 230, 230, 255);
const LABEL_DIM: Color = Color(180, 180, 190, 255);
const BTN_NORMAL: Color = Color(75, 75, 75, 255);
const BTN_HOVER: Color = Color(95, 95, 95, 255);
const BORDER: Color = Color(25, 25, 25, 255);
const BACKDROP: Color = Color(0, 0, 0, 170);

// ── scaling helpers (logical → drawable) ──────────────────────────
fn sy(m: &RenderMetrics, v: i32) -> i32 {
    m.extent(v, m.scale_y)
}
fn scale(m: &RenderMetrics) -> f32 {
    m.scale_y.max(m.scale_x)
}

/// Styled text with Retina-scaled font size.
fn txt(
    r: &mut dyn Renderer,
    text: &str,
    x: i32,
    y: i32,
    c: Color,
    cx: i8,
    cy: i8,
    m: &RenderMetrics,
) {
    r.draw(DrawOp::StyledText(
        text.into(),
        "default".into(),
        12.0 * scale(m),
        x,
        y,
        c,
        cx,
        cy,
    ));
}

// ── widgets ───────────────────────────────────────────────────────

/// Screen-filling dark overlay (dims the scene behind).
fn backdrop(r: &mut dyn Renderer, m: &RenderMetrics) {
    r.draw(DrawOp::Box(0, 0, m.x(m.logical_width), m.y(m.logical_height), BACKDROP));
}

/// Panel: dark filled rect + 1px border.
fn panel(r: &mut dyn Renderer, m: &RenderMetrics, x: i32, y: i32, w: i32, h: i32) {
    let (x1, y1, x2, y2) = (m.x(x), m.y(y), m.x(x + w), m.y(y + h));
    r.draw(DrawOp::Box(x1, y1, x2, y2, WIN_BG));
    r.draw(DrawOp::Box(x1, y1, x2, y2, BORDER));
}

/// Title-bar strip at the top of the panel.
fn title_bar(r: &mut dyn Renderer, m: &RenderMetrics, x: i32, y: i32, w: i32, label: &str) {
    let h = 24;
    let (x1, y1, x2, y2) = (m.x(x), m.y(y), m.x(x + w), m.y(y + h));
    r.draw(DrawOp::Box(x1, y1, x2, y2, TITLE_BG));
    txt(r, label, (x1 + x2) / 2, y1 + sy(m, 16), TITLE_FG, 0, -1, m);
}

/// Left-aligned label.
fn label(r: &mut dyn Renderer, m: &RenderMetrics, x: i32, y: i32, text: &str) {
    txt(r, text, m.x(x), m.y(y), LABEL_DIM, -1, -1, m);
}

/// A value string (brighter than label).
fn value(r: &mut dyn Renderer, m: &RenderMetrics, x: i32, y: i32, text: &str) {
    txt(r, text, m.x(x), m.y(y), TEXT, -1, -1, m);
}

/// Draw a microui-style button.  Returns the hit-rect (in logical coords)
/// for the caller to test mouse clicks against.
fn button(
    r: &mut dyn Renderer,
    m: &RenderMetrics,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    label_text: &str,
    hover: bool,
) -> (i32, i32, i32, i32) {
    let bg = if hover { BTN_HOVER } else { BTN_NORMAL };
    let (x1, y1, x2, y2) = (m.x(x), m.y(y), m.x(x + w), m.y(y + h));
    r.draw(DrawOp::Box(x1, y1, x2, y2, bg));
    r.draw(DrawOp::Box(x1, y1, x2, y2, BORDER));
    txt(
        r,
        label_text,
        (x1 + x2) / 2,
        (y1 + y2) / 2,
        TEXT,
        0,
        0,
        m,
    );
    (x, y, x + w, y + h)
}

// ── main overlay ──────────────────────────────────────────────────

/// Render the settings overlay.  Returns `true` when the test-beep
/// button was clicked this frame.
pub fn settings_overlay(
    r: &mut dyn Renderer,
    m: &RenderMetrics,
    stream_path: &str,
    mouse_logical: (i32, i32),
    mouse_down: bool,
    prev_mouse_down: &mut bool,
) -> bool {
    let (mx, my) = mouse_logical;
    let lw = m.logical_width;
    let lh = m.logical_height;

    // Panel geometry (logical)
    let pw = 360;
    let ph = 170;
    let px = (lw - pw) / 2;
    let py = (lh - ph) / 2;

    // Draw
    backdrop(r, m);
    panel(r, m, px, py, pw, ph);
    title_bar(r, m, px + 1, py + 1, pw - 2, "Settings");

    // Content area starts below the title bar (py + 24 + padding)
    let cy = py + 24 + 6;

    label(r, m, px + 8, cy, "Stream Output Path:");
    let path = if stream_path.is_empty() {
        "(default)"
    } else {
        stream_path
    };
    value(r, m, px + 8, cy + 18, path);

    // Test Beep button
    let btn_x = px + 8;
    let btn_y = cy + 46;
    let btn_w = 120;
    let btn_h = 28;
    let hover = mx >= btn_x && mx <= btn_x + btn_w && my >= btn_y && my <= btn_y + btn_h;
    let _ = button(r, m, btn_x, btn_y, btn_w, btn_h, "Test Beep", hover);

    // Hint
    txt(
        r,
        "Esc: close",
        m.x(px + pw - 8),
        m.y(py + ph - 4),
        Color(120, 120, 140, 255),
        1,
        1,
        m,
    );

    // Click detection — only on the release edge (mouse *was* down, now up)
    let inside = mx >= btn_x && mx <= btn_x + btn_w && my >= btn_y && my <= btn_y + btn_h;
    let clicked = inside && *prev_mouse_down && !mouse_down;
    *prev_mouse_down = mouse_down;
    clicked
}
