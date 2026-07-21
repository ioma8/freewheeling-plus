//! The display widgets which sit above the primitive displays.
//!
//! This module intentionally contains no SDL code.  It keeps the state and
//! geometry of the legacy widgets and emits the same logical drawing
//! operations through `videoio_displays::Renderer`.

use super::browser::Browser;
use super::event::Event;
use super::videoio_displays::{Color, Display, DrawOp, FloDisplay, RenderMetrics, Renderer, Value};
use crate::core_dsp::AudioLevel;

pub struct FloDisplayCircleSwitch<V: Value> {
    pub base: FloDisplay,
    pub exp: V,
    pub rad1: i32,
    pub rad0: i32,
    pub flash: bool,
    pub prev_nonzero: bool,
    pub nonzero_time: f64,
    /// Scene time supplied by the owner, corresponding to C++ `mygettime()`.
    pub now: f64,
}
impl<V: Value> Display for FloDisplayCircleSwitch<V> {
    fn base(&self) -> &FloDisplay {
        &self.base
    }
    fn base_mut(&mut self) -> &mut FloDisplay {
        &mut self.base
    }
    fn render(&mut self, r: &mut dyn Renderer, m: &RenderMetrics) {
        if !self.base.show {
            return;
        }
        // Preserve C++ ordering exactly: flash phase is sampled before this
        // frame detects a rising edge and stores its origin time.
        let flash_on =
            !self.flash || ((self.now - self.nonzero_time) * 4.0).floor() as i64 % 2 == 0;
        let on = self.exp.value() != 0.0;
        if on && !self.prev_nonzero {
            self.nonzero_time = self.now;
        }
        self.prev_nonzero = on;
        let lit = on && flash_on;
        let radius = if lit { self.rad1 } else { self.rad0 };
        let color = if lit {
            Color(0xdf, 0x20, 0x20, 255)
        } else {
            Color(0x11, 0x22, 0x33, 255)
        };
        // C++ uses `filledCircleRGBA`, not an outline.  Its flash cadence is
        // handled by the production scene clock; this generic widget retains
        // the same selected radius and fill primitive.
        r.draw(DrawOp::FilledCircle(
            m.x(self.base.xpos),
            m.y(self.base.ypos),
            m.x(radius),
            color,
        ));
        if let Some(t) = &self.base.title {
            r.draw(DrawOp::Text(
                t.clone(),
                m.x(self.base.xpos + 2 * self.rad0),
                m.y(self.base.ypos),
                Color(0x77, 0x88, 0x99, 255),
                0,
                1,
            ));
        }
    }
}

pub struct FloDisplayTextSwitch<V: Value> {
    pub base: FloDisplay,
    pub exp: V,
    pub text1: Option<String>,
    pub text0: Option<String>,
}
impl<V: Value> Display for FloDisplayTextSwitch<V> {
    fn base(&self) -> &FloDisplay {
        &self.base
    }
    fn base_mut(&mut self) -> &mut FloDisplay {
        &mut self.base
    }
    fn render(&mut self, r: &mut dyn Renderer, m: &RenderMetrics) {
        if !self.base.show {
            return;
        }
        let on = self.exp.value() != 0.0;
        if let Some(t) = if on {
            self.text1.as_ref()
        } else {
            self.text0.as_ref()
        } {
            r.draw(DrawOp::Text(
                t.clone(),
                m.x(self.base.xpos),
                m.y(self.base.ypos),
                if on {
                    Color(0x77, 0x88, 0x99, 255)
                } else {
                    Color(0x99, 0x88, 0x77, 255)
                },
                0,
                1,
            ));
        }
    }
}

