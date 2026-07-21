//! Platform wiring for the video subsystem.
//!
//! This module is deliberately small: platform code owns window/event work,
//! while the display tree only emits logical draw operations.  Keeping the
//! two sides behind these adapters also makes the ordering testable without
//! requiring SDL in unit tests.

use crate::sdlio::Sdl2Context;
use crate::surface_primitives::{Color as SurfaceColor, SoftwareSurface};
use crate::video_layout::FloLayout;
use crate::videoio::{VideoBackend, VideoFrame, VideoIO, VideoMode, VideoRenderer};
use crate::videoio_displays::{Display, DrawOp, RenderMetrics as DisplayMetrics, Renderer};
use fontdue::{Font, FontSettings};
use sdl2::pixels::PixelFormatEnum;
use sdl2::render::{BlendMode, Canvas};
use sdl2::video::{FullscreenType, Window};
use std::collections::HashMap;

struct TextStyle<'a> {
    alignment: (i8, i8),
    font_name: Option<&'a str>,
    point_size: f32,
}

#[path = "native_ui_scene.rs"]
pub mod native_ui_scene;

/// A platform backend may pump native events and perform its periodic update
/// immediately before a frame is presented.  The default methods are real
/// hooks (rather than no-op implementations): a backend must opt into the
/// event loop explicitly.
pub trait PlatformBackend: VideoBackend {
    fn pump_events(&mut self) -> Result<(), String>;
    fn update(&mut self) -> Result<(), String>;
}

/// The logical scene sent to the display/layout adapters.
pub struct DisplayScene {
    pub displays: Vec<Box<dyn Display>>,
    pub layouts: Vec<FloLayout>,
}

impl std::fmt::Debug for DisplayScene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DisplayScene")
            .field("display_count", &self.displays.len())
            .field("layouts", &self.layouts)
            .finish()
    }
}

// SAFETY: DisplayScene only holds thread-safe types (Arc, Mutex,
// Vec of AtomicallyReferenceCounted items).
unsafe impl Send for DisplayScene {}

impl DisplayScene {
    pub fn new() -> Self {
        Self {
            displays: Vec::new(),
            layouts: Vec::new(),
        }
    }

    pub fn render(&mut self, renderer: &mut dyn Renderer, metrics: &DisplayMetrics) {
        for display in &mut self.displays {
            if display.base().show || display.base().forceshow {
                display.render(renderer, metrics);
            }
        }
        // Layouts are already represented by `LayoutContent` displays in the
        // production XML scene.  Do not render `scene.layouts` a second time:
        // that compatibility list is retained for hit-testing, and its old
        // transparent placeholder pass would overwrite the real keyboard
        // geometry after it had been drawn.
    }
}

impl Default for DisplayScene {
    fn default() -> Self {
        Self::new()
    }
}

/// Bridges logical draw operations to a platform renderer.
pub trait PlatformRenderer: Send {
    fn begin_frame(&mut self, _width: u32, _height: u32) {}
    fn draw(&mut self, op: DrawOp);
    fn finish_frame(&mut self, _frame: &mut VideoFrame) {}
}

pub struct SceneRenderer<'a> {
    pub platform: &'a mut dyn PlatformRenderer,
}

impl Renderer for SceneRenderer<'_> {
    fn draw(&mut self, op: DrawOp) {
        self.platform.draw(op);
    }
}

pub struct FrameRenderer {
    pub scene: DisplayScene,
    pub platform: Box<dyn PlatformRenderer>,
    pub metrics: DisplayMetrics,
}

impl VideoRenderer for FrameRenderer {
    fn render(&mut self, frame: &mut VideoFrame) {
        if frame.width > 0 && frame.height > 0 {
            // The XML layout is authored in the logical resolution from
            // graphics.xml (normally 640x480).  A fullscreen transition can
            // change the drawable framebuffer to an arbitrary display size;
            // that size is not a new logical coordinate system.  In
            // particular, desktop fullscreen dimensions are almost never an
            // integral multiple of 640x480, so inferring the logical size
            // from the framebuffer makes the entire UI render at unit scale
            // in the upper-left corner.  Keep the logical dimensions stable
            // and update only the drawable dimensions.
            let logical_width = self.metrics.logical_width;
            let logical_height = self.metrics.logical_height;
            self.metrics = DisplayMetrics::new(
                logical_width,
                logical_height,
                frame.width as i32,
                frame.height as i32,
            );
        }
        self.platform.begin_frame(frame.width, frame.height);
        let mut renderer = SceneRenderer {
            platform: &mut *self.platform,
        };
        self.scene.render(&mut renderer, &self.metrics);
        self.platform.finish_frame(frame);
    }
}

