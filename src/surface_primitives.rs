//! Deterministic, renderer-independent equivalents of the SDL surface
//! primitives used by FreeWheeling.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgba(r, g, b, 255)
    }
    pub const fn packed(self) -> u32 {
        ((self.a as u32) << 24) | ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoftwareSurface {
    width: i32,
    height: i32,
    pixels: Vec<u32>,
    clip: Option<(i32, i32, i32, i32)>,
}

impl SoftwareSurface {
    pub fn new(width: i32, height: i32) -> Self {
        let width = width.max(0);
        let height = height.max(0);
        Self {
            width,
            height,
            pixels: vec![0; (width as usize).saturating_mul(height as usize)],
            clip: None,
        }
    }
    pub fn width(&self) -> i32 {
        self.width
    }
    pub fn height(&self) -> i32 {
        self.height
    }
    pub fn pixels(&self) -> &[u32] {
        &self.pixels
    }
    pub fn clear(&mut self, color: Color) {
        self.pixels.fill(color.packed());
    }
    pub fn set_clip(&mut self, clip: Option<(i32, i32, i32, i32)>) {
        self.clip = clip.map(|(x, y, w, h)| (x, y, x.saturating_add(w), y.saturating_add(h)));
    }
    pub fn rgba_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 4);
        for &pixel in &self.pixels {
            out.extend_from_slice(&[
                ((pixel >> 16) & 0xff) as u8,
                ((pixel >> 8) & 0xff) as u8,
                (pixel & 0xff) as u8,
                ((pixel >> 24) & 0xff) as u8,
            ]);
        }
        out
    }
    pub fn pixel(&self, x: i32, y: i32) -> Option<u32> {
        (x >= 0 && y >= 0 && x < self.width && y < self.height)
            .then(|| self.pixels[y as usize * self.width as usize + x as usize])
    }
    fn put_pixel(&mut self, x: i32, y: i32, color: u32) {
        let inside_clip = self.clip.is_none_or(|(left, top, right, bottom)| {
            x >= left && y >= top && x < right && y < bottom
        });
        if x < 0 || y < 0 || x >= self.width || y >= self.height || !inside_clip {
            return;
        }
        let pixel = &mut self.pixels[y as usize * self.width as usize + x as usize];
        // `fweelin_surface_primitives.cc` maps RGBA and writes that mapped
        // pixel directly for every primitive; it does not source-over blend.
        *pixel = color;
    }
    pub(crate) fn put_opaque_pixel(&mut self, x: i32, y: i32, color: u32) {
        let inside_clip = self.clip.is_none_or(|(left, top, right, bottom)| {
            x >= left && y >= top && x < right && y < bottom
        });
        if x < 0 || y < 0 || x >= self.width || y >= self.height || !inside_clip {
            return;
        }
        self.pixels[y as usize * self.width as usize + x as usize] = color;
    }
    pub fn blit_rgba(
        &mut self,
        source: &[u8],
        source_width: i32,
        source_height: i32,
        source_stride: usize,
        dst: (i32, i32, i32, i32),
    ) {
        let (dx, dy, dw, dh) = dst;
        if source_width <= 0
            || source_height <= 0
            || dw <= 0
            || dh <= 0
            || source_stride < source_width as usize * 4
        {
            return;
        }
        for y in 0..dh {
            let sy = (y as i64 * source_height as i64 / dh as i64) as i32;
            for x in 0..dw {
                let sx = (x as i64 * source_width as i64 / dw as i64) as i32;
                let offset = sy as usize * source_stride + sx as usize * 4;
                if let Some(rgba) = source.get(offset..offset + 4) {
                    let x = dx + x;
                    let y = dy + y;
                    let inside_clip = self.clip.is_none_or(|(left, top, right, bottom)| {
                        x >= left && y >= top && x < right && y < bottom
                    });
                    if x >= 0 && y >= 0 && x < self.width && y < self.height && inside_clip {
                        let pixel = &mut self.pixels[y as usize * self.width as usize + x as usize];
                        *pixel = blend(
                            *pixel,
                            Color::rgba(rgba[0], rgba[1], rgba[2], rgba[3]).packed(),
                        );
                    }
                }
            }
        }
    }
    pub fn hline_rgba(&mut self, mut x1: i32, mut x2: i32, y: i32, color: u32) {
        if x2 < x1 {
            std::mem::swap(&mut x1, &mut x2);
        }
        for x in x1..=x2 {
            self.put_pixel(x, y, color);
        }
    }
    pub fn vline_rgba(&mut self, x: i32, mut y1: i32, mut y2: i32, color: u32) {
        if y2 < y1 {
            std::mem::swap(&mut y1, &mut y2);
        }
        for y in y1..=y2 {
            self.put_pixel(x, y, color);
        }
    }
    pub fn box_rgba(&mut self, mut x1: i32, mut y1: i32, mut x2: i32, mut y2: i32, color: u32) {
        if x2 < x1 {
            std::mem::swap(&mut x1, &mut x2);
        }
        if y2 < y1 {
            std::mem::swap(&mut y1, &mut y2);
        }
        for y in y1..=y2 {
            self.hline_rgba(x1, x2, y, color);
        }
    }
    pub fn line_rgba(&mut self, mut x1: i32, mut y1: i32, x2: i32, y2: i32, color: u32) {
        let dx = (x2 - x1).abs();
        let dy = (y2 - y1).abs();
        let sx = if x1 < x2 { 1 } else { -1 };
        let sy = if y1 < y2 { 1 } else { -1 };
        let mut err = dx - dy;
        loop {
            self.put_pixel(x1, y1, color);
            if x1 == x2 && y1 == y2 {
                break;
            }
            let e2 = err * 2;
            if e2 > -dy {
                err -= dy;
                x1 += sx;
            }
            if e2 < dx {
                err += dx;
                y1 += sy;
            }
        }
    }
    pub fn circle_rgba(&mut self, x: i32, y: i32, rad: i32, color: u32) {
        if rad < 0 {
            return;
        }
        let (mut dx, mut dy, mut err) = (rad, 0, 1 - rad);
        while dx >= dy {
            for (px, py) in [
                (x + dx, y + dy),
                (x + dy, y + dx),
                (x - dy, y + dx),
                (x - dx, y + dy),
                (x - dx, y - dy),
                (x - dy, y - dx),
                (x + dy, y - dx),
                (x + dx, y - dy),
            ] {
                self.put_pixel(px, py, color);
            }
            dy += 1;
            if err < 0 {
                err += 2 * dy + 1;
            } else {
                dx -= 1;
                err += 2 * (dy - dx) + 1;
            }
        }
    }
    pub fn filled_circle_rgba(&mut self, x: i32, y: i32, rad: i32, color: u32) {
        if rad < 0 {
            return;
        }
        for dy in -rad..=rad {
            let span = ((rad * rad - dy * dy) as f64).sqrt() as i32;
            self.hline_rgba(x - span, x + span, y + dy, color);
        }
    }
    pub fn filled_pie_rgba(&mut self, x: i32, y: i32, rad: i32, start: i32, end: i32, color: u32) {
        if rad < 0 {
            return;
        }
        for dy in -rad..=rad {
            for dx in -rad..=rad {
                if dx * dx + dy * dy <= rad * rad
                    && angle_within_range(
                        (-dy as f64).atan2(dx as f64).to_degrees(),
                        start as f64,
                        end as f64,
                    )
                {
                    self.put_pixel(x + dx, y + dy, color);
                }
            }
        }
    }
    pub fn filledpie_rgba(&mut self, x: i32, y: i32, rad: i32, start: i32, end: i32, color: u32) {
        self.filled_pie_rgba(x, y, rad, start, end, color);
    }
}