pub struct FloDisplayBarSwitch<V: Value, S: Value> {
    pub base: FloDisplay,
    pub exp: V,
    pub switchexp: S,
    pub orientation: super::videoio_displays::Orientation,
    pub barscale: f32,
    pub thickness: i32,
    pub dbscale: bool,
    pub marks: bool,
    pub maxdb: f32,
    pub color: u8,
    pub calibrate: bool,
    pub cval: f32,
}
impl<V: Value, S: Value> Display for FloDisplayBarSwitch<V, S> {
    fn base(&self) -> &FloDisplay {
        &self.base
    }
    fn base_mut(&mut self) -> &mut FloDisplay {
        &mut self.base
    }
    fn render(&mut self, r: &mut dyn Renderer, m: &RenderMetrics) {
        if !self.base.show {
            return;
        }
        let v = self.exp.value();
        let level = if self.dbscale {
            let db = if v > 0.0 { 20.0 * v.log10() } else { -1000.0 };
            AudioLevel::db_to_fader(db, self.maxdb)
        } else {
            v
        };
        let scale = self.barscale
            * if self.orientation == super::videoio_displays::Orientation::Horizontal {
                m.scale_x
            } else {
                m.scale_y
            };
        let l = (level * scale) as i32;
        let t = m.extent(
            self.thickness,
            if self.orientation == super::videoio_displays::Orientation::Horizontal {
                m.scale_y
            } else {
                m.scale_x
            },
        );
        let a = if self.switchexp.value() != 0.0 {
            255
        } else {
            127
        };
        let c = if self.calibrate && v >= self.cval {
            Color(255, 0, 0, a)
        } else if self.color == 2 {
            Color(0xcf, 0x4f, 0xfc, a)
        } else {
            Color(0xef, 0xaf, 0xff, a)
        };
        let (x, y) = (m.x(self.base.xpos), m.y(self.base.ypos));
        if self.dbscale && self.marks {
            let mut db = -60.0;
            let mut shade = 0_i32;
            let step = (255.0 / ((self.maxdb + 60.0) / 6.0)) as i32;
            while db <= self.maxdb {
                let mark = (AudioLevel::db_to_fader(db, self.maxdb) * scale) as i32;
                let mark_color = Color(
                    shade.clamp(0, 255) as u8,
                    shade.clamp(0, 255) as u8,
                    shade.clamp(0, 255) as u8,
                    a,
                );
                match self.orientation {
                    super::videoio_displays::Orientation::Vertical => {
                        r.draw(DrawOp::Line(
                            (x - (t * 11) / 20, y - mark),
                            (x - t / 2, y - mark),
                            mark_color,
                        ));
                        r.draw(DrawOp::Line(
                            (x + t / 2, y - mark),
                            (x + (t * 11) / 20, y - mark),
                            mark_color,
                        ));
                    }
                    super::videoio_displays::Orientation::Horizontal => {
                        r.draw(DrawOp::Line(
                            (x + mark, y - (t * 11) / 20),
                            (x + mark, y - t / 2),
                            mark_color,
                        ));
                        r.draw(DrawOp::Line(
                            (x + mark, y + t / 2),
                            (x + mark, y + (t * 11) / 20),
                            mark_color,
                        ));
                    }
                }
                db += 6.0;
                shade += step;
            }
        }
        // `FloDisplayBarSwitch::Draw` uses a half-thickness bar and switches
        // axes with `orient`; the old Rust widget always drew the vertical
        // double-width form.
        match self.orientation {
            super::videoio_displays::Orientation::Vertical => {
                r.draw(DrawOp::Box(x - t / 2, y, x + t / 2, y - l, c));
                if !self.dbscale && self.calibrate {
                    r.draw(DrawOp::Line(
                        (x - t / 2, y - (self.cval * scale) as i32),
                        (x + t / 2, y - (self.cval * scale) as i32),
                        Color(255, 255, 255, a),
                    ));
                }
            }
            super::videoio_displays::Orientation::Horizontal => {
                r.draw(DrawOp::Box(x, y - t / 2, x + l, y + t / 2, c));
                if !self.dbscale && self.calibrate {
                    r.draw(DrawOp::Line(
                        (x + (self.cval * scale) as i32, y - t / 2),
                        (x + (self.cval * scale) as i32, y + t / 2),
                        Color(255, 255, 255, a),
                    ));
                }
            }
        }
    }
}

