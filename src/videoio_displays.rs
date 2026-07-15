//! Display state and rendering calculations formerly implemented by
//! `fweelin_videoio_displays.cc`.
//!
//! The renderer is deliberately an API boundary: SDL (or another backend)
//! implements [`Renderer`], while these types retain the old logical 640x480
//! coordinate system and display hierarchy.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderMetrics {
    pub logical_width: i32,
    pub logical_height: i32,
    pub drawable_width: i32,
    pub drawable_height: i32,
    pub scale_x: f32,
    pub scale_y: f32,
}
impl RenderMetrics {
    pub fn new(w: i32, h: i32, dw: i32, dh: i32) -> Self {
        let logical_width = if w <= 0 {
            if dw > 0 { dw } else { 1 }
        } else {
            w
        };
        let logical_height = if h <= 0 {
            if dh > 0 { dh } else { 1 }
        } else {
            h
        };
        let drawable_width = if dw <= 0 { logical_width } else { dw };
        let drawable_height = if dh <= 0 { logical_height } else { dh };
        Self {
            logical_width,
            logical_height,
            drawable_width,
            drawable_height,
            scale_x: drawable_width as f32 / logical_width as f32,
            scale_y: drawable_height as f32 / logical_height as f32,
        }
    }

    fn scale_extent(v: i32, scale: f32) -> i32 {
        if v <= 0 {
            0
        } else if scale <= 0.0 {
            v
        } else {
            ((v as f32 * scale + 0.5) as i32).max(1)
        }
    }
    pub fn x(&self, v: i32) -> i32 {
        Self::scale_extent(v, self.scale_x)
    }
    pub fn y(&self, v: i32) -> i32 {
        Self::scale_extent(v, self.scale_y)
    }
    pub fn extent(&self, v: i32, s: f32) -> i32 {
        Self::scale_extent(v, s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color(pub u8, pub u8, pub u8, pub u8);
#[derive(Clone, Debug, PartialEq)]
pub enum DrawOp {
    Box(i32, i32, i32, i32, Color),
    Line((i32, i32), (i32, i32), Color),
    Circle(i32, i32, i32, Color),
    Text(String, i32, i32, Color, i8, i8),
    StyledText(String, String, f32, i32, i32, Color, i8, i8),
    FilledCircle(i32, i32, i32, Color),
    FilledPie(i32, i32, i32, i32, i32, Color),
    Waveform(Vec<f32>, i32, i32, i32, i32, Color),
    /// Legacy loop scope: a peak/average strip circularly mapped by the
    /// platform renderer. `position` rotates the strip with playback.
    LoopScope(
        Vec<f32>,
        Vec<f32>,
        u16,
        i32,
        i32,
        i32,
        Color,
        Color,
        Color,
        f32,
        i32,
        i32,
    ),
    Image(Vec<u8>, u32, u32, i32, i32, i32, i32),
}
pub trait Renderer {
    fn draw(&mut self, op: DrawOp);
}
pub trait Value {
    fn value(&self) -> f32;
}
impl<F: Fn() -> f32> Value for F {
    fn value(&self) -> f32 {
        self()
    }
}

#[derive(Clone, Debug)]
pub struct FloDisplay {
    pub iid: i32,
    pub id: i32,
    pub title: Option<String>,
    pub xpos: i32,
    pub ypos: i32,
    pub show: bool,
    pub forceshow: bool,
}
impl FloDisplay {
    pub fn new(iid: i32) -> Self {
        Self {
            iid,
            id: -1,
            title: None,
            xpos: 0,
            ypos: 0,
            show: true,
            forceshow: false,
        }
    }
    pub fn set_show(&mut self, v: bool) {
        self.show = v
    }
}
pub trait Display {
    fn base(&self) -> &FloDisplay;
    fn base_mut(&mut self) -> &mut FloDisplay;
    fn render(&mut self, r: &mut dyn Renderer, m: &RenderMetrics);
}

pub struct FloDisplayPanel {
    pub base: FloDisplay,
    pub sx: i32,
    pub sy: i32,
    pub margin: i32,
    pub children: Vec<Box<dyn Display>>,
}
impl FloDisplayPanel {
    pub fn new(iid: i32) -> Self {
        Self {
            base: FloDisplay::new(iid),
            sx: 100,
            sy: 100,
            margin: 0,
            children: Vec::new(),
        }
    }
}
impl Display for FloDisplayPanel {
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
        let x = m.x(self.base.xpos);
        let y = m.y(self.base.ypos);
        let w = m.x(self.sx);
        let h = m.y(self.sy);
        r.draw(DrawOp::Box(x, y, x + w, y + h, Color(0, 0, 0, 190)));
        for c in &mut self.children {
            c.render(r, m)
        }
    }
}

pub struct FloDisplayText<V: Value> {
    pub base: FloDisplay,
    pub exp: V,
}
impl<V: Value> Display for FloDisplayText<V> {
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
        let x = m.x(self.base.xpos);
        let y = m.y(self.base.ypos);
        if let Some(t) = &self.base.title {
            r.draw(DrawOp::Text(
                t.clone(),
                x,
                y,
                Color(0x77, 0x88, 0x99, 255),
                0,
                1,
            ));
            r.draw(DrawOp::Text(
                format!("{}", self.exp.value()),
                x,
                y,
                Color(0xdf, 0xef, 0x20, 255),
                0,
                1,
            ));
        }
    }
}

pub struct FloDisplaySwitch<V: Value> {
    pub base: FloDisplay,
    pub exp: V,
}
impl<V: Value> Display for FloDisplaySwitch<V> {
    fn base(&self) -> &FloDisplay {
        &self.base
    }
    fn base_mut(&mut self) -> &mut FloDisplay {
        &mut self.base
    }
    fn render(&mut self, r: &mut dyn Renderer, m: &RenderMetrics) {
        if self.base.show
            && let Some(t) = &self.base.title
        {
            let c = if self.exp.value() != 0.0 {
                Color(0xdf, 0xef, 0x20, 255)
            } else {
                Color(0x11, 0x22, 0x33, 255)
            };
            r.draw(DrawOp::Text(
                t.clone(),
                m.x(self.base.xpos),
                m.y(self.base.ypos),
                c,
                0,
                1,
            ));
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Orientation {
    Horizontal,
    Vertical,
}
pub struct FloDisplayBar<V: Value> {
    pub base: FloDisplay,
    pub exp: V,
    pub orientation: Orientation,
    pub barscale: f32,
    pub thickness: i32,
    pub dbscale: bool,
    pub marks: bool,
    pub maxdb: f32,
}
impl<V: Value> FloDisplayBar<V> {
    pub fn level(&self, m: &RenderMetrics) -> f32 {
        let v = self.exp.value();
        let f = if self.dbscale {
            let db = if v > 0.0 { 20.0 * v.log10() } else { -60.0 };
            ((db + 60.0) / (self.maxdb + 60.0)).clamp(0.0, 1.0)
        } else {
            v
        };
        f * self.barscale
            * if self.orientation == Orientation::Horizontal {
                m.scale_x
            } else {
                m.scale_y
            }
    }
}
impl<V: Value> Display for FloDisplayBar<V> {
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
        let x = m.x(self.base.xpos);
        let y = m.y(self.base.ypos);
        let t = m.extent(
            self.thickness,
            if self.orientation == Orientation::Horizontal {
                m.scale_y
            } else {
                m.scale_x
            },
        );
        let l = self.level(m) as i32;
        let c = Color(0xff, 0x50, 0x20, 255);
        if self.orientation == Orientation::Vertical {
            r.draw(DrawOp::Box(x - t, y, x + t, y - l, c))
        } else {
            r.draw(DrawOp::Box(x, y - t, x + l, y + t, c))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct R(Vec<DrawOp>);
    impl Renderer for R {
        fn draw(&mut self, o: DrawOp) {
            self.0.push(o)
        }
    }
    #[test]
    fn metrics_scale() {
        assert_eq!(RenderMetrics::new(640, 480, 1280, 960).x(10), 20)
    }
    #[test]
    fn metrics_match_cpp_zero_extent_rules() {
        let m = RenderMetrics::new(0, 0, 1280, 960);
        assert_eq!((m.logical_width, m.logical_height), (1280, 960));
        let m = RenderMetrics::new(640, 480, 0, 0);
        assert_eq!((m.drawable_width, m.drawable_height), (640, 480));
        assert_eq!(m.x(-3), 0);
        assert_eq!(m.y(0), 0);
        assert_eq!(m.extent(7, 0.0), 7);
    }
    #[test]
    fn panel_renders_children() {
        let mut p = FloDisplayPanel::new(1);
        p.children.push(Box::new(FloDisplaySwitch {
            base: {
                let mut b = FloDisplay::new(1);
                b.title = Some("x".into());
                b
            },
            exp: || 1.0,
        }));
        let mut r = R(Vec::new());
        p.render(&mut r, &RenderMetrics::new(640, 480, 640, 480));
        assert_eq!(r.0.len(), 2);
    }
}