fn blend(dst: u32, src: u32) -> u32 {
    let sa = (src >> 24) & 0xff;
    if sa == 255 {
        return src;
    }
    if sa == 0 {
        return dst;
    }
    let da = (dst >> 24) & 0xff;
    if da == 0 {
        return src;
    }
    let inv = 255 - sa;
    let a = sa + (da * inv + 127) / 255;
    let channel = |shift: u32| {
        let source = (src >> shift) & 0xff;
        let destination = (dst >> shift) & 0xff;
        (source * sa * 255 + destination * da * inv + a * 255 / 2) / (a * 255)
    };
    (a << 24) | (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

fn angle_within_range(mut angle: f64, mut start: f64, mut end: f64) -> bool {
    while angle < 0.0 {
        angle += 360.0;
    }
    while start < 0.0 {
        start += 360.0;
    }
    while end < 0.0 {
        end += 360.0;
    }
    angle %= 360.0;
    start %= 360.0;
    end %= 360.0;
    if start <= end {
        angle >= start && angle <= end
    } else {
        angle >= start || angle <= end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn primitives_clip_and_pack() {
        let mut s = SoftwareSurface::new(3, 3);
        let c = Color::rgba(1, 2, 3, 4).packed();
        s.box_rgba(-1, -1, 1, 1, c);
        assert_eq!(s.pixel(0, 0), Some(c));
        assert_eq!(s.pixel(2, 2), Some(0));
    }
    #[test]
    fn circle_and_pie_are_inclusive() {
        let mut s = SoftwareSurface::new(7, 7);
        s.circle_rgba(3, 3, 2, Color::rgb(1, 1, 1).packed());
        assert!(s.pixel(5, 3).is_some());
        let pie = Color::rgb(2, 2, 2).packed();
        s.filled_pie_rgba(3, 3, 2, 0, 90, pie);
        assert_eq!(s.pixel(5, 3), Some(pie));
    }
    #[test]
    fn alpha_clip_and_nearest_scaling_are_deterministic() {
        let mut s = SoftwareSurface::new(3, 2);
        s.clear(Color::rgb(0, 0, 255));
        s.set_clip(Some((1, 0, 2, 2)));
        s.blit_rgba(&[255, 0, 0, 128], 1, 1, 4, (0, 0, 3, 2));
        assert_eq!(s.pixel(0, 0), Some(Color::rgb(0, 0, 255).packed()));
        assert_eq!(s.pixel(1, 0), Some(Color::rgba(128, 0, 127, 255).packed()));
        assert_eq!(&s.rgba_bytes()[4..8], &[128, 0, 127, 255]);
    }
    #[test]
    fn primitive_alpha_is_a_direct_cpp_pixel_write() {
        let mut s = SoftwareSurface::new(1, 1);
        s.clear(Color::rgb(0, 0, 255));
        let translucent = Color::rgba(255, 0, 0, 128).packed();
        s.hline_rgba(0, 0, 0, translucent);
        assert_eq!(s.pixel(0, 0), Some(translucent));
    }
}
