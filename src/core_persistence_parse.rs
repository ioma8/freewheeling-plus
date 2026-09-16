//! XML readers and load plans for the persistence data types.
//!
//! The C++ loader queues loop loads as it walks a scene.  This module keeps
//! that side effect as data: callers can apply [`SceneLoad`] to their loop and
//! snapshot owners without coupling persistence to the audio engine.
//!
//! Parsing contract: a missing or empty attribute falls back to the C++
//! loader's default, while a *present but malformed* value (or an unknown
//! element) is reported as an error. Silently accepting those would load a
//! truncated scene as a plausible one.

use crate::core_persistence::{LOOP_FORMAT_VERSION, LoopMeta, Scene, SnapshotLoop, SnapshotMeta};

/// Per-loop metadata written next to a saved loop's audio file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopMetadata {
    /// Whether the loop was saved with the endpoint-smoothing format.
    pub smooth_end: bool,
    /// Beat count of the save, when the file records one.
    pub nbeats: Option<i64>,
    /// Pulse length in frames, when the file records one.
    pub pulse_length: Option<u32>,
}

/// Parsed scene file: the load plan for the loop and snapshot owners.
///
/// A newtype over [`Scene`] so the plan cannot drift from the scene type: it
/// *is* a scene.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneLoad(pub Scene);

impl SceneLoad {
    /// Loops the scene refers to, in file order.
    pub fn loops(&self) -> &[LoopMeta] {
        &self.0.loops
    }

    /// Snapshots stored in the scene.
    pub fn snapshots(&self) -> &[SnapshotMeta] {
        &self.0.snapshots
    }

    /// The parsed scene itself.
    pub fn into_scene(self) -> Scene {
        self.0
    }
}

/// Parse a numeric attribute.
///
/// A missing or empty attribute yields `default`; a present but malformed value
/// is an error naming the element, the attribute and the offending text.
fn parse_number<T>(node: roxmltree::Node<'_, '_>, name: &str, default: T) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match node
        .attribute(name)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => value.parse().map_err(|error| {
            format!(
                "invalid attribute '{name}' value '{value}' in <{}>: {error}",
                node.tag_name().name()
            )
        }),
        None => Ok(default),
    }
}

/// Parse an optional numeric attribute; see [`parse_number`].
fn parse_optional_number<T>(
    node: roxmltree::Node<'_, '_>,
    name: &str,
) -> Result<Option<T>, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match node
        .attribute(name)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => value.parse().map(Some).map_err(|error| {
            format!(
                "invalid attribute '{name}' value '{value}' in <{}>: {error}",
                node.tag_name().name()
            )
        }),
        None => Ok(None),
    }
}

/// Read a loop's metadata file.
///
/// `version` is compared in `u32`, and an unparsable version is reported rather
/// than silently disabling endpoint smoothing.
pub fn parse_loop_metadata_xml(xml: &str) -> Result<LoopMetadata, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    if root.tag_name().name() != "loop" {
        return Err("loop data has bad format".into());
    }
    let version = parse_number::<u32>(root, "version", 0)?;
    Ok(LoopMetadata {
        smooth_end: version >= LOOP_FORMAT_VERSION,
        nbeats: parse_optional_number::<i64>(root, "nbeats")?,
        pulse_length: parse_optional_number::<u32>(root, "pulselen")?,
    })
}

/// Read a scene file.
///
/// `default_loop_id` is the C++ loader's placement fallback for a `<loop>`
/// element without a `loopid`.
pub fn parse_scene_xml(xml: &str, default_loop_id: i32) -> Result<SceneLoad, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    if root.tag_name().name() != "scene" {
        return Err("scene data has bad format".into());
    }
    let mut scene = Scene {
        loops: Vec::new(),
        snapshots: Vec::new(),
    };
    for node in root.children().filter(|n| n.is_element()) {
        match node.tag_name().name() {
            "loop" => {
                let loop_id = parse_number::<i32>(node, "loopid", default_loop_id)?;
                let hash = node
                    .attribute("hash")
                    .filter(|hash| !hash.is_empty())
                    .ok_or_else(|| {
                        format!("scene definition for loop (id {loop_id}) has missing hash")
                    })?;
                scene.loops.push(LoopMeta {
                    hash: hash.to_owned(),
                    loop_id,
                    volume: parse_number::<f32>(node, "volume", 1.0)?,
                });
            }
            "snapshot" => {
                let loops = node
                    .children()
                    .filter(|n| n.is_element() && n.tag_name().name() == "loopsnapshot")
                    .map(|n| {
                        Ok(SnapshotLoop {
                            loop_id: parse_number::<i32>(n, "loopid", 0)?,
                            status: parse_number::<i32>(n, "status", 0)?,
                            loop_volume: parse_number::<f32>(n, "loopvol", 0.0)?,
                            trigger_volume: parse_number::<f32>(n, "triggervol", 0.0)?,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                scene.snapshots.push(SnapshotMeta {
                    id: parse_number::<i32>(node, "snapid", 0)?,
                    name: node.attribute("name").unwrap_or_default().to_owned(),
                    loops,
                });
            }
            unknown => {
                return Err(format!("unexpected element <{unknown}> in scene"));
            }
        }
    }
    Ok(SceneLoad(scene))
}

/// Read a scene file with the default loop placement.
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
        assert_eq!(s.loops()[0].loop_id, 9);
        assert_eq!(s.loops()[0].volume, 1.0);
        assert_eq!(s.snapshots()[0].loops[0].loop_id, 0);
    }

    #[test]
    fn malformed_numbers_and_unknown_elements_are_reported() {
        let error = parse_scene_xml("<scene><loop hash=\"h\" volume=\"oops\"/></scene>", 0)
            .unwrap_err();
        assert!(error.contains("volume") && error.contains("oops"), "{error}");

        let error =
            parse_scene_xml("<scene><loops><loop hash=\"h\"/></loops></scene>", 0).unwrap_err();
        assert!(error.contains("loops"), "{error}");

        let error = parse_scene_xml("<scene><loop loopid=\"h\"/></scene>", 0).unwrap_err();
        assert!(error.contains("loopid"), "{error}");
    }

    #[test]
    fn an_empty_hash_is_rejected() {
        let error = parse_scene_xml("<scene><loop hash=\"\" loopid=\"4\"/></scene>", 0)
            .unwrap_err();
        assert!(error.contains("id 4"), "{error}");
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
        let error = parse_loop_metadata_xml("<loop version=\"one\"/>").unwrap_err();
        assert!(error.contains("version"), "{error}");
    }
}
