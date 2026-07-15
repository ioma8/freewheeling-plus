//! Production XML-driven user-interface scene.
//!
//! `graphics.xml`, `interfaces.xml`, and the graphics sections of each
//! interface are the source of truth.  This module deliberately retains the
//! declared order: it is part of FreeWheeling's overdraw behaviour.

use super::{DisplayScene, FrameRenderer, PlatformRenderer, SoftwareRgbaRenderer};
use crate::video_layout::{FloLayout, FloLayoutBox, FloLayoutElement};
use crate::videoio_displays::{
    Color, Display, DrawOp, FloDisplay, Orientation, RenderMetrics, Renderer,
};
use fontdue::{Font, FontSettings};
use roxmltree::{Document, Node};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

const FG: Color = Color(0xef, 0xaf, 0xff, 255);
const DIM: Color = Color(0x77, 0x88, 0x99, 255);
const HOT: Color = Color(0xff, 0x50, 0x20, 255);
/// `VERSION` in the checked C++ source (`src/fweelin_config.h`).  This is
/// intentionally the original application's version, rather than Cargo's
/// package version, because the startup animation is source-compatible UI.
const CPP_VERSION: &str = "0.6";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrowserSceneState {
    pub items: Vec<String>,
    /// C++ `LoopTrayItem::loopid`. Normal browsers leave this empty; tray
    /// rows must retain their true (potentially sparse) live slot id.
    pub loop_ids: Vec<i32>,
    pub selected: usize,
    pub expanded: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiSceneState {
    pub values: HashMap<String, f32>,
    pub snapshots: Vec<Option<String>>,
    pub browsers: HashMap<String, BrowserSceneState>,
    pub waveforms: HashMap<i32, Vec<f32>>,
    pub loop_scopes: HashMap<i32, LoopScopeState>,
    /// Mutable C++ `FloLayout` state, changed by `video-show-loop`,
    /// `video-show-layout`, and `video-switch-interface` events.
    pub layouts: HashMap<(i32, i32), LayoutSceneState>,
    pub help_page: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutSceneState {
    pub show: bool,
    pub loopids: (i32, i32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LoopScopeState {
    pub peaks: Vec<f32>,
    pub averages: Vec<f32>,
    pub position_column: u16,
    pub chunk_count: u16,
    pub current_peak: f32,
    pub mode: crate::native_dsp_graph::LoopMode,
    pub gain: f32,
    pub trigger_gain: f32,
    pub gain_delta: f32,
    pub selected: bool,
    pub recent_rank: Option<u8>,
    pub name: Option<String>,
}

impl Default for LoopScopeState {
    fn default() -> Self {
        Self {
            peaks: Vec::new(),
            averages: Vec::new(),
            position_column: 0,
            chunk_count: 0,
            current_peak: 0.0,
            mode: crate::native_dsp_graph::LoopMode::Empty,
            gain: 1.0,
            trigger_gain: 1.0,
            gain_delta: 1.0,
            selected: false,
            recent_rank: None,
            name: None,
        }
    }
}

pub type SharedUiSceneState = Arc<RwLock<UiSceneState>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneManifest {
    pub logical_size: (u32, u32),
    pub frame_delay: Duration,
    pub fonts: BTreeMap<String, (PathBuf, u32)>,
    pub interface_files: Vec<PathBuf>,
    pub display_kinds: BTreeMap<String, usize>,
    pub layout_count: usize,
    pub element_count: usize,
    pub help_lines: Vec<String>,
}

pub struct ProductionUiScene {
    pub scene: DisplayScene,
    pub state: SharedUiSceneState,
    pub manifest: SceneManifest,
}

pub struct ProductionUiRenderer {
    pub renderer: FrameRenderer,
    pub frame_delay: Duration,
}

/// Deadline scheduler with no cumulative drift. Slow frames skip missed
/// deadlines instead of trying to render a burst of stale frames.
#[derive(Clone, Debug)]
pub struct FrameScheduler {
    interval: Duration,
    next: Instant,
}

impl FrameScheduler {
    pub fn new(interval: Duration, now: Instant) -> Self {
        let interval = interval.max(Duration::from_millis(1));
        Self {
            interval,
            next: now,
        }
    }
    pub fn deadline(&self) -> Instant {
        self.next
    }
    pub fn advance(&mut self, now: Instant) -> Instant {
        while self.next <= now {
            self.next += self.interval;
        }
        self.next
    }
}

#[derive(Clone, Debug)]
enum WidgetKind {
    Text,
    Switch,
    TextSwitch {
        off: String,
        on: String,
    },
    Bar {
        switched: bool,
    },
    Circle {
        off: i32,
        on: i32,
    },
    Squares {
        size: (i32, i32),
        lo: f32,
        hi: f32,
        step: f32,
    },
    Panel {
        size: (i32, i32),
    },
    Snapshots {
        size: (i32, i32),
        margin: i32,
    },
    Browser {
        browse_type: String,
        expand: (i32, i32, i32, i32),
        loop_size: i32,
    },
}

#[derive(Clone, Debug)]
struct XmlDisplay {
    base: FloDisplay,
    kind: WidgetKind,
    variable: Option<String>,
    font: String,
    font_size: f32,
    orientation: Orientation,
    bar_scale: i32,
    thickness: i32,
    db_scale: bool,
    max_db: f32,
    state: SharedUiSceneState,
    children: Vec<XmlDisplay>,
}

#[derive(Clone, Debug)]
struct LayoutContent {
    base: FloDisplay,
    layout: FloLayout,
    state: SharedUiSceneState,
    current_peaks: HashMap<i32, CurrentPeakHistory>,
}

/// `VideoIO` owns `curpeakidx`, `lastpeakidx`, and `oldpeak` on the video
/// thread.  Keeping this state alongside each rendered layout makes Rust use
/// the same update cadence instead of treating UI-snapshot delivery as an
/// audio-peak boundary.
#[derive(Clone, Copy, Debug)]
struct CurrentPeakHistory {
    last_index: u16,
    old_peak: f32,
}

impl Default for CurrentPeakHistory {
    fn default() -> Self {
        Self {
            last_index: 0,
            old_peak: 1.0,
        }
    }
}

#[derive(Clone, Debug)]
struct HelpOverlay {
    base: FloDisplay,
    lines: Arc<Vec<String>>,
    state: SharedUiSceneState,
}

#[derive(Clone, Debug)]
struct LogoOverlay {
    base: FloDisplay,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    started: Instant,
    version_width: i32,
    version_height: i32,
    version_size: f32,
}

#[derive(Clone, Debug)]
struct StaticStatusOverlay {
    base: FloDisplay,
}

fn cpp_logo_y(drawable_height: i32, logo_height: i32, elapsed: f32) -> i32 {
    if elapsed < 1.0 {
        (-logo_height as f32 + drawable_height as f32 * elapsed) as i32
    } else {
        drawable_height - logo_height
    }
}

/// C++ `video_event_loop` logo-version phase.  `None` means that the version
/// is not drawn in that phase; the logo image itself remains visible.
fn cpp_logo_version_x(
    drawable_width: i32,
    version_width: i32,
    margin_x: i32,
    elapsed: f32,
) -> Option<i32> {
    let span = version_width + margin_x;
    if elapsed > 2.0 && elapsed < 3.0 {
        Some((drawable_width as f32 - (elapsed - 2.0) * span as f32) as i32)
    } else if (2.0..=4.0).contains(&elapsed) {
        Some(drawable_width - span)
    } else if elapsed > 4.0 && elapsed < 5.0 {
        Some((drawable_width as f32 - (1.0 - (elapsed - 4.0)) * span as f32) as i32)
    } else {
        None
    }
}

fn fontdue_text_metrics(font: &Font, text: &str, size: f32) -> (i32, i32) {
    let chars: Vec<char> = text.chars().collect();
    let width = chars
        .iter()
        .enumerate()
        .map(|(index, &character)| {
            let advance = font.metrics(character, size).advance_width.round() as i32;
            let kern = chars
                .get(index + 1)
                .and_then(|&next| font.horizontal_kern(character, next, size))
                .unwrap_or(0.0)
                .round() as i32;
            advance + kern
        })
        .sum();
    let height = font
        .horizontal_line_metrics(size)
        .map_or(size.ceil() as i32, |metrics| {
            metrics.new_line_size.ceil() as i32
        });
    (width, height)
}

#[derive(Clone, Debug)]
struct PulseOverlay {
    base: FloDisplay,
    state: SharedUiSceneState,
}

impl Display for PulseOverlay {
    fn base(&self) -> &FloDisplay {
        &self.base
    }

    fn base_mut(&mut self) -> &mut FloDisplay {
        &mut self.base
    }

    fn render(&mut self, renderer: &mut dyn Renderer, metrics: &RenderMetrics) {
        let state = self.state.read().expect("UI state poisoned");
        if state.values.get("pulse-active").copied().unwrap_or(0.0) == 0.0 {
            return;
        }
        let frames = state
            .values
            .get("pulse-frames")
            .copied()
            .unwrap_or(1.0)
            .max(1.0);
        let position = state.values.get("pulse-position").copied().unwrap_or(0.0);
        let end = (360.0 * (position / frames).clamp(0.0, 1.0)) as i32;
        let long_count = state
            .values
            .get("pulse-long-count")
            .copied()
            .unwrap_or(0.0)
            .max(0.0) as u32;
        let long_length = state
            .values
            .get("pulse-long-length")
            .copied()
            .unwrap_or(1.0)
            .max(1.0) as u32;
        // Exact logical geometry from VideoIO::video_event_loop: pulse zero is
        // at (600,30), and the selected pulse is twice the 10-pixel base size.
        let x = metrics.x(600);
        let y = metrics.y(30);
        let radius = metrics.x(10) * 2;
        // `VideoIO` first draws completed long-count wedges at 1.3× the
        // selected-pulse radius. The following black disk hides their inner
        // area, leaving the orange outer ring visible.
        let mut break_angle = 7.0_f32;
        let theta_length = 360.0 / long_length as f32;
        while theta_length < break_angle {
            break_angle /= 1.5;
        }
        for beat in 0..long_count.min(long_length) {
            let theta = beat as f32 * theta_length;
            renderer.draw(DrawOp::FilledPie(
                x,
                y,
                (radius as f32 * 1.3) as i32,
                theta.round() as i32,
                (theta + theta_length - break_angle).round() as i32,
                Color(255, 188, 0, 180),
            ));
        }
        renderer.draw(DrawOp::FilledPie(x, y, radius, 0, 359, Color(0, 0, 0, 255)));
        renderer.draw(DrawOp::FilledPie(
            x,
            y,
            radius,
            0,
            end,
            Color(127, 127, 127, 255),
        ));
        renderer.draw(DrawOp::StyledText(
            "1".into(),
            "main".into(),
            12.0 * metrics.scale_y,
            x - metrics.x(10),
            y - metrics.y(10),
            Color(255, 0, 0, 255),
            0,
            0,
        ));
    }
}

impl Display for StaticStatusOverlay {
    fn base(&self) -> &FloDisplay {
        &self.base
    }
    fn base_mut(&mut self) -> &mut FloDisplay {
        &mut self.base
    }
    fn render(&mut self, renderer: &mut dyn Renderer, metrics: &RenderMetrics) {
        renderer.draw(DrawOp::FilledPie(
            metrics.x(18),
            metrics.y(450),
            metrics.x(8),
            30,
            359,
            Color(0xf9, 0xe6, 0x13, 255),
        ));
        renderer.draw(DrawOp::Circle(
            metrics.x(18),
            metrics.y(450),
            metrics.x(8),
            Color(40, 40, 40, 255),
        ));
        renderer.draw(DrawOp::StyledText(
            "stream off".into(),
            "main".into(),
            20.0 * metrics.scale_y,
            metrics.x(35),
            metrics.y(443),
            DIM,
            0,
            0,
        ));
    }
}

impl Display for LogoOverlay {
    fn base(&self) -> &FloDisplay {
        &self.base
    }
    fn base_mut(&mut self) -> &mut FloDisplay {
        &mut self.base
    }
    fn render(&mut self, renderer: &mut dyn Renderer, metrics: &RenderMetrics) {
        let width = metrics.x(self.width as i32);
        let height = metrics.y(self.height as i32);
        // C++ VideoIO slides the logo down from directly above the drawable
        // surface during `0.0 <= video_elapsed < 1.0`; every later phase
        // keeps the image at this same bottom-right location.
        let elapsed = self.started.elapsed().as_secs_f32();
        let y = cpp_logo_y(metrics.drawable_height, height, elapsed);
        renderer.draw(DrawOp::Image(
            self.pixels.clone(),
            self.width,
            self.height,
            metrics.drawable_width - width,
            y,
            width,
            height,
        ));
        let margin_x = metrics.x(5);
        let margin_y = metrics.y(5);
        let version_width = metrics.x(self.version_width);
        if let Some(version_x) =
            cpp_logo_version_x(metrics.drawable_width, version_width, margin_x, elapsed)
        {
            renderer.draw(DrawOp::StyledText(
                CPP_VERSION.into(),
                "main".into(),
                self.version_size * metrics.scale_y,
                version_x,
                metrics.drawable_height - metrics.y(self.version_height) - margin_y,
                Color(255, 255, 255, 255),
                0,
                0,
            ));
        }
    }
}

impl Display for HelpOverlay {
    fn base(&self) -> &FloDisplay {
        &self.base
    }
    fn base_mut(&mut self) -> &mut FloDisplay {
        &mut self.base
    }
    fn render(&mut self, renderer: &mut dyn Renderer, metrics: &RenderMetrics) {
        let page = self.state.read().expect("UI state poisoned").help_page;
        if page == 0 || self.lines.is_empty() {
            return;
        }
        let start = (page - 1).saturating_mul(24);
        renderer.draw(DrawOp::Box(
            metrics.x(24),
            metrics.y(20),
            metrics.x(616),
            metrics.y(460),
            Color(0, 0, 0, 235),
        ));
        for (row, line) in self.lines.iter().skip(start).take(24).enumerate() {
            renderer.draw(DrawOp::StyledText(
                line.clone(),
                "help".into(),
                16.0,
                metrics.x(38),
                metrics.y(44 + row as i32 * 17),
                if line.starts_with("__") { FG } else { DIM },
                0,
                1,
            ));
        }
    }
}

impl Display for LayoutContent {
    fn base(&self) -> &FloDisplay {
        &self.base
    }
    fn base_mut(&mut self) -> &mut FloDisplay {
        &mut self.base
    }
    fn render(&mut self, renderer: &mut dyn Renderer, metrics: &RenderMetrics) {
        let state = self.state.read().expect("UI state poisoned");
        let layout_state = state.layouts.get(&(self.layout.iid, self.layout.id));
        if !layout_state.map_or(self.layout.show, |layout| layout.show) {
            return;
        }
        let loop_base = layout_state.map_or(self.layout.loopids.0, |layout| layout.loopids.0);
        const LOOP_COLORS: [[Color; 4]; 4] = [
            [
                Color(0x5f, 0x7c, 0x2b, 255),
                Color(0xd3, 0xff, 0x82, 255),
                Color(0xff, 0xff, 0xff, 255),
                Color(0xde, 0xe2, 0xd5, 255),
            ],
            [
                Color(0x8e, 0x75, 0x62, 255),
                Color(0xff, 0x9c, 0x4c, 255),
                Color(0xff, 0xff, 0xff, 255),
                Color(0xe0, 0xda, 0xd5, 255),
            ],
            [
                Color(0x62, 0x8c, 0x85, 255),
                Color(0x43, 0xf2, 0xd5, 255),
                Color(0xff, 0xff, 0xff, 255),
                Color(0xa9, 0xc6, 0xc1, 255),
            ],
            [
                Color(0x69, 0x4b, 0x89, 255),
                Color(0xa8, 0x56, 0xff, 255),
                Color(0xff, 0xff, 0xff, 255),
                Color(0xdf, 0xcb, 0xf4, 255),
            ],
        ];
        const SELECTED: [Color; 4] = [
            Color(0xf9, 0xe6, 0x13, 255),
            Color(0x62, 0x62, 0x62, 255),
            Color(0xff, 0xff, 0xff, 255),
            Color(0xe0, 0xda, 0xd5, 255),
        ];
        let scaled = |color: Color, magnitude: f32, alpha: u8| {
            Color(
                (color.0 as f32 * magnitude) as u8,
                (color.1 as f32 * magnitude) as u8,
                (color.2 as f32 * magnitude) as u8,
                alpha,
            )
        };
        // `VideoIO::DrawLoop` attenuates both the mapped scope and its
        // progress pie by AutoLimitProcessor's current shared gain (`lvol`).
        // The audio snapshot publishes the same system variable.
        let limiter_gain = state
            .values
            .get("SYSTEM_cur_limiter_gain")
            .copied()
            .unwrap_or(1.0)
            .max(0.0);
        for element in &self.layout.elements {
            let loop_id = loop_base + element.id;
            let visual = state.loop_scopes.get(&loop_id);
            let magnitude = visual
                .map(|loop_state| (0.5 + loop_state.trigger_gain).min(1.0))
                .unwrap_or(0.5);
            let palette = visual
                .filter(|loop_state| loop_state.selected)
                .map_or(&LOOP_COLORS[loop_id.rem_euclid(4) as usize], |_| &SELECTED);
            let color = visual
                .filter(|loop_state| loop_state.selected)
                .map_or(scaled(palette[0], magnitude, 255), |_| SELECTED[0]);
            for geometry in &element.geometry {
                geometry.render(renderer, metrics, color);
            }
            if let Some(scope) = visual {
                let size = metrics.x(element.loopsize).max(2);
                let x = metrics.x(element.loopx) - size / 2;
                let y = metrics.y(element.loopy) - size / 2;
                // Exact `DrawLoop` current-peak update: only newly entered
                // 500-frame columns affect the pulse; otherwise retain the
                // previous value.  On a loop wrap C++ starts scanning at zero.
                let history = self.current_peaks.entry(loop_id).or_default();
                let current_peak = if scope.position_column == history.last_index {
                    history.old_peak
                } else {
                    let start = if scope.position_column < history.last_index {
                        0
                    } else {
                        usize::from(history.last_index)
                    };
                    let end = usize::from(scope.position_column).min(scope.peaks.len());
                    let peak = scope.peaks[start.min(end)..end]
                        .iter()
                        .fold(0.0_f32, |maximum, value| maximum.max(*value * scope.gain));
                    let value = peak * 2.0 + 0.5;
                    history.last_index = scope.position_column;
                    history.old_peak = value;
                    value
                };
                let pulse_magnitude = current_peak;
                let waveform_magnitude = limiter_gain * scope.gain * 20.0 * pulse_magnitude;
                let chunks = usize::from(scope.chunk_count).min(scope.peaks.len());
                renderer.draw(DrawOp::LoopScope(
                    scope.peaks[..chunks].to_vec(),
                    scope.averages[..chunks].to_vec(),
                    scope.position_column,
                    x,
                    y,
                    size,
                    scaled(palette[0], magnitude, 255),
                    scaled(palette[1], magnitude, 255),
                    scaled(palette[2], magnitude, 255),
                    waveform_magnitude,
                    metrics.x(320),
                    metrics.y(30),
                ));
                let center_x = x + size / 2;
                let center_y = y + size / 2;
                let pie_radius = ((limiter_gain * metrics.x(20) as f32 * pulse_magnitude) as i32)
                    .min(metrics.x(70));
                let progress =
                    360 * i32::from(scope.position_column) / i32::from(scope.chunk_count.max(1));
                renderer.draw(DrawOp::FilledPie(
                    center_x,
                    center_y,
                    pie_radius,
                    0,
                    progress,
                    scaled(palette[3], magnitude, 127),
                ));

                let trigger_bar = (size as f32 * 0.45 * scope.trigger_gain) as i32;
                renderer.draw(DrawOp::Box(
                    center_x - trigger_bar,
                    center_y - size / 10,
                    center_x + trigger_bar,
                    center_y + size / 10,
                    Color((palette[3].0 as f32 * magnitude) as u8, 0, 0, 127),
                ));
                let delta_bar = ((scope.gain_delta - 1.0) * size as f32 * 250.0) as i32;
                renderer.draw(DrawOp::Box(
                    center_x,
                    center_y + size / 8,
                    center_x + delta_bar,
                    center_y + size / 4,
                    Color((palette[3].0 as f32 * magnitude) as u8, 0, 0, 127),
                ));
                if scope.mode == crate::native_dsp_graph::LoopMode::Overdubbing {
                    renderer.draw(DrawOp::StyledText(
                        "O".into(),
                        "main".into(),
                        20.0 * metrics.scale_y,
                        center_x,
                        center_y,
                        HOT,
                        1,
                        1,
                    ));
                }
                if let Some(name) = &scope.name {
                    renderer.draw(DrawOp::StyledText(
                        name.clone(),
                        "small".into(),
                        14.0 * metrics.scale_y,
                        x,
                        y + size,
                        FG,
                        0,
                        2,
                    ));
                }
                if let Some(rank) = scope.recent_rank {
                    renderer.draw(DrawOp::StyledText(
                        format!("L{}", rank + 1),
                        "small".into(),
                        14.0 * metrics.scale_y,
                        x + size,
                        y,
                        FG,
                        2,
                        0,
                    ));
                }
            }
            if self.layout.showelabel
                && let Some(name) = &element.name
            {
                renderer.draw(DrawOp::StyledText(
                    name.clone(),
                    "main".into(),
                    20.0 * metrics.scale_y,
                    metrics.x(self.layout.xpos + element.nxpos),
                    metrics.y(self.layout.ypos + element.nypos),
                    FG,
                    0,
                    0,
                ));
            }
        }
        if self.layout.showlabel
            && let Some(name) = &self.layout.name
        {
            renderer.draw(DrawOp::StyledText(
                name.clone(),
                "main".into(),
                20.0 * metrics.scale_y,
                metrics.x(self.layout.xpos + self.layout.nxpos),
                metrics.y(self.layout.ypos + self.layout.nypos),
                FG,
                0,
                0,
            ));
        }
    }
}

impl XmlDisplay {
    fn value(&self, state: &UiSceneState) -> f32 {
        self.variable
            .as_ref()
            .map_or(0.0, |expression| evaluate(expression, &state.values))
    }
    fn text(&self, r: &mut dyn Renderer, text: String, x: i32, y: i32, color: Color, scale: f32) {
        r.draw(DrawOp::StyledText(
            text,
            self.font.clone(),
            self.font_size * scale,
            x,
            y,
            color,
            0,
            0,
        ));
    }
    fn render_at(&mut self, r: &mut dyn Renderer, m: &RenderMetrics, offset: (i32, i32)) {
        if !self.base.show && !self.base.forceshow {
            return;
        }
        let x = m.x(offset.0 + self.base.xpos);
        let y = m.y(offset.1 + self.base.ypos);
        let guard = self.state.read().expect("UI state poisoned");
        let value = self.value(&guard);
        match &self.kind {
            WidgetKind::Text => {
                let text = format_value(value);
                let title = self.base.title.clone().unwrap_or_default();
                self.text(r, format!("{title}{text}"), x, y, FG, m.scale_y);
            }
            WidgetKind::Switch => {
                if let Some(title) = self.base.title.clone() {
                    self.text(
                        r,
                        title,
                        x,
                        y,
                        if value != 0.0 {
                            FG
                        } else {
                            Color(0x11, 0x22, 0x33, 255)
                        },
                        m.scale_y,
                    );
                }
            }
            WidgetKind::TextSwitch { off, on } => self.text(
                r,
                if value != 0.0 {
                    on.clone()
                } else {
                    off.clone()
                },
                x,
                y,
                if value != 0.0 { FG } else { DIM },
                m.scale_y,
            ),
            WidgetKind::Bar { switched } => {
                let normalized = if self.db_scale {
                    let db = if value > 0.0 {
                        20.0 * value.log10()
                    } else {
                        -60.0
                    };
                    ((db + 60.0) / (self.max_db + 60.0)).clamp(0.0, 1.0)
                } else {
                    value.clamp(0.0, 1.0)
                };
                let length = (self.bar_scale as f32 * normalized) as i32;
                let color = if *switched && value == 0.0 {
                    Color(HOT.0, HOT.1, HOT.2, 127)
                } else {
                    HOT
                };
                if !self.db_scale {
                    let calibration = Color(HOT.0 / 2, HOT.1 / 2, HOT.2 / 2, 255);
                    if self.orientation == Orientation::Vertical {
                        r.draw(DrawOp::Box(
                            x - self.thickness / 2,
                            y,
                            x + self.thickness / 2,
                            y - m.y(self.bar_scale),
                            calibration,
                        ));
                    } else {
                        r.draw(DrawOp::Box(
                            x,
                            y - self.thickness / 2,
                            x + m.x(self.bar_scale),
                            y + self.thickness / 2,
                            calibration,
                        ));
                    }
                }
                if self.orientation == Orientation::Vertical {
                    r.draw(DrawOp::Box(
                        x - self.thickness,
                        y - m.y(length),
                        x + self.thickness,
                        y,
                        color,
                    ));
                } else {
                    r.draw(DrawOp::Box(
                        x,
                        y - self.thickness,
                        x + m.x(length),
                        y + self.thickness,
                        color,
                    ));
                }
                if let Some(title) = self.base.title.clone() {
                    r.draw(DrawOp::StyledText(
                        title,
                        self.font.clone(),
                        self.font_size * m.scale_y,
                        x,
                        y,
                        DIM,
                        if self.orientation == Orientation::Vertical {
                            1
                        } else {
                            2
                        },
                        if self.orientation == Orientation::Horizontal {
                            1
                        } else {
                            0
                        },
                    ));
                }
            }
            WidgetKind::Circle { off, on } => r.draw(DrawOp::FilledCircle(
                x,
                y,
                m.extent(
                    if value != 0.0 { *on } else { *off },
                    m.scale_x.min(m.scale_y),
                ),
                if value != 0.0 {
                    Color(0xdf, 0x20, 0x20, 255)
                } else {
                    Color(0x11, 0x22, 0x33, 255)
                },
            )),
            WidgetKind::Squares { size, lo, hi, step } => {
                let count = ((value - *lo) / step.max(f32::EPSILON))
                    .clamp(0.0, (hi - lo) / step.max(f32::EPSILON))
                    as i32;
                for index in 0..count {
                    r.draw(DrawOp::Box(
                        x + m.x(index * size.0),
                        y,
                        x + m.x(index * size.0 + size.0 - 1),
                        y + m.y(size.1 - 1),
                        HOT,
                    ));
                }
            }
            WidgetKind::Panel { size } => r.draw(DrawOp::Box(
                x,
                y,
                x + m.x(size.0),
                y + m.y(size.1),
                Color(0, 0, 0, 190),
            )),
            WidgetKind::Snapshots { size, margin } => {
                r.draw(DrawOp::Box(
                    x,
                    y,
                    x + m.x(size.0),
                    y + m.y(size.1),
                    Color(0, 0, 0, 190),
                ));
                let border = Color(0xff, 0x50, 0x20, 255);
                r.draw(DrawOp::Line((x, y), (x + m.x(size.0), y), border));
                r.draw(DrawOp::Line(
                    (x, y + m.y(size.1)),
                    (x + m.x(size.0), y + m.y(size.1)),
                    border,
                ));
                r.draw(DrawOp::Line((x, y), (x, y + m.y(size.1)), border));
                r.draw(DrawOp::Line(
                    (x + m.x(size.0), y),
                    (x + m.x(size.0), y + m.y(size.1)),
                    border,
                ));
                for (index, name) in guard.snapshots.iter().enumerate() {
                    let row = y + m.y(*margin + index as i32 * (self.font_size as i32 + 2));
                    if row > y + m.y(size.1) {
                        break;
                    }
                    self.text(
                        r,
                        match name {
                            Some(name) => format!("{:2} {name}", index + 1),
                            // C++ prints just the numeric slot when no
                            // snapshot object exists (and `**` only for an
                            // existing unnamed object, metadata Rust does not
                            // currently retain).
                            None => format!("{:2}", index + 1),
                        },
                        x + m.x(*margin),
                        row,
                        if index == guard.help_page { FG } else { DIM },
                        m.scale_y,
                    );
                }
            }
            WidgetKind::Browser {
                browse_type,
                expand,
                loop_size,
            } => {
                let browser = guard.browsers.get(browse_type);
                if let Some(browser) = browser {
                    if browse_type == "BROWSE_loop_tray" {
                        render_loop_tray(
                            r,
                            m,
                            &guard,
                            browser,
                            *expand,
                            *loop_size,
                            x,
                            y,
                            &self.font,
                            self.font_size,
                        );
                    } else {
                        if browser.expanded {
                            r.draw(DrawOp::Box(
                                m.x(expand.0),
                                m.y(expand.1),
                                m.x(expand.2),
                                m.y(expand.3),
                                // C++ dims this region with alpha 200 before
                                // its opaque border and selection strip.
                                Color(0, 0, 0, 200),
                            ));
                            let border = Color(127, 127, 127, 255);
                            r.draw(DrawOp::Line(
                                (m.x(expand.0), m.y(expand.1)),
                                (m.x(expand.2), m.y(expand.1)),
                                border,
                            ));
                            r.draw(DrawOp::Line(
                                (m.x(expand.0), m.y(expand.3)),
                                (m.x(expand.2), m.y(expand.3)),
                                border,
                            ));
                            r.draw(DrawOp::Line(
                                (m.x(expand.0), m.y(expand.1)),
                                (m.x(expand.0), m.y(expand.3)),
                                border,
                            ));
                            r.draw(DrawOp::Line(
                                (m.x(expand.2), m.y(expand.1)),
                                (m.x(expand.2), m.y(expand.3)),
                                border,
                            ));
                            let line_height = (self.font_size * 1.2).round() as i32;
                            let center = (expand.1 + expand.3) / 2;
                            r.draw(DrawOp::Box(
                                m.x(expand.0),
                                m.y(center),
                                m.x(expand.2),
                                m.y(center + line_height),
                                Color(127, 0, 0, 255),
                            ));
                            if let Some(current) = browser.items.get(browser.selected) {
                                // `Browser::Draw` starts at `cur`, then
                                // walks `prev` upward and `next` downward.
                                self.text(
                                    r,
                                    current.clone(),
                                    m.x(expand.0),
                                    m.y(center),
                                    FG,
                                    m.scale_y,
                                );
                                let spread = ((center - expand.1).min(expand.3 - center)
                                    / line_height.max(1))
                                .max(0) as usize;
                                for distance in 1..=spread {
                                    let Some(index) = browser.selected.checked_sub(distance) else {
                                        break;
                                    };
                                    self.text(
                                        r,
                                        browser.items[index].clone(),
                                        m.x(expand.0),
                                        m.y(center - distance as i32 * line_height),
                                        FG,
                                        m.scale_y,
                                    );
                                }
                                for distance in 1..spread {
                                    let index = browser.selected + distance;
                                    let Some(item) = browser.items.get(index) else {
                                        break;
                                    };
                                    self.text(
                                        r,
                                        item.clone(),
                                        m.x(expand.0),
                                        m.y(center + distance as i32 * line_height),
                                        FG,
                                        m.scale_y,
                                    );
                                }
                            }
                        }
                        if let Some(item) = browser.items.get(browser.selected) {
                            self.text(r, item.clone(), x, y, FG, m.scale_y);
                        }
                    }
                }
            }
        }
        drop(guard);
        let child_offset = (offset.0 + self.base.xpos, offset.1 + self.base.ypos);
        for child in &mut self.children {
            child.render_at(r, m, child_offset);
        }
    }
}

/// Exact structural counterpart of C++ `LoopTray::Draw` and `Draw_Item`.
/// The normal browser renderer cannot be reused here: a tray item is a
/// `LoopTrayItem` keyed by a loop slot and invokes `VideoIO::DrawLoop`.
fn render_loop_tray(
    r: &mut dyn Renderer,
    m: &RenderMetrics,
    state: &UiSceneState,
    browser: &BrowserSceneState,
    expand: (i32, i32, i32, i32),
    loop_size: i32,
    x: i32,
    y: i32,
    font: &str,
    font_size: f32,
) {
    const BORDER: Color = Color(100, 100, 90, 255);
    const OUTLINE: Color = Color(40, 40, 40, 255);
    const WHITE: Color = Color(0xef, 0xaf, 0xff, 255);
    const LOOP_COLORS: [Color; 4] = [
        Color(0x62, 0x62, 0x62, 255),
        Color(0xf9, 0xe6, 0x13, 255),
        Color(0xff, 0xff, 0xff, 255),
        Color(0xe0, 0xda, 0xd5, 255),
    ];
    // LoopTray::Setup uses XCvt(0.03), which is 19 logical pixels in the
    // shipped 640px graphics configuration.  It is intentionally not the
    // waveform size from the XML `loopsize` attribute.
    let icon = m.x(19);
    r.draw(DrawOp::Box(x, y, x + icon, y + icon, BORDER));
    r.draw(DrawOp::Line((x, y), (x + icon, y), OUTLINE));
    r.draw(DrawOp::Line((x, y + icon), (x + icon, y + icon), OUTLINE));
    r.draw(DrawOp::Line((x, y), (x, y + icon), OUTLINE));
    r.draw(DrawOp::Line((x + icon, y), (x + icon, y + icon), OUTLINE));
    r.draw(DrawOp::FilledPie(
        x + icon / 2,
        y + icon / 2,
        icon * 3 / 8,
        30,
        359,
        LOOP_COLORS[1],
    ));
    r.draw(DrawOp::Circle(
        x + icon / 2,
        y + icon / 2,
        icon * 3 / 8,
        OUTLINE,
    ));
    if !browser.expanded {
        return;
    }

    let (left, top, right, bottom) = (m.x(expand.0), m.y(expand.1), m.x(expand.2), m.y(expand.3));
    // LoopTray::Setup uses XCvt(0.016), 10 pixels at 640px.
    let base_x = m.x(10);
    let base_y = m.y(10);
    r.draw(DrawOp::Box(left, top, left + base_x, bottom, BORDER));
    r.draw(DrawOp::Box(right - base_x, top, right, bottom, BORDER));
    r.draw(DrawOp::Box(left, top, right, top + base_y, BORDER));
    r.draw(DrawOp::Box(left, bottom - base_y, right, bottom, BORDER));
    r.draw(DrawOp::Box(
        left + base_x,
        top + base_y,
        right - base_x,
        bottom - base_y,
        Color(0, 0, 0, 255),
    ));
    r.draw(DrawOp::Line((left, top), (right, top), OUTLINE));
    r.draw(DrawOp::Line((left, bottom), (right, bottom), OUTLINE));
    r.draw(DrawOp::Line((left, top), (left, bottom), OUTLINE));
    r.draw(DrawOp::Line((right, top), (right, bottom), OUTLINE));

    let jump = loop_size + 10;
    let width = expand.2 - expand.0;
    let height = expand.3 - expand.1;
    let mut item_x = 10;
    let mut item_y = 10;
    let limiter = state
        .values
        .get("SYSTEM_cur_limiter_gain")
        .copied()
        .unwrap_or(1.0)
        .max(0.0);
    for (index, loop_id) in browser.loop_ids.iter().copied().enumerate() {
        // C++ stops at the first item that cannot fit in the fixed grid.
        if item_x >= width - jump || item_y >= height - jump {
            break;
        }
        let draw_x = left + m.x(item_x);
        let draw_y = top + m.y(item_y);
        if let Some(scope) = state.loop_scopes.get(&loop_id) {
            let colormag = (0.5 + scope.trigger_gain).min(1.0);
            let scaled = |color: Color, alpha: u8| {
                Color(
                    (color.0 as f32 * colormag) as u8,
                    (color.1 as f32 * colormag) as u8,
                    (color.2 as f32 * colormag) as u8,
                    alpha,
                )
            };
            let chunks = usize::from(scope.chunk_count).min(scope.peaks.len());
            r.draw(DrawOp::LoopScope(
                scope.peaks[..chunks].to_vec(),
                scope.averages[..chunks].to_vec(),
                scope.position_column,
                draw_x,
                draw_y,
                m.x(loop_size),
                scaled(LOOP_COLORS[0], 255),
                scaled(LOOP_COLORS[1], 255),
                scaled(LOOP_COLORS[2], 255),
                limiter * scope.gain * 20.0 * scope.current_peak,
                m.x(320),
                m.y(30),
            ));
            if let Some(name) = browser.items.get(index) {
                r.draw(DrawOp::StyledText(
                    name.clone(),
                    font.into(),
                    font_size * m.scale_y,
                    draw_x,
                    draw_y + m.y(loop_size),
                    WHITE,
                    0,
                    2,
                ));
            }
        }
        item_x += jump;
        if item_x >= width - jump {
            item_x = 10;
            item_y += jump;
        }
    }
}

impl Display for XmlDisplay {
    fn base(&self) -> &FloDisplay {
        &self.base
    }
    fn base_mut(&mut self) -> &mut FloDisplay {
        &mut self.base
    }
    fn render(&mut self, r: &mut dyn Renderer, m: &RenderMetrics) {
        self.render_at(r, m, (0, 0));
    }
}

/// Load the production scene using the real application startup time for the
/// C++ logo animation.
pub fn load_production_scene(data_dir: impl AsRef<Path>) -> Result<ProductionUiScene, String> {
    load_production_scene_at(data_dir, Instant::now())
}

/// Load the production scene with an explicit logo-animation origin.
///
/// The native application uses [`load_production_scene`], whereas fixture and
/// pixel-parity callers can pin the C++ `video_start` equivalent.  Rendering
/// a scene twice must not accidentally compare two different animation times.
pub fn load_production_scene_at(
    data_dir: impl AsRef<Path>,
    logo_started: Instant,
) -> Result<ProductionUiScene, String> {
    let data_dir = data_dir.as_ref();
    let graphics_path = data_dir.join("graphics.xml");
    let interfaces_path = data_dir.join("interfaces.xml");
    let graphics = read_xml(&graphics_path)?;
    let interfaces = read_xml(&interfaces_path)?;
    let graphics_doc =
        Document::parse(&graphics).map_err(|e| format!("{}: {e}", graphics_path.display()))?;
    let interfaces_doc =
        Document::parse(&interfaces).map_err(|e| format!("{}: {e}", interfaces_path.display()))?;
    let logical_size = graphics_doc
        .descendants()
        .find(|n| n.has_tag_name("var"))
        .and_then(|n| n.attribute("resolution"))
        .map(|v| parse_pair_u32(v, (640, 480)))
        .unwrap_or((640, 480));
    let delay = graphics_doc
        .descendants()
        .find(|n| n.has_tag_name("var"))
        .and_then(|n| n.attribute("videodelay"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    let mut font_specs = BTreeMap::new();
    for node in graphics_doc
        .descendants()
        .filter(|n| n.has_tag_name("font"))
    {
        let name = node.attribute("name").unwrap_or("main").to_string();
        let requested = node.attribute("file").unwrap_or("Vera.ttf");
        let path = resolve_asset(data_dir, requested);
        font_specs.insert(name, (path, attr_u32(node, "size", 12)));
    }
    let mut docs = vec![(0, graphics_path, graphics)];
    let mut next_switchable_id = 1;
    let mut next_non_switchable_id = 1000;
    for node in interfaces_doc
        .descendants()
        .filter(|n| n.has_tag_name("interface"))
    {
        let Some(setup) = node.attribute("setup") else {
            continue;
        };
        let interface_id = if node.attribute("switchable") == Some("0") {
            let id = next_non_switchable_id;
            next_non_switchable_id += 1;
            id
        } else {
            let id = next_switchable_id;
            next_switchable_id += 1;
            id
        };
        let path = data_dir.join(setup);
        docs.push((interface_id, path.clone(), read_xml(&path)?));
    }
    let state = Arc::new(RwLock::new(UiSceneState::default()));
    let mut scene = DisplayScene::new();
    let mut layout_displays: Vec<Box<dyn Display>> = Vec::new();
    let mut kinds = BTreeMap::new();
    let mut help_lines = Vec::new();
    let mut element_count = 0;
    for (iid, path, text) in &docs {
        let doc = Document::parse(text).map_err(|e| format!("{}: {e}", path.display()))?;
        for node in doc.descendants().filter(|n| n.is_comment()) {
            for line in node.text().unwrap_or("").lines() {
                if let Some(help) = line.trim().strip_prefix("HELP:") {
                    help_lines.push(help.trim().to_string());
                }
            }
        }
        for graphics_node in doc.descendants().filter(|n| n.has_tag_name("graphics")) {
            for node in graphics_node.children().filter(|n| n.is_element()) {
                if node.has_tag_name("layout") {
                    let layout = parse_layout(node, *iid, logical_size)?;
                    state.write().expect("UI state poisoned").layouts.insert(
                        (*iid, layout.id),
                        LayoutSceneState {
                            show: layout.show,
                            loopids: layout.loopids,
                        },
                    );
                    element_count += layout.elements.len();
                    layout_displays.push(Box::new(LayoutContent {
                        base: FloDisplay::new(*iid),
                        layout: layout.clone(),
                        state: Arc::clone(&state),
                        current_peaks: HashMap::new(),
                    }));
                    scene.layouts.push(layout);
                } else if node.has_tag_name("display") {
                    let widget = parse_display(
                        node,
                        *iid,
                        logical_size,
                        &font_specs,
                        Arc::clone(&state),
                        &mut kinds,
                    )?;
                    scene.displays.push(Box::new(widget));
                }
            }
        }
    }
    seed_browser_data(data_dir, &mut state.write().expect("UI state poisoned"))?;
    layout_displays.append(&mut scene.displays);
    scene.displays = layout_displays;
    scene.displays.push(Box::new(HelpOverlay {
        base: FloDisplay::new(0),
        lines: Arc::new(help_lines.clone()),
        state: Arc::clone(&state),
    }));
    scene.displays.push(Box::new(StaticStatusOverlay {
        base: FloDisplay::new(0),
    }));
    scene.displays.push(Box::new(PulseOverlay {
        base: FloDisplay::new(0),
        state: Arc::clone(&state),
    }));
    // This follows the C++ platform split exactly: macOS loads the bundle PNG
    // and continues after a warning if it is absent; other platforms draw the
    // compiled `fweelin_logo` RGBA surface.  Do not use the developer-only
    // extracted-assets fallback here, as installed macOS bundles do not have
    // that path and C++ never consults it.
    #[cfg(target_os = "macos")]
    let logo = {
        let logo_path = data_dir.join("fweelin-logo.png");
        match image::open(&logo_path) {
            Ok(image) => {
                let image = image.into_rgba8();
                Some((image.width(), image.height(), image.into_raw()))
            }
            Err(error) => {
                eprintln!(
                    "VIDEO: Warning: Couldn't load logo image from '{}': {error}",
                    logo_path.display()
                );
                None
            }
        }
    };
    #[cfg(not(target_os = "macos"))]
    let logo = Some((
        crate::logo::WIDTH as u32,
        crate::logo::HEIGHT as u32,
        crate::logo::PIXEL_DATA
            [..crate::logo::WIDTH * crate::logo::HEIGHT * crate::logo::BYTES_PER_PIXEL]
            .to_vec(),
    ));
    if let Some((width, height, pixels)) = logo {
        let (main_font_path, main_font_size) = font_specs
            .get("main")
            .ok_or("graphics.xml does not declare main font")?;
        let main_font_data = fs::read(main_font_path).map_err(|error| {
            format!(
                "could not read main font {}: {error}",
                main_font_path.display()
            )
        })?;
        let main_font = Font::from_bytes(main_font_data, FontSettings::default())
            .map_err(|error| format!("invalid main font {}: {error}", main_font_path.display()))?;
        let (version_width, version_height) =
            fontdue_text_metrics(&main_font, CPP_VERSION, *main_font_size as f32);
        scene.displays.push(Box::new(LogoOverlay {
            base: FloDisplay::new(0),
            width,
            height,
            pixels,
            started: logo_started,
            version_width,
            version_height,
            version_size: *main_font_size as f32,
        }));
    }
    let manifest = SceneManifest {
        logical_size,
        frame_delay: Duration::from_millis(delay),
        fonts: font_specs,
        interface_files: docs.iter().skip(1).map(|(_, p, _)| p.clone()).collect(),
        display_kinds: kinds,
        layout_count: scene.layouts.len(),
        element_count,
        help_lines,
    };
    Ok(ProductionUiScene {
        scene,
        state,
        manifest,
    })
}

pub fn production_software_renderer(
    scene: ProductionUiScene,
) -> Result<ProductionUiRenderer, String> {
    let mut fonts = Vec::new();
    for (name, (path, _)) in &scene.manifest.fonts {
        fonts.push((
            name.clone(),
            fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?,
        ));
    }
    let default = if scene.manifest.fonts.contains_key("main") {
        "main"
    } else {
        scene
            .manifest
            .fonts
            .keys()
            .next()
            .ok_or("graphics.xml defines no fonts")?
    };
    let platform = SoftwareRgbaRenderer::with_fonts(fonts, default.to_string(), 12.0)?;
    Ok(production_renderer(scene, Box::new(platform)))
}

pub fn production_renderer(
    scene: ProductionUiScene,
    platform: Box<dyn PlatformRenderer>,
) -> ProductionUiRenderer {
    let (w, h) = scene.manifest.logical_size;
    ProductionUiRenderer {
        frame_delay: scene.manifest.frame_delay,
        renderer: FrameRenderer {
            scene: scene.scene,
            platform,
            // XML coordinates are authored in this logical space; the frame
            // dimensions are the drawable target, including HiDPI backing
            // pixels.
            metrics: RenderMetrics::new(w as i32, h as i32, w as i32, h as i32),
        },
    }
}

fn parse_display(
    node: Node<'_, '_>,
    iid: i32,
    size: (u32, u32),
    fonts: &BTreeMap<String, (PathBuf, u32)>,
    state: SharedUiSceneState,
    kinds: &mut BTreeMap<String, usize>,
) -> Result<XmlDisplay, String> {
    let name = node.attribute("type").unwrap_or("text");
    *kinds.entry(name.to_string()).or_default() += 1;
    let pos = parse_normalized(node.attribute("pos").unwrap_or("0,0"), size);
    let font = node.attribute("font").unwrap_or("main").to_string();
    let font_size = fonts.get(&font).map_or(12, |v| v.1) as f32;
    if name == "browser" && node.attribute("xpand") == Some("1") {
        let browse_type = node.attribute("browsetype").unwrap_or("BROWSE_loop");
        state
            .write()
            .expect("UI state poisoned")
            .browsers
            .entry(browse_type.to_string())
            .or_default()
            .expanded = true;
    }
    let kind = match name {
        "text" => WidgetKind::Text,
        "switch" => WidgetKind::Switch,
        "text-switch" => WidgetKind::TextSwitch {
            off: node.attribute("text0").unwrap_or("Beat").into(),
            on: node.attribute("text1").unwrap_or("Bar").into(),
        },
        "bar" | "bar-switch" => WidgetKind::Bar {
            switched: name == "bar-switch",
        },
        "circle-switch" => WidgetKind::Circle {
            off: normalized_scalar(node.attribute("size0").unwrap_or("0.01"), size.0),
            on: normalized_scalar(node.attribute("size1").unwrap_or("0.02"), size.0),
        },
        "squares" => WidgetKind::Squares {
            size: parse_normalized(node.attribute("squaresize").unwrap_or("0.03,0.03"), size),
            lo: attr_f32(node, "value1", 0.0),
            hi: attr_f32(node, "value2", 10.0),
            step: attr_f32(node, "interval", 1.0),
        },
        "panel" => WidgetKind::Panel {
            size: parse_normalized(node.attribute("size").unwrap_or("0.2,0.2"), size),
        },
        "snapshots" => WidgetKind::Snapshots {
            size: parse_normalized(node.attribute("size").unwrap_or("0.3,0.22"), size),
            margin: normalized_scalar(node.attribute("margin").unwrap_or("0.01"), size.0),
        },
        "browser" => WidgetKind::Browser {
            browse_type: node.attribute("browsetype").unwrap_or("BROWSE_loop").into(),
            expand: parse_quad_normalized(
                node.attribute("xbox").unwrap_or("0.1,0.1,0.9,0.9"),
                size,
            ),
            loop_size: normalized_scalar(node.attribute("loopsize").unwrap_or("0.05"), size.0),
        },
        other => return Err(format!("unsupported display type '{other}'")),
    };
    let mut base = FloDisplay::new(
        node.attribute("interfaceid")
            .and_then(|v| v.parse().ok())
            .unwrap_or(iid),
    );
    base.id = node.attribute("id").map(stable_id).unwrap_or(-1);
    base.title = node.attribute("title").map(str::to_string);
    base.xpos = pos.0;
    base.ypos = pos.1;
    base.show = node.attribute("show").unwrap_or("1") != "0";
    let mut display = XmlDisplay {
        base,
        kind,
        variable: node.attribute("var").map(str::to_string),
        font,
        font_size,
        orientation: if node.attribute("orientation") == Some("horizontal") {
            Orientation::Horizontal
        } else {
            Orientation::Vertical
        },
        bar_scale: normalized_scalar(
            node.attribute("barscale").unwrap_or("0.1"),
            if node.attribute("orientation") == Some("horizontal") {
                size.0
            } else {
                size.1
            },
        ),
        thickness: normalized_scalar(node.attribute("thickness").unwrap_or("0.01"), size.0).max(1),
        db_scale: node.attribute("dbscale") == Some("1"),
        max_db: attr_f32(node, "maxdb", 6.0),
        state,
        children: Vec::new(),
    };
    for child in node.children().filter(|n| n.has_tag_name("display")) {
        display.children.push(parse_display(
            child,
            iid,
            size,
            fonts,
            Arc::clone(&display.state),
            kinds,
        )?);
    }
    Ok(display)
}

fn parse_layout(node: Node<'_, '_>, iid: i32, size: (u32, u32)) -> Result<FloLayout, String> {
    let scale = parse_pair_f32(node.attribute("scale").unwrap_or("1,1"), (1., 1.));
    let pos = parse_normalized(node.attribute("pos").unwrap_or("0,0"), size);
    let mut layout = FloLayout::new();
    layout.id = attr_i32(node, "id", 0);
    layout.iid = iid;
    layout.xpos = pos.0;
    layout.ypos = pos.1;
    layout.name = node.attribute("name").map(str::to_string);
    if let Some(name_pos) = node.attribute("namepos") {
        let name_pos = parse_normalized(name_pos, size);
        layout.nxpos = name_pos.0;
        layout.nypos = name_pos.1;
    }
    layout.show = node.attribute("show").unwrap_or("1") != "0";
    layout.showlabel = node.attribute("label").unwrap_or("1") != "0";
    layout.showelabel = node.attribute("elabel").unwrap_or("1") != "0";
    for en in node.children().filter(|n| n.has_tag_name("element")) {
        let base = parse_pair_f32(en.attribute("base").unwrap_or("0,0"), (0., 0.));
        let lp = parse_pair_f32(en.attribute("looppos").unwrap_or("0,0"), (0., 0.));
        let mut e = FloLayoutElement {
            id: attr_i32(en, "id", 0),
            name: en.attribute("name").map(str::to_string),
            nxpos: 0,
            nypos: 0,
            bx: base.0,
            by: base.1,
            loopx: (size.0 as f32 * (pos.0 as f32 / size.0 as f32 + (base.0 + lp.0) * scale.0))
                as i32,
            loopy: (size.1 as f32 * (pos.1 as f32 / size.1 as f32 + (base.1 + lp.1) * scale.1))
                as i32,
            loopsize: en
                .attribute("loopsize")
                .and_then(|value| value.parse::<f32>().ok())
                .map(|value| {
                    (size.0.min(size.1) as f32 * scale.0.min(scale.1) * value).round() as i32
                })
                .unwrap_or(0),
            geometry: Vec::new(),
        };
        if let Some(np) = en.attribute("namepos") {
            let n = parse_pair_f32(np, (0., 0.));
            e.nxpos = (size.0 as f32 * (base.0 + n.0) * scale.0) as i32;
            e.nypos = (size.1 as f32 * (base.1 + n.1) * scale.1) as i32;
        }
        for b in en.children().filter(|n| n.has_tag_name("box")) {
            let q = parse_quad_f32(b.attribute("pos").unwrap_or("0,0,0,0"));
            let outline = b.attribute("outline").unwrap_or("");
            let left = pos.0 + (size.0 as f32 * (base.0 + q.0) * scale.0) as i32;
            let top = pos.1 + (size.1 as f32 * (base.1 + q.1) * scale.1) as i32;
            e.add_box(FloLayoutBox {
                left,
                top,
                right: pos.0 + (size.0 as f32 * (base.0 + q.2) * scale.0) as i32,
                bottom: pos.1 + (size.1 as f32 * (base.1 + q.3) * scale.1) as i32,
                lineleft: outline.contains('L'),
                linetop: outline.contains('T'),
                lineright: outline.contains('R'),
                linebottom: outline.contains('B'),
            });
        }
        layout.add_element(e);
    }
    Ok(layout)
}

fn seed_browser_data(data: &Path, state: &mut UiSceneState) -> Result<(), String> {
    let mut patches = Vec::new();
    let path = data.join("patches-channels.xml");
    let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let doc = Document::parse(&text).map_err(|e| e.to_string())?;
    for n in doc.descendants().filter(|n| n.has_tag_name("patch")) {
        if let Some(name) = n.attribute("name") {
            let channel = attr_i32(n, "channel", patches.len() as i32);
            patches.push(format!("{channel:02}: {name}"));
        }
    }
    let expanded = state
        .browsers
        .get("BROWSE_patch")
        .is_some_and(|browser| browser.expanded);
    state.browsers.insert(
        "BROWSE_patch".into(),
        BrowserSceneState {
            items: patches,
            expanded,
            ..Default::default()
        },
    );
    state.browsers.entry("BROWSE_loop".into()).or_default();
    state.browsers.entry("BROWSE_scene".into()).or_default();
    state.browsers.entry("BROWSE_loop_tray".into()).or_default();
    Ok(())
}

fn evaluate(expression: &str, values: &HashMap<String, f32>) -> f32 {
    let mut result = 0.0;
    let mut op = '+';
    for token in expression.split_inclusive(['+', '-', '*', '/']) {
        let trimmed = token.trim_end_matches(['+', '-', '*', '/']);
        let value = trimmed
            .parse()
            .ok()
            .or_else(|| values.get(trimmed).copied())
            .unwrap_or(0.0);
        result = match op {
            '+' => result + value,
            '-' => result - value,
            '*' => result * value,
            '/' if value != 0.0 => result / value,
            _ => result,
        };
        op = token
            .chars()
            .last()
            .filter(|c| ['+', '-', '*', '/'].contains(c))
            .unwrap_or(op);
    }
    result
}
fn format_value(v: f32) -> String {
    if v.fract().abs() < f32::EPSILON {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}
fn read_xml(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}
fn resolve_asset(data: &Path, requested: &str) -> PathBuf {
    let direct = data.join(requested);
    if direct.exists() {
        direct
    } else {
        data.join(Path::new(requested).file_name().unwrap_or_default())
    }
}
fn stable_id(value: &str) -> i32 {
    value.parse().unwrap_or_else(|_| {
        value.bytes().fold(0x811c9dc5u32, |h, b| {
            (h ^ b as u32).wrapping_mul(0x01000193)
        }) as i32
    })
}
fn attr_i32(n: Node<'_, '_>, k: &str, d: i32) -> i32 {
    n.attribute(k).and_then(|v| v.parse().ok()).unwrap_or(d)
}
fn attr_u32(n: Node<'_, '_>, k: &str, d: u32) -> u32 {
    n.attribute(k).and_then(|v| v.parse().ok()).unwrap_or(d)
}
fn attr_f32(n: Node<'_, '_>, k: &str, d: f32) -> f32 {
    n.attribute(k).and_then(|v| v.parse().ok()).unwrap_or(d)
}
fn numbers(v: &str) -> Vec<f32> {
    v.split(',').filter_map(|n| n.trim().parse().ok()).collect()
}
fn parse_pair_f32(v: &str, d: (f32, f32)) -> (f32, f32) {
    let n = numbers(v);
    if n.len() >= 2 { (n[0], n[1]) } else { d }
}
fn parse_quad_f32(v: &str) -> (f32, f32, f32, f32) {
    let n = numbers(v);
    if n.len() >= 4 {
        (n[0], n[1], n[2], n[3])
    } else {
        (0., 0., 0., 0.)
    }
}
fn parse_pair_u32(v: &str, d: (u32, u32)) -> (u32, u32) {
    let p = parse_pair_f32(v, (d.0 as f32, d.1 as f32));
    (p.0.max(1.) as u32, p.1.max(1.) as u32)
}
fn parse_normalized(v: &str, s: (u32, u32)) -> (i32, i32) {
    let p = parse_pair_f32(v, (0., 0.));
    (
        (p.0 * s.0 as f32).round() as i32,
        (p.1 * s.1 as f32).round() as i32,
    )
}
fn parse_quad_normalized(v: &str, s: (u32, u32)) -> (i32, i32, i32, i32) {
    let p = parse_quad_f32(v);
    (
        (p.0 * s.0 as f32) as i32,
        (p.1 * s.1 as f32) as i32,
        (p.2 * s.0 as f32) as i32,
        (p.3 * s.1 as f32) as i32,
    )
}
fn normalized_scalar(v: &str, size: u32) -> i32 {
    v.parse::<f32>()
        .map(|n| (n * size as f32).round() as i32)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::videoio::{VideoFrame, VideoRenderer};

    #[derive(Default)]
    struct RecordingRenderer(Vec<DrawOp>);
    impl Renderer for RecordingRenderer {
        fn draw(&mut self, op: DrawOp) {
            self.0.push(op);
        }
    }

    fn data() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../data")
    }
    #[test]
    fn loads_every_real_graphics_fixture() {
        let ui = load_production_scene(data()).unwrap();
        assert_eq!(ui.manifest.logical_size, (640, 480));
        assert!(ui.manifest.interface_files.len() >= 8);
        assert!(ui.manifest.layout_count >= 4);
        assert!(ui.manifest.element_count >= 40);
        for kind in [
            "text",
            "switch",
            "text-switch",
            "bar",
            "bar-switch",
            "circle-switch",
            "squares",
            "panel",
            "snapshots",
            "browser",
        ] {
            assert!(
                ui.manifest.display_kinds.contains_key(kind),
                "missing {kind}"
            );
        }
        assert!(ui.manifest.help_lines.len() > 40);
        assert!(
            !ui.state.read().unwrap().browsers["BROWSE_patch"]
                .items
                .is_empty()
        );
    }
    #[test]
    fn actual_data_renders_a_nonblank_deterministic_frame() {
        fn render() -> VideoFrame {
            let ui =
                load_production_scene_at(data(), Instant::now() - Duration::from_secs(1)).unwrap();
            let mut renderer = production_software_renderer(ui).unwrap().renderer;
            let mut frame = VideoFrame {
                pixels: vec![],
                width: 640,
                height: 480,
                stride: 0,
                timestamp: 0.,
            };
            renderer.render(&mut frame);
            frame
        }
        let a = render();
        let b = render();
        assert_eq!(a.pixels, b.pixels);
        assert_eq!(a.pixels.len(), 640 * 480 * 4);
        assert!(
            a.pixels
                .chunks_exact(4)
                .filter(|p| p[0] != 0 || p[1] != 0 || p[2] != 0)
                .count()
                > 1_000
        );
    }

    #[test]
    fn loop_rendering_includes_legacy_scope_overlays_and_labels() {
        let state = Arc::new(RwLock::new(UiSceneState::default()));
        state
            .write()
            .unwrap()
            .values
            .insert("SYSTEM_cur_limiter_gain".into(), 0.5);
        state.write().unwrap().loop_scopes.insert(
            0,
            LoopScopeState {
                peaks: vec![0.5; crate::native_dsp_graph::LOOP_SCOPE_COLUMNS],
                averages: vec![0.25; crate::native_dsp_graph::LOOP_SCOPE_COLUMNS],
                position_column: 80,
                chunk_count: crate::native_dsp_graph::LOOP_SCOPE_COLUMNS as u16,
                current_peak: 0.75,
                mode: crate::native_dsp_graph::LoopMode::Overdubbing,
                gain: 1.0,
                trigger_gain: 0.5,
                gain_delta: 1.01,
                selected: true,
                recent_rank: Some(0),
                name: Some("loop-name".into()),
            },
        );
        let mut layout = FloLayout::new();
        layout.loopids = (0, 0);
        layout.showlabel = false;
        layout.showelabel = false;
        layout.elements.push(FloLayoutElement {
            id: 0,
            loopx: 100,
            loopy: 100,
            loopsize: 60,
            ..Default::default()
        });
        let mut display = LayoutContent {
            base: FloDisplay::new(0),
            layout,
            state,
            current_peaks: HashMap::new(),
        };
        let mut renderer = RecordingRenderer::default();
        display.render(&mut renderer, &RenderMetrics::new(640, 480, 640, 480));

        assert!(
            renderer
                .0
                .iter()
                .any(|op| matches!(op, DrawOp::LoopScope(..)))
        );
        assert!(
            renderer
                .0
                .iter()
                .any(|op| matches!(op, DrawOp::FilledPie(..)))
        );
        let scope_magnitude = renderer
            .0
            .iter()
            .find_map(|op| match op {
                DrawOp::LoopScope(_, _, _, _, _, _, _, _, _, magnitude, _, _) => Some(*magnitude),
                _ => None,
            })
            .unwrap();
        // C++ scans the newly entered peak columns (all 0.5), producing
        // `0.5 * 2 + 0.5 == 1.5`; the final magnitude is
        // `lvol * loopvol * 20 * current_peak == 15`.
        assert!((scope_magnitude - 15.0).abs() < f32::EPSILON);
        assert!(
            renderer
                .0
                .iter()
                .any(|op| matches!(op, DrawOp::FilledPie(_, _, 15, ..)))
        );
        assert_eq!(
            renderer
                .0
                .iter()
                .filter(|op| matches!(op, DrawOp::Box(..)))
                .count(),
            2
        );
        for expected in ["O", "loop-name", "L1"] {
            assert!(
                renderer
                    .0
                    .iter()
                    .any(|op| matches!(op, DrawOp::StyledText(text, ..) if text == expected))
            );
        }
    }

    #[test]
    fn loop_tray_uses_sparse_live_slot_ids_for_draw_loop_scopes() {
        let mut state = UiSceneState::default();
        state.values.insert("SYSTEM_cur_limiter_gain".into(), 1.0);
        state.loop_scopes.insert(
            7,
            LoopScopeState {
                peaks: vec![0.75],
                averages: vec![0.25],
                position_column: 1,
                chunk_count: 1,
                current_peak: 0.5,
                gain: 1.0,
                trigger_gain: 0.25,
                ..LoopScopeState::default()
            },
        );
        let browser = BrowserSceneState {
            items: vec!["slot-seven".into()],
            loop_ids: vec![7],
            expanded: true,
            ..Default::default()
        };
        let metrics = RenderMetrics::new(640, 480, 640, 480);
        let mut renderer = RecordingRenderer::default();
        render_loop_tray(
            &mut renderer,
            &metrics,
            &state,
            &browser,
            (32, 288, 608, 432),
            32,
            10,
            442,
            "tiny",
            8.0,
        );
        assert!(renderer.0.iter().any(|op| matches!(
            op,
            DrawOp::LoopScope(peaks, averages, 1, 42, 298, 32, ..)
                if peaks == &vec![0.75] && averages == &vec![0.25]
        )));
        assert!(
            renderer
                .0
                .iter()
                .any(|op| matches!(op, DrawOp::StyledText(text, ..) if text == "slot-seven"))
        );
    }

    #[test]
    fn expanded_browser_draws_entries_before_and_after_current_item() {
        let state = Arc::new(RwLock::new(UiSceneState::default()));
        state.write().unwrap().browsers.insert(
            "BROWSE_loop".into(),
            BrowserSceneState {
                items: vec!["A".into(), "B".into(), "C".into(), "D".into(), "E".into()],
                selected: 2,
                expanded: true,
                ..Default::default()
            },
        );
        let mut display = XmlDisplay {
            base: FloDisplay::new(0),
            kind: WidgetKind::Browser {
                browse_type: "BROWSE_loop".into(),
                expand: (0, 0, 100, 100),
                loop_size: 32,
            },
            variable: None,
            font: "main".into(),
            font_size: 10.0,
            orientation: Orientation::Horizontal,
            bar_scale: 1,
            thickness: 1,
            db_scale: false,
            max_db: 0.0,
            state,
            children: Vec::new(),
        };
        let mut renderer = RecordingRenderer::default();
        display.render(&mut renderer, &RenderMetrics::new(100, 100, 100, 100));
        let text_y = |needle: &str| {
            renderer.0.iter().find_map(|op| match op {
                DrawOp::StyledText(text, _, _, _, y, _, _, _) if text == needle => Some(*y),
                _ => None,
            })
        };
        assert!(text_y("A").is_some());
        assert!(text_y("B").is_some());
        assert!(text_y("C").is_some());
        assert!(text_y("D").is_some());
        assert!(text_y("B").unwrap() < text_y("C").unwrap());
        assert!(text_y("C").unwrap() < text_y("D").unwrap());
    }

    #[test]
    fn selected_pulse_draws_cpp_long_count_outer_wedges() {
        let state = Arc::new(RwLock::new(UiSceneState::default()));
        state.write().unwrap().values.extend([
            ("pulse-active".into(), 1.0),
            ("pulse-frames".into(), 100.0),
            ("pulse-position".into(), 25.0),
            ("pulse-long-count".into(), 2.0),
            ("pulse-long-length".into(), 4.0),
        ]);
        let mut pulse = PulseOverlay {
            base: FloDisplay::new(0),
            state,
        };
        let mut renderer = RecordingRenderer::default();
        pulse.render(&mut renderer, &RenderMetrics::new(640, 480, 640, 480));
        // Selected pulse: base radius 10 × 2; C++ completed-beat wedges use
        // `int(radius * 1.3)`, hence 26.
        assert!(renderer.0.iter().any(|op| matches!(
            op,
            DrawOp::FilledPie(_, _, 26, _, _, Color(255, 188, 0, 180))
        )));
        assert!(
            renderer
                .0
                .iter()
                .any(|op| matches!(op, DrawOp::FilledPie(_, _, 20, 0, 359, _)))
        );
    }

    #[test]
    fn logo_uses_the_cpp_steady_state_bottom_right_position() {
        let mut logo = LogoOverlay {
            base: FloDisplay::new(0),
            width: 223,
            height: 42,
            pixels: vec![0; 223 * 42 * 4],
            started: Instant::now() - Duration::from_secs(2),
            version_width: 16,
            version_height: 12,
            version_size: 12.0,
        };
        let mut renderer = RecordingRenderer::default();
        logo.render(&mut renderer, &RenderMetrics::new(640, 480, 640, 480));
        assert!(
            renderer
                .0
                .iter()
                .any(|op| matches!(op, DrawOp::Image(_, 223, 42, 417, 438, 223, 42)))
        );
    }

    #[test]
    fn logo_startup_slide_uses_cpp_drawable_coordinates() {
        assert_eq!(cpp_logo_y(480, 42, 0.0), -42);
        assert_eq!(cpp_logo_y(480, 42, 0.5), 198);
        assert_eq!(cpp_logo_y(480, 42, 0.999), 437);
        assert_eq!(cpp_logo_y(480, 42, 1.0), 438);
    }

    #[test]
    fn logo_version_uses_cpp_float_in_hold_and_float_out_phases() {
        // `ver_x + version_margin_x == 25` at this fixture size.
        assert_eq!(cpp_logo_version_x(640, 20, 5, 1.999), None);
        assert_eq!(cpp_logo_version_x(640, 20, 5, 2.0), Some(615));
        assert_eq!(cpp_logo_version_x(640, 20, 5, 2.5), Some(627));
        assert_eq!(cpp_logo_version_x(640, 20, 5, 3.0), Some(615));
        assert_eq!(cpp_logo_version_x(640, 20, 5, 4.0), Some(615));
        assert_eq!(cpp_logo_version_x(640, 20, 5, 4.5), Some(627));
        assert_eq!(cpp_logo_version_x(640, 20, 5, 5.0), None);
    }

    #[test]
    fn layout_loop_geometry_uses_cpp_min_axis_and_layout_scale() {
        let xml = r#"
            <layout pos="0.1,0.2" namepos="0.05,0.1" scale="0.5,0.75">
              <element id="0" base="0.2,0.3" namepos="0.04,0.05"
                       looppos="0.1,0.1" loopsize="0.1" />
            </layout>
        "#;
        let document = Document::parse(xml).unwrap();
        let layout = parse_layout(document.root_element(), 0, (640, 480)).unwrap();
        let element = &layout.elements[0];
        // FloConfig uses the layout's smaller scale on the smaller screen
        // axis for loop diameter: min(640*.5*.1, 480*.5*.1) == 24.
        assert_eq!(element.loopsize, 24);
        assert_eq!((element.loopx, element.loopy), (160, 240));
        assert_eq!((element.nxpos, element.nypos), (76, 126));
        assert_eq!((layout.nxpos, layout.nypos), (32, 48));
    }
    #[test]
    fn scheduler_skips_missed_ticks_without_drift() {
        let start = Instant::now();
        let mut s = FrameScheduler::new(Duration::from_millis(40), start);
        assert_eq!(
            s.advance(start + Duration::from_millis(95)),
            start + Duration::from_millis(120)
        );
    }
}
