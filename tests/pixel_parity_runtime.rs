use freewheeling_plus::videoio::{VideoFrame, VideoRenderer};
use freewheeling_plus::videoio_platform::native_ui_scene::{
    load_production_scene_at, production_software_renderer,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAGIC: &[u8] = b"FWRGBA1\n";
const MAX_DELTA: u8 = 2;
const MINIMUM_PERCENT: f64 = 99.5;
type RegionDeclaration = (&'static str, (u32, u32, u32, u32));

#[derive(Clone, Copy)]
struct Capture {
    evidence: &'static str,
    reference: &'static str,
    drawable: (u32, u32),
}

const CAPTURES: [Capture; 4] = [
    Capture {
        evidence: "640x480",
        reference: "window-640x480.png",
        drawable: (640, 480),
    },
    Capture {
        evidence: "configured",
        reference: "configured-800x600.png",
        drawable: (800, 600),
    },
    Capture {
        evidence: "fullscreen",
        reference: "fullscreen-logical-1024x768.png",
        drawable: (1024, 768),
    },
    Capture {
        evidence: "hidpi",
        reference: "hidpi-640x480-2x.png",
        drawable: (1280, 960),
    },
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn render(drawable: (u32, u32), logo_started: Instant) -> VideoFrame {
    let scene = load_production_scene_at(root().join("data"), logo_started)
        .expect("load production XML scene");
    assert_eq!(scene.manifest.logical_size, (640, 480));
    let mut renderer = production_software_renderer(scene).expect("create software renderer");
    let mut frame = VideoFrame {
        pixels: Vec::new(),
        width: drawable.0,
        height: drawable.1,
        stride: drawable.0 as usize * 4,
        timestamp: 0.0,
    };
    renderer.renderer.render(&mut frame);
    assert_eq!(frame.stride, drawable.0 as usize * 4);
    assert_eq!(
        frame.pixels.len(),
        drawable.0 as usize * drawable.1 as usize * 4
    );
    frame
}

fn fwrgba(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(MAGIC.len() + 8 + pixels.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&height.to_le_bytes());
    bytes.extend_from_slice(pixels);
    bytes
}

fn region_declarations(width: u32, height: u32) -> Vec<RegionDeclaration> {
    let scaled = |x: u32, y: u32, w: u32, h: u32| {
        (
            x * width / 640,
            y * height / 480,
            w * width / 640,
            h * height / 480,
        )
    };
    vec![
        ("keyboard-and-logo", scaled(0, 0, 640, 145)),
        ("primary-browser", scaled(23, 240, 555, 192)),
        ("status-and-controls", scaled(0, 382, 640, 98)),
    ]
}

fn regions(width: u32, height: u32) -> String {
    let mut out = String::from("# name\tx\ty\twidth\theight\n");
    for (name, (x, y, w, h)) in region_declarations(width, height) {
        out.push_str(&format!("{name}\t{x}\t{y}\t{w}\t{h}\n"));
    }
    out
}

fn region_parity(
    reference: &[u8],
    candidate: &[u8],
    frame_width: u32,
    region: (u32, u32, u32, u32),
) -> f64 {
    let (x, y, width, height) = region;
    let mut good = 0usize;
    for row in y..y + height {
        for column in x..x + width {
            let offset = ((row * frame_width + column) * 4) as usize;
            good += (0..4).all(|channel| {
                reference[offset + channel].abs_diff(candidate[offset + channel]) <= MAX_DELTA
            }) as usize;
        }
    }
    good as f64 * 100.0 / (width * height) as f64
}

fn parity(reference: &[u8], candidate: &[u8]) -> f64 {
    assert_eq!(reference.len(), candidate.len());
    let good = reference
        .chunks_exact(4)
        .zip(candidate.chunks_exact(4))
        .filter(|(a, b)| (0..4).all(|channel| a[channel].abs_diff(b[channel]) <= MAX_DELTA))
        .count();
    good as f64 * 100.0 / (reference.len() / 4) as f64
}

fn decode_reference(path: &Path, dimensions: (u32, u32)) -> Vec<u8> {
    let image = image::open(path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .into_rgba8();
    assert_eq!(
        image.dimensions(),
        dimensions,
        "{} dimensions",
        path.display()
    );
    image.into_raw()
}

#[cfg(target_os = "macos")]
#[test]
fn production_xml_scene_is_deterministic_at_all_acceptance_sizes() {
    // Pin C++ `video_start` in the stationary logo phase. The normal
    // application keeps the real-time slide; fixture renders must instead
    // compare identical animation state.
    let logo_started = Instant::now() - Duration::from_secs(1);
    for capture in CAPTURES {
        let first = render(capture.drawable, logo_started);
        let second = render(capture.drawable, logo_started);
        assert_eq!(
            first.pixels, second.pixels,
            "{} rendering is nondeterministic",
            capture.evidence
        );
    }
}

#[test]
fn emit_candidates_and_compare_genuine_cpp_references_when_requested() {
    let Some(evidence_root) = std::env::var_os("FW_PIXEL_EVIDENCE") else {
        return;
    };
    let reference_root = std::env::var_os("FW_CPP_SCREENSHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| root().join("fixtures/cpp-golden/screenshots"));
    let mut failures = Vec::new();
    let logo_started = Instant::now() - Duration::from_secs(1);
    for capture in CAPTURES {
        let frame = render(capture.drawable, logo_started);
        let directory = PathBuf::from(&evidence_root)
            .join("pixels")
            .join(capture.evidence);
        fs::create_dir_all(&directory).expect("create evidence directory");
        fs::write(
            directory.join("candidate.fwrgba"),
            fwrgba(frame.width, frame.height, &frame.pixels),
        )
        .unwrap();
        fs::write(
            directory.join("regions.tsv"),
            regions(frame.width, frame.height),
        )
        .unwrap();

        let reference_path = reference_root.join(capture.reference);
        if !reference_path.is_file() {
            eprintln!(
                "reference absent, candidate only: {}",
                reference_path.display()
            );
            continue;
        }
        let reference = decode_reference(&reference_path, capture.drawable);
        fs::write(
            directory.join("reference.fwrgba"),
            fwrgba(frame.width, frame.height, &reference),
        )
        .unwrap();
        let percent = parity(&reference, &frame.pixels);
        eprintln!(
            "{}: pixels_within_delta={percent:.6}% max_delta={MAX_DELTA}",
            capture.evidence
        );
        if percent + 1e-12 < MINIMUM_PERCENT {
            failures.push(format!("{}={percent:.6}%", capture.evidence));
        }
        for (name, bounds) in region_declarations(frame.width, frame.height) {
            let region_percent = region_parity(&reference, &frame.pixels, frame.width, bounds);
            eprintln!(
                "{} region={name} pixels_within_delta={region_percent:.6}%",
                capture.evidence
            );
            if region_percent + 1e-12 < MINIMUM_PERCENT {
                failures.push(format!("{}/{name}={region_percent:.6}%", capture.evidence));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "pixel parity below {MINIMUM_PERCENT:.6}%: {}",
        failures.join(", ")
    );
}
