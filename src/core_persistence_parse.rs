//! XML readers and load plans for the persistence data types.
//!
//! The C++ loader queues loop loads as it walks a scene.  This module keeps
//! that side effect as data: callers can apply `SceneLoad` to their loop and
//! snapshot owners without coupling persistence to the audio engine.

use crate::core_persistence::{LOOP_FORMAT_VERSION, LoopMeta, Scene, SnapshotLoop, SnapshotMeta};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopMetadata {
    pub smooth_end: bool,
    pub nbeats: Option<i64>,
    pub pulse_length: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneLoad {
    pub loops: Vec<LoopMeta>,
    pub snapshots: Vec<SnapshotMeta>,
}

impl SceneLoad {
    pub fn into_scene(self) -> Scene {
        Scene {
            loops: self.loops,
            snapshots: self.snapshots,
        }
    }
}

fn atoi(s: Option<&str>, default: i32) -> i32 {
    let s = match s {
        Some(s) => s.trim(),
        None => return default,
    };
    let (sign, digits) = match s.strip_prefix('-') {
        Some(v) => (-1, v),
        None => (1, s),
    };
    let digits = digits
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        default
    } else {
        sign * digits.parse::<i32>().unwrap_or(i32::MAX)
    }
}

fn atof(s: Option<&str>, default: f32) -> f32 {
    s.and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

pub fn parse_loop_metadata_xml(xml: &str) -> Result<LoopMetadata, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    if root.tag_name().name() != "loop" {
        return Err("loop data has bad format".into());
    }
    let version = atoi(root.attribute("version"), 0);
    Ok(LoopMetadata {
        smooth_end: version >= LOOP_FORMAT_VERSION as i32,
        nbeats: root.attribute("nbeats").and_then(|v| v.trim().parse().ok()),
        pulse_length: root
            .attribute("pulselen")
            .and_then(|v| v.trim().parse().ok()),
    })
}

pub fn parse_scene_xml(xml: &str, default_loop_id: i32) -> Result<SceneLoad, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    if root.tag_name().name() != "scene" {
        return Err("scene data has bad format".into());
    }
    let mut result = SceneLoad {
        loops: Vec::new(),
        snapshots: Vec::new(),
    };
    for node in root.children().filter(|n| n.is_element()) {
        match node.tag_name().name() {
            "loop" => {
                let hash = node.attribute("hash").ok_or_else(|| {
                    format!(
                        "scene definition for loop (id {}) has missing hash",
                        atoi(node.attribute("loopid"), default_loop_id)
                    )
                })?;
                result.loops.push(LoopMeta {
                    hash: hash.to_owned(),
                    loop_id: atoi(node.attribute("loopid"), default_loop_id),
                    volume: atof(node.attribute("volume"), 1.0),
                });
            }
            "snapshot" => {
                let loops = node
                    .children()
                    .filter(|n| n.is_element() && n.tag_name().name() == "loopsnapshot")
                    .map(|n| SnapshotLoop {
                        loop_id: atoi(n.attribute("loopid"), 0),
                        status: atoi(n.attribute("status"), 0),
                        loop_volume: atof(n.attribute("loopvol"), 0.0),
                        trigger_volume: atof(n.attribute("triggervol"), 0.0),
                    })
                    .collect();
                result.snapshots.push(SnapshotMeta {
                    id: atoi(node.attribute("snapid"), 0),
                    name: node.attribute("name").unwrap_or_default().to_owned(),
                    loops,
                });
            }
            _ => {}
        }
    }
    Ok(result)
}

pub fn parse_scene(xml: &str) -> Result<Scene, String> {
    parse_scene_xml(xml, 0).map(SceneLoad::into_scene)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_persistence::{LoopMeta, SnapshotMeta, scene_xml};

    #[test]
    fn scene_serialization_round_trips() {
        let scene = Scene {
            loops: vec![LoopMeta {
                hash: "AB".into(),
                loop_id: 3,
                volume: 0.5,
            }],
            snapshots: vec![SnapshotMeta {
                id: 2,
                name: "a & b".into(),
                loops: vec![SnapshotLoop {
                    loop_id: 3,
                    status: 1,
                    loop_volume: 0.7,
                    trigger_volume: 0.8,
                }],
            }],
        };
        assert_eq!(parse_scene(&scene_xml(&scene)).unwrap(), scene);
    }

    #[test]
    fn defaults_match_cpp_loader() {
        let s = parse_scene_xml(
            "<scene><loop hash=\"h\"/><snapshot><loopsnapshot/></snapshot></scene>",
            9,
        )
        .unwrap();
        assert_eq!(s.loops[0].loop_id, 9);
        assert_eq!(s.loops[0].volume, 1.0);
        assert_eq!(s.snapshots[0].loops[0].loop_id, 0);
    }

    #[test]
    fn loop_metadata_version_controls_smoothing() {
        assert!(
            !parse_loop_metadata_xml("<loop version=\"0\"/>")
                .unwrap()
                .smooth_end
        );
        assert!(
            parse_loop_metadata_xml("<loop version=\"1\" nbeats=\"4\" pulselen=\"12\"/>")
                .unwrap()
                .smooth_end
        );
    }
}