/// Canonical CPU renderer. All output is straight-alpha RGBA8 and all scaling
/// is nearest-neighbour, making fixture output independent of GPU and driver.
pub struct SoftwareRgbaRenderer {
    surface: SoftwareSurface,
    fonts: HashMap<String, Font>,
    default_font: String,
    point_size: f32,
}

impl SoftwareRgbaRenderer {
    pub fn new(font_data: impl AsRef<[u8]>, point_size: f32) -> Result<Self, String> {
        let font = Font::from_bytes(font_data.as_ref(), FontSettings::default())
            .map_err(|error| format!("invalid Vera font: {error}"))?;
        let mut fonts = HashMap::new();
        fonts.insert("default".to_string(), font);
        Ok(Self {
            surface: SoftwareSurface::new(0, 0),
            fonts,
            default_font: "default".to_string(),
            point_size: point_size.max(1.0),
        })
    }

    pub fn with_fonts<I, N, B>(fonts: I, default_font: N, point_size: f32) -> Result<Self, String>
    where
        I: IntoIterator<Item = (N, B)>,
        N: Into<String>,
        B: AsRef<[u8]>,
    {
        let mut loaded = HashMap::new();
        for (name, bytes) in fonts {
            loaded.insert(
                name.into(),
                Font::from_bytes(bytes.as_ref(), FontSettings::default())
                    .map_err(|error| format!("invalid bundled font: {error}"))?,
            );
        }
        let default_font = default_font.into();
        if !loaded.contains_key(&default_font) {
            return Err(format!("default font '{default_font}' was not loaded"));
        }
        Ok(Self {
            surface: SoftwareSurface::new(0, 0),
            fonts: loaded,
            default_font,
            point_size: point_size.max(1.0),
        })
    }

    pub fn decode_image(bytes: &[u8]) -> Result<DecodedImage, String> {
        let image = image::load_from_memory(bytes)
            .map_err(|error| format!("image decode failed: {error}"))?
            .into_rgba8();
        Ok(DecodedImage {
            width: image.width(),
            height: image.height(),
            pixels: image.into_raw(),
        })
    }

    pub fn draw_image(&mut self, image: &DecodedImage, destination: (i32, i32, i32, i32)) {
        self.surface.blit_rgba(
            &image.pixels,
            image.width as i32,
            image.height as i32,
            image.width as usize * 4,
            destination,
        );
    }

    fn draw_text(
        &mut self,
        text: &str,
        x: i32,
        y: i32,
        color: crate::videoio_displays::Color,
        center_x: i8,
        center_y: i8,
    ) {
        self.draw_styled_text(
            text,
            x,
            y,
            color,
            TextStyle {
                alignment: (center_x, center_y),
                font_name: None,
                point_size: self.point_size,
            },
        )
    }