pub struct FloDisplaySquares<V: Value> {
    pub base: FloDisplay,
    pub exp: V,
    pub orientation: super::videoio_displays::Orientation,
    pub v1: f32,
    pub v2: f32,
    pub sinterval: f32,
    pub sx: i32,
    pub sy: i32,
}
impl<V: Value> Display for FloDisplaySquares<V> {
    fn base(&self) -> &FloDisplay {
        &self.base
    }
    fn base_mut(&mut self) -> &mut FloDisplay {
        &mut self.base
    }
    fn render(&mut self, r: &mut dyn Renderer, m: &RenderMetrics) {
        if !self.base.show {
            return;
        }
        let n = ((self.exp.value() - self.v1) / self.sinterval)
            .clamp(0.0, ((self.v2 - self.v1) / self.sinterval).max(0.0)) as i32;
        for i in 0..n {
            let (x, y) = (
                m.x(self.base.xpos
                    + if self.orientation == super::videoio_displays::Orientation::Horizontal {
                        i * self.sx
                    } else {
                        0
                    }),
                m.y(self.base.ypos
                    + if self.orientation == super::videoio_displays::Orientation::Vertical {
                        i * self.sy
                    } else {
                        0
                    }),
            );
            r.draw(DrawOp::Box(
                x,
                y,
                x + m.x(self.sx),
                y + m.y(self.sy),
                Color(0xff, 0x50, 0x20, 255),
            ));
        }
    }
}

pub struct FloLayoutBox {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub lineleft: bool,
    pub lineright: bool,
    pub linetop: bool,
    pub linebottom: bool,
}
impl FloLayoutBox {
    pub fn render(&self, r: &mut dyn Renderer, m: &RenderMetrics, c: Color) {
        let (l, t, rr, b) = (
            m.x(self.left),
            m.y(self.top),
            m.x(self.right),
            m.y(self.bottom),
        );
        r.draw(DrawOp::Box(l, t, rr, b, c));
        let black = Color(0, 0, 0, 255);
        if self.lineleft {
            r.draw(DrawOp::Line((l, t), (l, b), black))
        }
        if self.lineright {
            r.draw(DrawOp::Line((rr, t), (rr, b), black))
        }
        if self.linetop {
            r.draw(DrawOp::Line((l, t), (rr, t), black))
        }
        if self.linebottom {
            r.draw(DrawOp::Line((l, b), (rr, b), black))
        }
    }
}

pub struct BrowserWidget {
    pub base: FloDisplay,
    pub browser: Browser,
    pub expanded: bool,
    pub expand_rect: (i32, i32, i32, i32),
    pub expand_delay: f64,
    pub last_activity: f64,
    pub line_height: i32,
}
impl BrowserWidget {
    pub fn move_by(&mut self, adjust: i32) {
        if let Some(i) = self.browser.current_index {
            let n = self.browser.items.len() as i32;
            self.browser.current_index = if n == 0 {
                None
            } else {
                Some((i as i32 + adjust).clamp(0, n - 1) as usize)
            };
            self.expanded = true;
        }
    }
    pub fn select_event(&self) -> Option<Event> {
        self.browser.current_index.map(|_| Event::BrowserSelectItem {
            browserid: self.browser.browser_id,
        })
    }
    pub fn move_event(&self, adjust: i32) -> Event {
        Event::BrowserMoveToItem {
            browserid: self.browser.browser_id,
            adjust,
            jump_adjust: 0,
        }
    }
}

