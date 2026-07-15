//! Persistence primitives migrated from `fweelin_core_persistence.cc`.
//!
//! Application, configuration, browser, audio and event-manager integration is
//! deliberately expressed through data/traits here; those owners can provide
//! the adapters when their modules are migrated.

use std::fmt::Write;

pub const HASH_LENGTH: usize = 16;
pub const LOOP_FORMAT_VERSION: u32 = 1;

pub trait Saveable {
    fn save_hash(&self) -> Option<[u8; HASH_LENGTH]>;
    fn set_save_hash(&mut self, hash: [u8; HASH_LENGTH]);
}

pub trait LoopSource: Saveable {
    fn audio_bytes(&self) -> &[u8];
    fn object_name(&self) -> Option<&str>;
    fn nbeats(&self) -> i64;
    fn pulse_length(&self) -> u32;
}

pub trait AudioLoopSource: LoopSource {
    fn sample_rate(&self) -> u32;
    fn left_samples(&self) -> &[crate::block::Sample];
    fn right_samples(&self) -> Option<&[crate::block::Sample]>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopPath {
    pub stub: String,
    pub audio: String,
    pub data: String,
}

pub fn saveable_stub(base: &str, hash: &str, name: Option<&str>, ext: Option<&str>) -> String {
    let mut s = format!("{}-{}", base, hash);
    if let Some(n) = name.filter(|n| !n.is_empty()) {
        write!(s, "-{n}").unwrap();
    }
    s.push_str(ext.unwrap_or(""));
    s
}

pub fn saveable_path(
    library: &str,
    base: &str,
    hash: &str,
    name: Option<&str>,
    ext: Option<&str>,
) -> String {
    format!("{}/{}", library, saveable_stub(base, hash, name, ext))
}

pub fn split_filename(filename: &str, base_len: usize) -> Result<(String, String, String), String> {
    let slash = filename
        .get(base_len..)
        .ok_or_else(|| format!("invalid filename: {filename}"))?;
    if slash.is_empty() {
        return Err(format!("invalid filename: {filename}"));
    }
    let dot = filename.rfind('.').unwrap_or(filename.len());
    let breaker = filename[base_len + 1..]
        .find('-')
        .map(|i| base_len + 1 + i)
        .unwrap_or(dot);
    if dot < base_len + 1 || breaker < base_len + 1 || breaker - (base_len + 1) != HASH_LENGTH * 2 {
        return Err(format!("invalid hash in filename: {filename}"));
    }
    let hash = &filename[base_len + 1..breaker];
    if decode_hash(hash).is_none() {
        return Err(format!("invalid hash in filename: {filename}"));
    }
    let name = if breaker < dot {
        filename[breaker + 1..dot].to_string()
    } else {
        String::new()
    };
    Ok((filename[..base_len].to_string(), hash.to_string(), name))
}

pub fn encode_hash(hash: &[u8; HASH_LENGTH]) -> String {
    hash.iter().map(|b| format!("{b:02X}")).collect()
}
pub fn decode_hash(s: &str) -> Option<[u8; HASH_LENGTH]> {
    if s.len() != HASH_LENGTH * 2 {
        return None;
    }
    let mut out = [0; HASH_LENGTH];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

// The C++ implementation hashes quantized audio bytes. This adapter preserves
// that contract while keeping audio/block ownership outside this module.
pub fn md5_audio(bytes: &[u8]) -> [u8; HASH_LENGTH] {
    md5(bytes)
}

/// Hash the signal representation used by FreeWheeling loop persistence.
///
/// `LoopManager::SetupSaveLoop` feeds one 8-bit quantised value per sample to
/// its MD5 stream, first for the left channel and then for the right channel.
/// The old C++ implementation obtains the source pointer incorrectly (it
/// passes the address of a stack scalar with a multi-byte length), which is
/// undefined and consequently cannot be reproduced as a portable identifier.
/// This is the deterministic representation that code was clearly intended to
/// generate and keeps hashes independent of the host float byte order.
pub fn md5_loop_samples(
    left: &[crate::block::Sample],
    right: Option<&[crate::block::Sample]>,
) -> [u8; HASH_LENGTH] {
    let right = right.unwrap_or(&[]);
    let mut quantised = Vec::with_capacity(left.len().saturating_add(right.len()));
    quantised.extend(left.iter().map(|sample| (*sample * 256.0) as u8));
    quantised.extend(right.iter().map(|sample| (*sample * 256.0) as u8));
    md5(&quantised)
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoopMeta {
    pub hash: String,
    pub loop_id: i32,
    pub volume: f32,
}
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotLoop {
    pub loop_id: i32,
    pub status: i32,
    pub loop_volume: f32,
    pub trigger_volume: f32,
}
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotMeta {
    pub id: i32,
    pub name: String,
    pub loops: Vec<SnapshotLoop>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub loops: Vec<LoopMeta>,
    pub snapshots: Vec<SnapshotMeta>,
}

pub fn loop_metadata_xml(nbeats: i64, pulse_length: u32) -> String {
    format!(
        "<?xml version=\"1.0\"?>\n<loop version=\"{LOOP_FORMAT_VERSION}\" nbeats=\"{nbeats}\" pulselen=\"{pulse_length}\"/>\n"
    )
}

pub fn scene_xml(scene: &Scene) -> String {
    let mut x = String::from("<?xml version=\"1.0\"?>\n<scene>\n");
    for l in &scene.loops {
        writeln!(
            x,
            "  <loop loopid=\"{}\" hash=\"{}\" volume=\"{:.5}\"/>",
            l.loop_id, l.hash, l.volume
        )
        .unwrap();
    }
    for s in &scene.snapshots {
        writeln!(
            x,
            "  <snapshot snapid=\"{}\" name=\"{}\">",
            s.id,
            xml_escape(&s.name)
        )
        .unwrap();
        for l in &s.loops {
            writeln!(
                x,
                "    <loopsnapshot loopid=\"{}\" status=\"{}\" loopvol=\"{}\" triggervol=\"{}\"/>",
                l.loop_id,
                l.status,
                format_args!("{:.5}", l.loop_volume),
                format_args!("{:.5}", l.trigger_volume)
            )
            .unwrap();
        }
        x.push_str("  </snapshot>\n");
    }
    x.push_str("</scene>\n");
    x
}
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

// Small dependency-free MD5 implementation.
fn md5(input: &[u8]) -> [u8; 16] {
    let mut a: u32 = 0x67452301;
    let mut b: u32 = 0xefcdab89;
    let mut c: u32 = 0x98badcfe;
    let mut d: u32 = 0x10325476;
    let mut m = input.to_vec();
    let bits = (m.len() as u64) * 8;
    m.push(0x80);
    while m.len() % 64 != 56 {
        m.push(0)
    }
    m.extend_from_slice(&bits.to_le_bytes());
    let s = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];
    let mut k = [0u32; 64];
    for (i, value) in k.iter_mut().enumerate() {
        *value = ((f64::sin((i + 1) as f64).abs() * (1u64 << 32) as f64) as u64) as u32;
    }
    for ch in m.chunks(64) {
        let mut q = [0u32; 16];
        for (value, bytes) in q.iter_mut().zip(ch.chunks_exact(4)) {
            *value = u32::from_le_bytes(bytes.try_into().unwrap())
        }
        let (aa, bb, cc, dd) = (a, b, c, d);
        let mut f;
        let mut g;
        for i in 0..64 {
            if i < 16 {
                f = (b & c) | (!b & d);
                g = i
            } else if i < 32 {
                f = (d & b) | (!d & c);
                g = (5 * i + 1) % 16
            } else if i < 48 {
                f = b ^ c ^ d;
                g = (3 * i + 5) % 16
            } else {
                f = c ^ (b | !d);
                g = (7 * i) % 16
            }
            let t = a.wrapping_add(f).wrapping_add(k[i]).wrapping_add(q[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(t.rotate_left(s[i % 4 + (i / 16) * 4]));
        }
        a = a.wrapping_add(aa);
        b = b.wrapping_add(bb);
        c = c.wrapping_add(cc);
        d = d.wrapping_add(dd);
    }
    let mut out = [0; 16];
    for (i, v) in [a, b, c, d].iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes())
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_and_split() {
        let hash = "0123456789ABCDEF0123456789ABCDEF";
        let p = saveable_path("lib", "loop", hash, Some("name"), Some(".wav"));
        assert_eq!(p, "lib/loop-0123456789ABCDEF0123456789ABCDEF-name.wav");
        assert_eq!(
            split_filename("lib/loop-0123456789ABCDEF0123456789ABCDEF-name.wav", 8).unwrap(),
            ("lib/loop".into(), hash.into(), "name".into())
        );
    }
    #[test]
    fn md5_known() {
        assert_eq!(
            encode_hash(&md5_audio(b"abc")),
            "900150983CD24FB0D6963F7D28E17F72"
        );
    }

    #[test]
    fn loop_signal_hash_uses_cpp_channel_order_and_quantisation() {
        let left = [0.0, 0.5, 0.25];
        let right = [0.75, 0.125];
        assert_eq!(
            encode_hash(&md5_loop_samples(&left, Some(&right))),
            encode_hash(&md5_audio(&[0, 128, 64, 192, 32]))
        );
    }

    #[test]
    fn scene_floats_match_cpp_five_decimal_serialization() {
        let xml = scene_xml(&Scene {
            loops: vec![LoopMeta {
                hash: "AB".into(),
                loop_id: 3,
                volume: 0.5,
            }],
            snapshots: vec![SnapshotMeta {
                id: 1,
                name: "snapshot".into(),
                loops: vec![SnapshotLoop {
                    loop_id: 3,
                    status: 2,
                    loop_volume: 0.25,
                    trigger_volume: 1.0,
                }],
            }],
        });
        assert!(xml.contains("loopid=\"3\" hash=\"AB\" volume=\"0.50000\""));
        assert!(xml.contains("loopvol=\"0.25000\" triggervol=\"1.00000\""));
    }
}