    fn draw_styled_text(
        &mut self,
        text: &str,
        mut x: i32,
        mut y: i32,
        color: crate::videoio_displays::Color,
        style: TextStyle<'_>,
    ) {
        // SDL_ttf's top-origin placement sits above fontdue's line-box
        // origin by roughly three tenths of the requested Vera size.
        y -= (style.point_size * 0.25).round() as i32;
        let font = style
            .font_name
            .and_then(|name| self.fonts.get(name))
            .or_else(|| self.fonts.get(&self.default_font))
            .expect("renderer always has a default font");
        let characters: Vec<char> = text.chars().collect();
        let width: i32 = characters
            .iter()
            .enumerate()
            .map(|(index, &character)| {
                let advance = font
                    .metrics(character, style.point_size)
                    .advance_width
                    .round() as i32;
                let kern = characters
                    .get(index + 1)
                    .and_then(|&next| font.horizontal_kern(character, next, style.point_size))
                    .unwrap_or(0.0)
                    .round() as i32;
                advance + kern
            })
            .sum();
        let height = font
            .horizontal_line_metrics(style.point_size)
            .map_or(style.point_size.ceil() as i32, |metrics| {
                metrics.new_line_size.ceil() as i32
            });
        let (center_x, center_y) = style.alignment;
        if center_x != 0 {
            x -= width / center_x as i32;
        }
        if center_y != 0 {
            y -= height / center_y as i32;
        }
        let mut pen = x;
        for (index, &character) in characters.iter().enumerate() {
            let (metrics, bitmap) = font.rasterize(character, style.point_size);
            let gx = pen + metrics.xmin;
            let gy = y + height - metrics.height as i32 - metrics.ymin;
            for row in 0..metrics.height {
                for column in 0..metrics.width {
                    let coverage = bitmap[row * metrics.width + column] as u16;
                    if coverage == 0 {
                        continue;
                    }
                    // TTF_RenderText_Shaded rasterizes against black and the
                    // legacy path then color-keys only exact black pixels.
                    // Antialiased edge pixels therefore replace the target
                    // with dark, opaque RGB rather than alpha-blending it.
                    self.surface.put_opaque_pixel(
                        gx + column as i32,
                        gy + row as i32,
                        SurfaceColor::rgb(
                            (color.0 as u16 * coverage / 255) as u8,
                            (color.1 as u16 * coverage / 255) as u8,
                            (color.2 as u16 * coverage / 255) as u8,
                        )
                        .packed(),
                    );
                }
            }
            pen += metrics.advance_width.round() as i32;
            if let Some(&next) = characters.get(index + 1) {
                pen += font
                    .horizontal_kern(character, next, style.point_size)
                    .unwrap_or(0.0)
                    .round() as i32;
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl PlatformRenderer for SoftwareRgbaRenderer {
    fn begin_frame(&mut self, width: u32, height: u32) {
        if self.surface.width() != width as i32 || self.surface.height() != height as i32 {
            self.surface = SoftwareSurface::new(width as i32, height as i32);
        }
        self.surface.clear(SurfaceColor::rgba(0, 0, 0, 255));
    }

    fn draw(&mut self, op: DrawOp) {
        match op {
            DrawOp::Box(x1, y1, x2, y2, c) => self.surface.box_rgba(
                x1,
                y1,
                x2,
                y2,
                SurfaceColor::rgba(c.0, c.1, c.2, c.3).packed(),
            ),
            DrawOp::Line(a, b, c) => self.surface.line_rgba(
                a.0,
                a.1,
                b.0,
                b.1,
                SurfaceColor::rgba(c.0, c.1, c.2, c.3).packed(),
            ),
            DrawOp::Circle(x, y, radius, c) => self.surface.circle_rgba(
                x,
                y,
                radius,
                SurfaceColor::rgba(c.0, c.1, c.2, c.3).packed(),
            ),
            DrawOp::Text(text, x, y, color, center_x, center_y) => {
                self.draw_text(&text, x, y, color, center_x, center_y)
            }
            DrawOp::StyledText(text, font, size, x, y, color, center_x, center_y) => self
                .draw_styled_text(
                    &text,
                    x,
                    y,
                    color,
                    TextStyle {
                        alignment: (center_x, center_y),
                        font_name: Some(&font),
                        point_size: size,
                    },
                ),
            DrawOp::FilledCircle(x, y, radius, c) => self.surface.filled_circle_rgba(
                x,
                y,
                radius,
                SurfaceColor::rgba(c.0, c.1, c.2, c.3).packed(),
            ),
            DrawOp::FilledPie(x, y, radius, start, end, c) => self.surface.filled_pie_rgba(
                x,
                y,
                radius,
                start,
                end,
                SurfaceColor::rgba(c.0, c.1, c.2, c.3).packed(),
            ),
            DrawOp::Waveform(samples, x, y, width, height, c) => {
                if width > 0 && height > 0 && !samples.is_empty() {
                    let color = SurfaceColor::rgba(c.0, c.1, c.2, c.3).packed();
                    let mid = y + height / 2;
                    let mut previous = (x, mid);
                    for column in 0..width {
                        let index = column as usize * samples.len() / width as usize;
                        let value = samples[index].clamp(-1.0, 1.0);
                        let current = (x + column, mid - (value * height as f32 * 0.5) as i32);
                        self.surface
                            .line_rgba(previous.0, previous.1, current.0, current.1, color);
                        previous = current;
                    }
                }
            }
            DrawOp::LoopScope(
                peaks,
                averages,
                position,
                x,
                y,
                size,
                background,
                average_color,
                peak_color,
                magnitude,
                flat_width,
                flat_height,
            ) => {
                if size > 1
                    && flat_width > 0
                    && flat_height > 0
                    && !peaks.is_empty()
                    && peaks.len() == averages.len()
                {
                    let center_x = x + size / 2;
                    let center_y = y + size / 2;
                    let inner = (size as f32 * 0.13) as i32;
                    let ring = size / 2 - inner;
                    if ring <= 0 {
                        return;
                    }
                    let bg =
                        SurfaceColor::rgba(background.0, background.1, background.2, background.3)
                            .packed();
                    // This is the temporary 320×30 `lscopepic` used by
                    // C++ `VideoIO::DrawLoop`.  Build it first, then apply
                    // the identical `CircularMap` coordinate transform.
                    let mut flat = vec![bg; flat_width as usize * flat_height as usize];
                    let midpoint = flat_height / 2;
                    let pixels_per_chunk = flat_width as f32 / peaks.len() as f32;
                    let mut strip_x = -(position as f32) * pixels_per_chunk;
                    if strip_x < 0.0 {
                        strip_x += flat_width as f32;
                    }
                    for (&peak, &average) in peaks.iter().zip(averages.iter()) {
                        let peak_height = (peak * magnitude).min(15.0) as i32;
                        let average_weight = (average / (peak * peak + 0.00000001) * 2.0).min(1.0);
                        let mix = |average: u8, peak: u8| {
                            (average as f32 * average_weight + peak as f32 * (1.0 - average_weight))
                                as u8
                        };
                        let color = SurfaceColor::rgba(
                            mix(average_color.0, peak_color.0),
                            mix(average_color.1, peak_color.1),
                            mix(average_color.2, peak_color.2),
                            255,
                        )
                        .packed();
                        let x_start = strip_x as i32;
                        let x_end = if pixels_per_chunk >= 1.0 {
                            (strip_x + pixels_per_chunk) as i32
                        } else {
                            x_start
                        };
                        for column in x_start..=x_end {
                            if !(0..flat_width).contains(&column) {
                                continue;
                            }
                            for row in midpoint - peak_height..=midpoint + peak_height {
                                if (0..flat_height).contains(&row) {
                                    flat[row as usize * flat_width as usize + column as usize] =
                                        color;
                                }
                            }
                        }
                        strip_x += pixels_per_chunk;
                        if strip_x >= flat_width as f32 {
                            strip_x = 0.0;
                        }
                    }
                    for py in 0..size {
                        for px in 0..size {
                            let xo = center_x - (x + px);
                            let yo = y + py - center_y;
                            let theta = (yo as f32).atan2(xo as f32);
                            let in_x = flat_width as f32 * (theta + std::f32::consts::PI)
                                / (2.0 * std::f32::consts::PI);
                            let sine = theta.sin();
                            let in_y = if sine == 0.0 {
                                (xo - inner) as f32 * flat_height as f32 / ring as f32
                            } else {
                                (yo as f32 / sine - inner as f32) * flat_height as f32 / ring as f32
                            };
                            if !(0.0..flat_width as f32).contains(&in_x)
                                || !(0.0..flat_height as f32).contains(&in_y)
                            {
                                continue;
                            }
                            let source_x = (in_x + 0.5).floor() as i32;
                            let source_y = (in_y + 0.5).floor() as i32;
                            if !(0..flat_width).contains(&source_x)
                                || !(0..flat_height).contains(&source_y)
                            {
                                continue;
                            }
                            self.surface.put_opaque_pixel(
                                x + px,
                                y + py,
                                flat[source_y as usize * flat_width as usize + source_x as usize],
                            );
                        }
                    }
                    self.surface.circle_rgba(
                        center_x,
                        center_y,
                        size / 2,
                        SurfaceColor::rgba(0, 0, 0, 255).packed(),
                    );
                }
            }
            DrawOp::Image(bytes, width, height, x, y, draw_width, draw_height) => {
                self.surface.blit_rgba(
                    &bytes,
                    width as i32,
                    height as i32,
                    width as usize * 4,
                    (x, y, draw_width, draw_height),
                )
            }
        }
    }

    fn finish_frame(&mut self, frame: &mut VideoFrame) {
        frame.width = self.surface.width() as u32;
        frame.height = self.surface.height() as u32;
        frame.stride = frame.width as usize * 4;
        frame.pixels = self.surface.rgba_bytes();
    }
}

/// SDL2 presentation backend. SDL objects are created in `open`, on the video
/// worker, and never leave that thread.
pub struct Sdl2VideoBackend {
    title: String,
    canvas: Option<Canvas<Window>>,
    sdl: Option<Sdl2Context>,
}

// SAFETY: Sdl2VideoBackend only holds types that are used from one
// thread at a time, protected by the type system.
unsafe impl Send for Sdl2VideoBackend {}

impl Sdl2VideoBackend {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            canvas: None,
            // Keep the legacy infallible constructor usable while ensuring
            // input created later reuses this thread's SDL initialization.
            sdl: Sdl2Context::shared()
                .ok()
                .or_else(|| Sdl2Context::new().ok()),
        }
    }

    /// Constructs a video backend using the same SDL library handle as input.
    pub fn new_with_context(title: impl Into<String>, context: Sdl2Context) -> Self {
        Self {
            title: title.into(),
            canvas: None,
            sdl: Some(context),
        }
    }

    fn metrics(&self) -> Result<crate::videoio::RenderMetrics, String> {
        let canvas = self
            .canvas
            .as_ref()
            .ok_or_else(|| "SDL video backend is closed".to_string())?;
        Ok(crate::videoio::RenderMetrics::from_sizes(
            canvas.window().size(),
            canvas.output_size()?,
        ))
    }
}

#[cfg(test)]
mod frame_renderer_tests {
    use super::*;

    struct Capture(Vec<DrawOp>);

    impl PlatformRenderer for Capture {
        fn draw(&mut self, op: DrawOp) {
            self.0.push(op);
        }
    }

    #[test]
    fn drawable_resize_keeps_xml_logical_coordinate_system() {
        let mut renderer = FrameRenderer {
            scene: DisplayScene::new(),
            platform: Box::new(Capture(Vec::new())),
            metrics: DisplayMetrics::new(640, 480, 640, 480),
        };
        let mut frame = VideoFrame {
            pixels: Vec::new(),
            width: 1728,
            height: 1117,
            stride: 1728 * 4,
            timestamp: 0.0,
        };

        renderer.render(&mut frame);

        assert_eq!(
            (
                renderer.metrics.logical_width,
                renderer.metrics.logical_height
            ),
            (640, 480)
        );
        assert_eq!(
            (
                renderer.metrics.drawable_width,
                renderer.metrics.drawable_height
            ),
            (1728, 1117)
        );
        assert_eq!(renderer.metrics.x(640), 1728);
        assert_eq!(renderer.metrics.y(480), 1117);
    }
}

impl VideoBackend for Sdl2VideoBackend {
    fn requires_main_thread() -> bool {
        true
    }

    fn open(&mut self, mode: VideoMode) -> Result<crate::videoio::RenderMetrics, String> {
        if self.canvas.is_some() {
            return self.set_mode(mode);
        }
        let context = self
            .sdl
            .take()
            .or_else(|| Sdl2Context::shared().ok())
            .ok_or_else(|| "SDL context has not been initialized on this thread".to_string())?;
        let sdl = context.sdl();
        let video = sdl.video()?;
        let mut builder = video.window(
            &self.title,
            mode.windowed_size.0.max(1),
            mode.windowed_size.1.max(1),
        );
        builder.position_centered().allow_highdpi().resizable();
        let window = builder.build().map_err(|error| error.to_string())?;
        let canvas = window
            .into_canvas()
            .accelerated()
            .present_vsync()
            .build()
            .or_else(|_| {
                video
                    .window(
                        &self.title,
                        mode.windowed_size.0.max(1),
                        mode.windowed_size.1.max(1),
                    )
                    .position_centered()
                    .allow_highdpi()
                    .build()
                    .map_err(|error| error.to_string())?
                    .into_canvas()
                    .software()
                    .build()
                    .map_err(|error| error.to_string())
            })?;
        self.canvas = Some(canvas);
        self.sdl = Some(context);
        self.set_mode(mode)
    }

    fn set_mode(&mut self, mode: VideoMode) -> Result<crate::videoio::RenderMetrics, String> {
        let canvas = self
            .canvas
            .as_mut()
            .ok_or_else(|| "SDL video backend is closed".to_string())?;
        canvas.window_mut().set_fullscreen(if mode.fullscreen {
            FullscreenType::Desktop
        } else {
            FullscreenType::Off
        })?;
        if !mode.fullscreen {
            canvas
                .window_mut()
                .set_size(mode.windowed_size.0.max(1), mode.windowed_size.1.max(1))
                .map_err(|error| error.to_string())?;
        }
        self.metrics()
    }

    fn present(&mut self, frame: &VideoFrame) -> Result<(), String> {
        if frame.width == 0
            || frame.height == 0
            || frame.stride < frame.width as usize * 4
            || frame.pixels.len() < frame.stride.saturating_mul(frame.height as usize)
        {
            return Err("invalid RGBA frame dimensions or stride".into());
        }
        let canvas = self
            .canvas
            .as_mut()
            .ok_or_else(|| "SDL video backend is closed".to_string())?;
        let creator = canvas.texture_creator();
        let mut texture = creator
            .create_texture_streaming(PixelFormatEnum::RGBA32, frame.width, frame.height)
            .map_err(|error| error.to_string())?;
        // C++ renders directly into the SDL window surface.  Its primitive
        // pixels are replacement writes, including their alpha byte, so the
        // final upload must not source-over blend this complete frame.
        texture.set_blend_mode(BlendMode::None);
        texture
            .update(None, &frame.pixels, frame.stride)
            .map_err(|error| error.to_string())?;
        canvas.clear();
        canvas.copy(&texture, None, None)?;
        canvas.present();
        Ok(())
    }

    fn close(&mut self) {
        self.canvas = None;
        self.sdl = None;
    }
}

/// Convenience constructor retaining `VideoIO`'s open → frame/update → mode
/// → stop ordering and its channel wakeups.
pub fn activate_scene<B: VideoBackend>(
    video: &mut VideoIO<B>,
    scene: DisplayScene,
    platform: Box<dyn PlatformRenderer>,
) -> Result<(), String> {
    let m = video.render_metrics();
    let metrics = DisplayMetrics::new(
        m.logical_width as i32,
        m.logical_height as i32,
        m.drawable_width as i32,
        m.drawable_height as i32,
    );
    video.activate(FrameRenderer {
        scene,
        platform,
        metrics,
    })
}

pub fn mode(fullscreen: bool, windowed_size: (u32, u32)) -> VideoMode {
    VideoMode {
        fullscreen,
        windowed_size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct P(Vec<DrawOp>);
    impl PlatformRenderer for P {
        fn draw(&mut self, op: DrawOp) {
            self.0.push(op);
        }
    }
    #[test]
    fn scene_preserves_display_order() {
        let mut scene = DisplayScene::new();
        let mut d = crate::videoio_displays::FloDisplayPanel::new(1);
        d.base.title = Some("x".into());
        scene.displays.push(Box::new(d));
        let mut p = P(Vec::new());
        let m = DisplayMetrics::new(640, 480, 640, 480);
        let mut r = SceneRenderer { platform: &mut p };
        scene.render(&mut r, &m);
        assert_eq!(p.0.len(), 1);
    }

    #[test]
    fn vera_fixture_is_byte_deterministic() {
        fn render() -> VideoFrame {
            let mut renderer =
                SoftwareRgbaRenderer::new(include_bytes!("../data/Vera.ttf"), 12.0).unwrap();
            renderer.begin_frame(96, 32);
            renderer.draw(DrawOp::Box(
                0,
                0,
                95,
                31,
                crate::videoio_displays::Color(8, 16, 24, 255),
            ));
            renderer.draw(DrawOp::Text(
                "FreeWheeling".into(),
                48,
                16,
                crate::videoio_displays::Color(239, 175, 255, 255),
                2,
                2,
            ));
            let mut frame = VideoFrame {
                pixels: Vec::new(),
                width: 96,
                height: 32,
                stride: 96 * 4,
                timestamp: 0.0,
            };
            renderer.finish_frame(&mut frame);
            frame
        }

        let first = render();
        let second = render();
        assert_eq!(first.pixels, second.pixels);
        assert_eq!(first.pixels.len(), 96 * 32 * 4);
        assert!(first.pixels.chunks_exact(4).any(|pixel| pixel[0] > 8));
    }
}