pub struct LoopTray {
    pub browser: BrowserWidget,
    pub loop_size: i32,
    pub base_pos: i32,
    pub icon_size: i32,
    pub touch_tray: bool,
    pub item_positions: Vec<(i32, i32)>,
}
impl LoopTray {
    /// Logical port of `LoopTray::Draw`'s chrome and layout.  Loop waveform
    /// drawing remains owned by the production `DrawLoop` scene path, while
    /// this reusable widget supplies the same icon, expanded bezel, and item
    /// labels/grid to any renderer.
    pub fn render(&mut self, r: &mut dyn Renderer, m: &RenderMetrics) {
        if !self.browser.base.show {
            return;
        }
        let border = Color(100, 100, 90, 255);
        let outline = Color(40, 40, 40, 255);
        let x = m.x(self.browser.base.xpos);
        let y = m.y(self.browser.base.ypos);
        let icon = m.x(self.icon_size);
        r.draw(DrawOp::Box(x, y, x + icon, y + icon, border));
        r.draw(DrawOp::Line((x, y), (x + icon, y), outline));
        r.draw(DrawOp::Line((x, y + icon), (x + icon, y + icon), outline));
        r.draw(DrawOp::Line((x, y), (x, y + icon), outline));
        r.draw(DrawOp::Line((x + icon, y), (x + icon, y + icon), outline));
        r.draw(DrawOp::FilledPie(
            x + icon / 2,
            y + icon / 2,
            icon * 3 / 8,
            30,
            359,
            Color(0xf9, 0xe6, 0x13, 255),
        ));
        r.draw(DrawOp::Circle(
            x + icon / 2,
            y + icon / 2,
            icon * 3 / 8,
            outline,
        ));

        if !self.browser.expanded {
            return;
        }
        let (left, top, right, bottom) = self.browser.expand_rect;
        let (left, top, right, bottom) = (m.x(left), m.y(top), m.x(right), m.y(bottom));
        let base_x = m.x(self.base_pos);
        let base_y = m.y(self.base_pos);
        r.draw(DrawOp::Box(left, top, left + base_x, bottom, border));
        r.draw(DrawOp::Box(right - base_x, top, right, bottom, border));
        r.draw(DrawOp::Box(left, top, right, top + base_y, border));
        r.draw(DrawOp::Box(left, bottom - base_y, right, bottom, border));
        r.draw(DrawOp::Box(
            left + base_x,
            top + base_y,
            right - base_x,
            bottom - base_y,
            Color(0, 0, 0, 255),
        ));
        r.draw(DrawOp::Line((left, top), (right, top), outline));
        r.draw(DrawOp::Line((left, bottom), (right, bottom), outline));
        r.draw(DrawOp::Line((left, top), (left, bottom), outline));
        r.draw(DrawOp::Line((right, top), (right, bottom), outline));

        let width = self.browser.expand_rect.2 - self.browser.expand_rect.0;
        let height = self.browser.expand_rect.3 - self.browser.expand_rect.1;
        if self.touch_tray {
            self.recalculate(width, height);
        }
        for (item, &(item_x, item_y)) in self
            .browser
            .browser
            .items
            .iter()
            .filter(|item| item.item_type != super::browser::BrowserItemType::Division)
            .zip(&self.item_positions)
        {
            r.draw(DrawOp::Text(
                item.name.clone(),
                left + m.x(item_x),
                top + m.y(item_y + self.loop_size),
                Color(0xef, 0xaf, 0xff, 255),
                0,
                0,
            ));
        }
    }

    pub fn recalculate(&mut self, width: i32, height: i32) {
        self.item_positions.clear();
        let jump = self.loop_size + self.base_pos;
        let mut y = self.base_pos;
        while y + jump < height {
            let mut x = self.base_pos;
            while x + jump < width {
                self.item_positions.push((x, y));
                x += jump;
            }
            y += jump;
        }
        self.touch_tray = false;
    }
}

pub struct FloDisplaySnapshots {
    pub base: FloDisplay,
    pub names: Vec<Option<String>>,
    pub firstidx: usize,
    pub numdisp: Option<usize>,
    pub margin: i32,
}

