//! Layout configuration and geometry for loop-oriented video displays.
//!
//! Coordinates are logical (640x480) coordinates.  A renderer owns font
//! handles and text measurement; layout code never guesses glyph metrics.

use crate::videoio_displays::{Color, DrawOp, RenderMetrics, Renderer};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextMetrics {
    pub width: i32,
    pub height: i32,
}

pub trait LayoutRenderer: Renderer {
    fn text_metrics(&self, font: &FloFont, text: &str) -> TextMetrics;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FloFont {
    pub name: Option<String>,
    pub filename: Option<String>,
    pub size: i32,
}
impl FloFont {
    pub fn new(name: impl Into<String>, filename: impl Into<String>, size: i32) -> Self {
        Self {
            name: Some(name.into()),
            filename: Some(filename.into()),
            size,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FloLayoutBox {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub lineleft: bool,
    pub linetop: bool,
    pub lineright: bool,
    pub linebottom: bool,
}
impl FloLayoutBox {
    pub fn inside(&self, x: i32, y: i32) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }
    pub fn render(&self, r: &mut dyn Renderer, m: &RenderMetrics, color: Color) {
        let (l, t, rr, b) = (
            m.x(self.left),
            m.y(self.top),
            m.x(self.right),
            m.y(self.bottom),
        );
        r.draw(DrawOp::Box(l, t, rr, b, color));
        let black = Color(0, 0, 0, 255);
        if self.lineleft {
            r.draw(DrawOp::Line((l, t), (l, b), black));
        }
        if self.lineright {
            r.draw(DrawOp::Line((rr, t), (rr, b), black));
        }
        if self.linetop {
            r.draw(DrawOp::Line((l, t), (rr, t), black));
        }
        if self.linebottom {
            r.draw(DrawOp::Line((l, b), (rr, b), black));
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FloLayoutElement {
    pub id: i32,
    pub name: Option<String>,
    pub nxpos: i32,
    pub nypos: i32,
    pub bx: f32,
    pub by: f32,
    pub loopx: i32,
    pub loopy: i32,
    pub loopsize: i32,
    /// Name of a `state.values` key (e.g. a config variable or system value
    /// such as `streaming`). While the value is nonzero the element is
    /// rendered in the active color, giving touch buttons visible feedback
    /// for latched/recording states.
    pub toggle: Option<String>,
    /// Upper bound for the `toggle` value (seconds for time-based feedback
    /// such as `scene-save-age`): the element is active while
    /// `0 < value < togglemax`. `None` means any nonzero value activates it.
    pub togglemax: Option<f32>,
    /// Center the element label (name) inside its first box instead of
    /// placing it at `namepos`.
    pub labelcenter: bool,
    pub geometry: Vec<FloLayoutBox>,
}
impl FloLayoutElement {
    pub fn inside(&self, x: i32, y: i32) -> bool {
        self.geometry.iter().any(|g| g.inside(x, y))
    }
    pub fn add_box(&mut self, geometry: FloLayoutBox) {
        self.geometry.push(geometry);
    }
    pub fn label_metrics<R: LayoutRenderer>(&self, r: &R, font: &FloFont) -> Option<TextMetrics> {
        self.name.as_deref().map(|name| r.text_metrics(font, name))
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FloLayout {
    pub id: i32,
    pub iid: i32,
    pub xpos: i32,
    pub ypos: i32,
    pub name: Option<String>,
    pub nxpos: i32,
    pub nypos: i32,
    pub loopids: (i32, i32),
    pub elements: Vec<FloLayoutElement>,
    pub show: bool,
    pub showlabel: bool,
    pub showelabel: bool,
    /// Text drawn centered over the layout while no loop has any audio
    /// (mobile first-run hint, e.g. "tap a cell to record").
    pub emptyhint: Option<String>,
}
impl FloLayout {
    pub fn new() -> Self {
        Self {
            show: true,
            showlabel: true,
            showelabel: true,
            ..Default::default()
        }
    }
    pub fn add_element(&mut self, element: FloLayoutElement) {
        self.elements.push(element);
    }
    pub fn element_at(&self, x: i32, y: i32) -> Option<&FloLayoutElement> {
        // Topmost first: elements render in order, so a later element draws
        // over an earlier one and wins the hit test (e.g. the sessions panel
        // background must not swallow its row buttons).
        self.elements.iter().rev().find(|element| element.inside(x, y))
    }
    pub fn label_metrics<R: LayoutRenderer>(&self, r: &R, font: &FloFont) -> Option<TextMetrics> {
        self.name.as_deref().map(|name| r.text_metrics(font, name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct R(Vec<DrawOp>);
    impl Renderer for R {
        fn draw(&mut self, op: DrawOp) {
            self.0.push(op);
        }
    }
    impl LayoutRenderer for R {
        fn text_metrics(&self, font: &FloFont, text: &str) -> TextMetrics {
            TextMetrics {
                width: text.len() as i32 * font.size,
                height: font.size,
            }
        }
    }
    #[test]
    fn element_at_prefers_topmost_element() {
        // Elements render in order; a later element draws over an earlier one
        // and must win the hit test (e.g. the sessions panel background must
        // not swallow its row buttons).
        let mut layout = FloLayout::new();
        layout.add_element(FloLayoutElement {
            id: 199,
            geometry: vec![FloLayoutBox {
                left: 0,
                top: 0,
                right: 100,
                bottom: 100,
                ..Default::default()
            }],
            ..Default::default()
        });
        layout.add_element(FloLayoutElement {
            id: 200,
            geometry: vec![FloLayoutBox {
                left: 10,
                top: 10,
                right: 90,
                bottom: 40,
                ..Default::default()
            }],
            ..Default::default()
        });
        // The row (200) overlaps the background (199); the topmost row wins.
        let hit = layout.element_at(50, 20).unwrap();
        assert_eq!(hit.id, 200);
        // Outside the row but inside the background, the background wins.
        let hit = layout.element_at(50, 80).unwrap();
        assert_eq!(hit.id, 199);
    }

    #[test]
    fn box_edges_are_inside() {
        let b = FloLayoutBox {
            left: 1,
            top: 2,
            right: 3,
            bottom: 4,
            ..Default::default()
        };
        assert!(b.inside(1, 2));
        assert!(b.inside(3, 4));
        assert!(!b.inside(0, 2));
    }
    #[test]
    fn layout_finds_element_and_measures_label() {
        let mut l = FloLayout::new();
        let mut e = FloLayoutElement {
            id: 7,
            name: Some("loop".into()),
            ..Default::default()
        };
        e.add_box(FloLayoutBox {
            left: 10,
            top: 10,
            right: 20,
            bottom: 20,
            ..Default::default()
        });
        l.add_element(e);
        let r = R(vec![]);
        assert_eq!(l.element_at(15, 15).unwrap().id, 7);
        assert_eq!(
            l.elements[0]
                .label_metrics(&r, &FloFont::new("x", "x", 2))
                .unwrap()
                .width,
            8
        );
    }
}
