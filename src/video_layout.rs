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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FloStringList {
    pub str: String,
    pub str2: Option<String>,
    pub next: Option<Box<FloStringList>>,
}
impl FloStringList {
    pub fn new(str: impl Into<String>, str2: Option<String>) -> Self {
        Self {
            str: str.into(),
            str2,
            next: None,
        }
    }
    pub fn push(&mut self, item: FloStringList) {
        match &mut self.next {
            Some(next) => next.push(item),
            None => self.next = Some(Box::new(item)),
        }
    }
    pub fn get(&self, index: usize, column: usize) -> Option<&str> {
        let mut cur = self;
        for _ in 0..index {
            cur = cur.next.as_deref()?;
        }
        match column {
            0 => Some(&cur.str),
            1 => cur.str2.as_deref(),
            _ => None,
        }
    }
    pub fn len(&self) -> usize {
        1 + self.next.as_deref().map_or(0, Self::len)
    }
    pub fn is_empty(&self) -> bool {
        false
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
        self.elements.iter().find(|element| element.inside(x, y))
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
    #[test]
    fn string_list_preserves_two_columns() {
        let mut list = FloStringList::new("a", Some("b".into()));
        list.push(FloStringList::new("c", None));
        assert_eq!(list.len(), 2);
        assert_eq!(list.get(1, 0), Some("c"));
        assert_eq!(list.get(0, 1), Some("b"));
    }
}