pub struct ParamBar {
    pub name: Option<String>,
    pub value: f32,
}
pub struct FloDisplayParamSet {
    pub base: FloDisplay,
    pub sx: i32,
    pub sy: i32,
    pub margin: i32,
    pub bank_name: Option<String>,
    pub bars: Vec<ParamBar>,
    pub max_value: f32,
}
impl FloDisplayParamSet {
    pub fn render(&self, r: &mut dyn Renderer, m: &RenderMetrics) {
        if !self.base.show {
            return;
        }
        let (x, y) = (m.x(self.base.xpos), m.y(self.base.ypos));
        r.draw(DrawOp::Box(
            x,
            y,
            x + m.x(self.sx),
            y + m.y(self.sy),
            Color(0, 0, 0, 190),
        ));
        let border = Color(0xff, 0x50, 0x20, 255);
        r.draw(DrawOp::Line((x, y), (x + m.x(self.sx), y), border));
        r.draw(DrawOp::Line(
            (x, y + m.y(self.sy)),
            (x + m.x(self.sx), y + m.y(self.sy)),
            border,
        ));
        r.draw(DrawOp::Line((x, y), (x, y + m.y(self.sy)), border));
        r.draw(DrawOp::Line(
            (x + m.x(self.sx), y),
            (x + m.x(self.sx), y + m.y(self.sy)),
            border,
        ));
        if let Some(title) = &self.base.title {
            r.draw(DrawOp::Text(
                title.clone(),
                x + m.x(self.sx - self.margin),
                y,
                Color(0x77, 0x88, 0x99, 255),
                2,
                0,
            ));
        }
        if let Some(bank_name) = &self.bank_name {
            r.draw(DrawOp::Text(
                bank_name.clone(),
                x + m.x(self.margin),
                y,
                Color(0x77, 0x88, 0x99, 255),
                0,
                0,
            ));
        }
        let spacing = (self.sx - 2 * self.margin) / self.bars.len().max(1) as i32;
        for (i, bar) in self.bars.iter().enumerate() {
            let bx = m.x(self.base.xpos + self.margin + i as i32 * spacing);
            let level = if self.max_value > 0.0 {
                (bar.value / self.max_value).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let baseline = y + m.y(self.sy - self.margin);
            let max_height = self.sy - self.margin * 3;
            let h = (max_height as f32 * level) as i32;
            let thickness = (spacing / 4).max(1);
            if let Some(name) = &bar.name {
                r.draw(DrawOp::Text(
                    name.clone(),
                    bx + m.x(thickness * 2),
                    baseline,
                    Color(0x77, 0x88, 0x99, 255),
                    1,
                    2,
                ));
            }
            // C++ draws a dim full-scale reference, then a wide dim value,
            // then the narrow bright value bar.
            r.draw(DrawOp::Box(
                bx - m.x(thickness / 2),
                baseline,
                bx + m.x(thickness / 2),
                baseline - m.y(max_height),
                Color(0x7f, 0x28, 0x10, 255),
            ));
            r.draw(DrawOp::Box(
                bx - m.x(thickness),
                baseline - m.y(h),
                bx + m.x(thickness),
                baseline,
                Color(0x7f, 0x28, 0x10, 255),
            ));
            r.draw(DrawOp::Box(
                bx - m.x(thickness / 2),
                baseline - m.y(h),
                bx + m.x(thickness / 2),
                baseline,
                Color(0xff, 0x50, 0x20, 255),
            ));
        }
    }
}
impl FloDisplaySnapshots {
    pub fn visible(&self) -> &[Option<String>] {
        let n = self.numdisp.unwrap_or(self.names.len());
        let end = (self.firstidx + n).min(self.names.len());
        &self.names[self.firstidx.min(end)..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::BrowserItemType;
    struct R(Vec<DrawOp>);
    impl Renderer for R {
        fn draw(&mut self, o: DrawOp) {
            self.0.push(o)
        }
    }
    #[test]
    fn layout_lines() {
        let mut r = R(Vec::new());
        FloLayoutBox {
            left: 1,
            top: 2,
            right: 3,
            bottom: 4,
            lineleft: true,
            linetop: true,
            lineright: false,
            linebottom: false,
        }
        .render(
            &mut r,
            &RenderMetrics::new(10, 10, 20, 20),
            Color(1, 2, 3, 4),
        );
        assert_eq!(r.0.len(), 3)
    }
    #[test]
    fn tray_positions_match_grid() {
        let mut t = LoopTray {
            browser: BrowserWidget {
                base: FloDisplay::new(0),
                browser: Browser::new("", BrowserItemType::Loop),
                expanded: true,
                expand_rect: (0, 0, 20, 20),
                expand_delay: 0.,
                last_activity: 0.,
                line_height: 1,
            },
            loop_size: 4,
            base_pos: 1,
            icon_size: 4,
            touch_tray: true,
            item_positions: vec![],
        };
        t.recalculate(12, 12);
        assert_eq!(t.item_positions, vec![(1, 1), (6, 1), (1, 6), (6, 6)]);
    }

    #[test]
    fn tray_renders_cpp_icon_and_expanded_bezel() {
        let mut tray = LoopTray {
            browser: BrowserWidget {
                base: FloDisplay::new(0),
                browser: Browser::new("", BrowserItemType::Loop),
                expanded: true,
                expand_rect: (10, 20, 40, 50),
                expand_delay: 0.0,
                last_activity: 0.0,
                line_height: 1,
            },
            loop_size: 4,
            base_pos: 1,
            icon_size: 8,
            touch_tray: true,
            item_positions: vec![],
        };
        let mut renderer = R(Vec::new());
        tray.render(&mut renderer, &RenderMetrics::new(64, 64, 64, 64));
        assert!(
            renderer
                .0
                .iter()
                .any(|op| matches!(op, DrawOp::FilledPie(_, _, _, 30, 359, _)))
        );
        assert!(
            renderer
                .0
                .iter()
                .any(|op| matches!(op, DrawOp::Box(_, _, _, _, Color(0, 0, 0, 255))))
        );
    }

    #[test]
    fn bar_switch_uses_cpp_orientation_and_half_thickness() {
        let base = FloDisplay {
            xpos: 10,
            ypos: 20,
            ..FloDisplay::new(0)
        };
        let make = |orientation| FloDisplayBarSwitch {
            base: base.clone(),
            exp: || 0.5,
            switchexp: || 1.0,
            orientation,
            barscale: 20.0,
            thickness: 6,
            dbscale: false,
            marks: false,
            maxdb: 0.0,
            color: 0,
            calibrate: false,
            cval: 0.0,
        };
        let metrics = RenderMetrics::new(100, 100, 100, 100);
        let mut vertical = make(super::super::videoio_displays::Orientation::Vertical);
        let mut r = R(Vec::new());
        vertical.render(&mut r, &metrics);
        assert!(matches!(r.0.last(), Some(DrawOp::Box(7, 20, 13, 10, _))));
        let mut horizontal = make(super::super::videoio_displays::Orientation::Horizontal);
        let mut r = R(Vec::new());
        horizontal.render(&mut r, &metrics);
        assert!(matches!(r.0.last(), Some(DrawOp::Box(10, 17, 20, 23, _))));
    }

    #[test]
    fn bar_switch_uses_cpp_db_ticks_and_switch_alpha() {
        let mut display = FloDisplayBarSwitch {
            base: FloDisplay::new(0),
            exp: || 1.0,
            switchexp: || 0.0,
            orientation: super::super::videoio_displays::Orientation::Vertical,
            barscale: 60.0,
            thickness: 10,
            dbscale: true,
            marks: true,
            maxdb: 0.0,
            color: 0,
            calibrate: false,
            cval: 0.0,
        };
        let mut renderer = R(Vec::new());
        display.render(&mut renderer, &RenderMetrics::new(100, 100, 100, 100));
        assert!(
            renderer
                .0
                .iter()
                .any(|op| matches!(op, DrawOp::Line(_, _, Color(_, _, _, 127))))
        );
        assert!(matches!(
            renderer.0.last(),
            Some(DrawOp::Box(_, _, _, _, Color(0xef, 0xaf, 0xff, 127)))
        ));
    }

    #[test]
    fn parameter_set_renders_cpp_reference_and_value_bars() {
        let display = FloDisplayParamSet {
            base: FloDisplay {
                title: Some("params".into()),
                ..FloDisplay::new(0)
            },
            sx: 100,
            sy: 40,
            margin: 4,
            bank_name: Some("bank".into()),
            bars: vec![ParamBar {
                name: Some("gain".into()),
                value: 0.5,
            }],
            max_value: 1.0,
        };
        let mut renderer = R(Vec::new());
        display.render(&mut renderer, &RenderMetrics::new(100, 50, 100, 50));
        assert!(
            renderer
                .0
                .iter()
                .any(|op| matches!(op, DrawOp::Line(_, _, Color(0xff, 0x50, 0x20, 255))))
        );
        assert!(
            renderer
                .0
                .iter()
                .any(|op| matches!(op, DrawOp::Box(_, _, _, _, Color(0x7f, 0x28, 0x10, 255))))
        );
        assert!(
            renderer
                .0
                .iter()
                .any(|op| matches!(op, DrawOp::Box(_, _, _, _, Color(0xff, 0x50, 0x20, 255))))
        );
    }
}
