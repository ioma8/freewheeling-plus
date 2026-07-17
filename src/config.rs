/*
   Configuration system.
   Ported from fweelin_config.h/cc
   Handles XML-based configuration file parsing and variable management.
*/

use crate::datatypes::{CoreDataType, Range, UserVariable};
use crate::event::{Event, EventParameter, EventType, INTERFACEID};
use crate::fluidsynth::FluidSetting;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Maximum config path length
pub const CFG_PATH_MAX: usize = 2048;
pub const FWEELIN_CONFIG_DIR: &str = ".fweelin";
pub const FWEELIN_CONFIG_FILE: &str = "fweelin.xml";
pub const FWEELIN_CONFIG_EXT: &str = ".xml";
/// Upper bound for one renderer-facing variable snapshot.
pub const MAX_CONFIG_VARIABLE_SNAPSHOT: usize = 256;

// ============================================================
// Config expression for math operations
// ============================================================

#[derive(Debug, Clone)]
pub enum CfgOperation {
    Add(f32),
    Sub(f32),
    Mul(f32),
    Div(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfgTokenType {
    None,
    EventParameter,
    UserVariable,
    Static,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SdlKeyList {
    pub key: i32,
}

#[derive(Debug, Clone)]
pub struct CfgToken {
    pub cvt: CfgTokenType,
    pub var_name: Option<String>,
    pub evparam: Option<EventParameter>,
    pub val: UserVariable,
}

impl CfgToken {
    pub fn none() -> Self {
        Self {
            cvt: CfgTokenType::None,
            var_name: None,
            evparam: None,
            val: UserVariable::new(),
        }
    }

    pub fn evaluate(&self, cfg: &FloConfig, ev: Option<&dyn Event>) -> UserVariable {
        match self.cvt {
            CfgTokenType::None => UserVariable::new(),
            CfgTokenType::EventParameter => {
                if let (Some(param), Some(ev)) = (self.evparam, ev) {
                    cfg.read_event_parameter(ev, param)
                } else {
                    UserVariable::new()
                }
            }
            CfgTokenType::Static => self.val.clone(),
            CfgTokenType::UserVariable => self
                .var_name
                .as_deref()
                .and_then(|name| cfg.get_variable(name))
                .cloned()
                .unwrap_or_else(UserVariable::new),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CfgMathOperation {
    pub otype: char,
    pub operand: CfgToken,
}

#[derive(Debug, Clone)]
pub struct ParsedExpression {
    pub start: CfgToken,
    pub ops: Vec<CfgMathOperation>,
}

impl ParsedExpression {
    pub fn is_static(&self) -> bool {
        self.start.cvt == CfgTokenType::Static
            && self
                .ops
                .iter()
                .all(|op| op.operand.cvt == CfgTokenType::Static)
    }

    pub fn evaluate(&self, cfg: &FloConfig) -> UserVariable {
        self.evaluate_with_event(cfg, None)
    }

    pub fn evaluate_with_event(&self, cfg: &FloConfig, ev: Option<&dyn Event>) -> UserVariable {
        let mut cur = self.start.evaluate(cfg, ev);
        for op in &self.ops {
            let tmp = op.operand.evaluate(cfg, ev);
            cur = apply_math_operation(&cur, &tmp, op.otype);
        }
        cur
    }
}

fn apply_math_operation(left: &UserVariable, right: &UserVariable, op: char) -> UserVariable {
    let mut out = UserVariable::new();
    if left.get_type() == crate::datatypes::CoreDataType::Float
        || right.get_type() == crate::datatypes::CoreDataType::Float
    {
        let lhs = left.as_f32();
        let rhs = right.as_f32();
        let val = match op {
            '/' => lhs / rhs,
            '*' => lhs * rhs,
            '+' => lhs + rhs,
            '-' => lhs - rhs,
            _ => lhs,
        };
        out.set_float(val);
    } else {
        let lhs = left.as_i32();
        let rhs = right.as_i32();
        let val = match op {
            '/' => lhs / rhs,
            '*' => lhs * rhs,
            '+' => lhs + rhs,
            '-' => lhs - rhs,
            _ => lhs,
        };
        out.set_int(val);
    }
    out
}

#[derive(Debug, Clone)]
pub enum StoredParameterValue {
    Char(i8),
    Int(i32),
    Long(i64),
    Float(f32),
    Range(Range),
    Variable(UserVariable),
    VariableRef(Option<String>),
}

/// A copied value suitable for publishing to a non-realtime renderer.
///
/// `Raw` is retained for the legacy opaque variable kinds so a snapshot never
/// silently changes their type or bytes.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigVariableValue {
    Char(i8),
    Int(i32),
    Long(i64),
    Float(f32),
    Range(Range),
    Raw([u8; crate::datatypes::CFG_VAR_SIZE]),
}

/// One immutable copy of a configuration/system variable.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigVariableSnapshot {
    pub name: String,
    pub type_: CoreDataType,
    pub value: ConfigVariableValue,
    pub is_system: bool,
}

/// Bounded, deterministic collection returned by [`FloConfig::variable_snapshot`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConfigVariableSnapshotSet {
    pub variables: Vec<ConfigVariableSnapshot>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct DynamicToken {
    pub token: CfgToken,
    pub exp: ParsedExpression,
}

#[derive(Debug, Clone, Default)]
pub struct EventBinding {
    pub tokenconds: Vec<DynamicToken>,
    pub paramsets: Vec<DynamicToken>,
    pub static_paramsets: Vec<(String, StoredParameterValue)>,
    pub output_event: Option<EventType>,
    pub echo: bool,
    pub continued: bool,
    /// Compatibility marker for the shipped pckeyboard bindings only.
    pub pckeyboard_loop_ids: bool,
}

#[derive(Debug, Clone, Default)]
pub struct IndexedBindingTable {
    pub indexed_param: Option<EventParameter>,
    pub buckets: Vec<Vec<EventBinding>>,
}

#[derive(Debug, Clone)]
pub struct ResolvedBinding {
    pub binding: EventBinding,
    pub parameters: Vec<(String, StoredParameterValue)>,
}

#[derive(Debug, Clone)]
pub struct BindingDispatchResult {
    pub echo: bool,
    pub matched: Vec<ResolvedBinding>,
}

#[derive(Debug, Clone, Default)]
pub struct BindingRegistry {
    pub tables: HashMap<EventType, IndexedBindingTable>,
}

impl BindingRegistry {
    pub fn table_for(&self, typ: EventType) -> Option<&IndexedBindingTable> {
        self.tables.get(&typ)
    }
}

/// Configuration file parser and variable system
pub struct FloConfig {
    variables: HashMap<String, UserVariable>,
    key_bindings: HashMap<String, Vec<String>>,
    midi_bindings: HashMap<String, Vec<String>>,
    /// Whether we're running on macOS
    pub is_macos: bool,
    /// Audio memory length in seconds
    pub audio_memory_len: f32,
    /// Number of preallocated audio blocks
    pub num_preallocated_audio_blocks: usize,
    /// Number of preallocated time markers
    pub num_preallocated_time_markers: usize,
    /// Number of external MIDI output ports. C++ clamps this to at least one.
    pub midi_outputs: usize,
    /// Zero-based external MIDI ports that receive transport sync. XML uses
    /// one-based values in `midisyncouts`, as does the original C++ config.
    pub midi_sync_outputs: Vec<usize>,
    /// C++ `FloConfig::transpose`, adjusted by `adjust-midi-transpose` and
    /// applied to externally echoed notes.
    pub midi_transpose: i32,
    /// `externalaudioinputs`: one entry per external input, true for stereo.
    pub external_audio_input_stereo: Vec<bool>,
    /// `audioinputmonitoring`: software-monitoring state per input.
    pub audio_input_monitoring: Vec<bool>,
    /// `streaminputs`: disk-stream state per input.
    pub stream_inputs: Vec<bool>,
    pub stream_final_mix: bool,
    pub stream_loop_mix: bool,
    pub max_play_volume: f32,
    pub max_limiter_gain: f32,
    pub limiter_threshold: f32,
    pub limiter_release_rate: f32,
    pub vorbis_encode_quality: f32,
    /// C++ `audiobuffersize`; the backend may negotiate another valid size.
    pub preferred_audio_buffer_frames: u32,
    /// Config data directory
    pub data_dir: String,
    /// Library directory for loops/scenes
    pub library_dir: String,
    /// Maximum dB represented by a full-scale fader position
    pub fader_max_db: f32,
    /// Audio codec used by `save-loop`; defaults to the legacy Vorbis value.
    pub loop_output_format: crate::block::Codec,
    /// Audio codec used by disk streaming; defaults to the legacy Vorbis value.
    pub stream_output_format: crate::block::Codec,
    /// Bindings loaded from the base file and all configured interfaces.
    pub binding_registry: BindingRegistry,
    /// Interface files in configured order, with their C++ compatible IDs.
    pub interfaces: Vec<InterfaceConfig>,
    pub video: VideoConfig,
    /// Runtime state for XML `paramset` displays.  Keeping this next to the
    /// binding registry lets paramset get/set events take effect in-order,
    /// including when a continued binding reads the value immediately after
    /// a `paramset-get-param` output.
    pub paramsets: HashMap<(i32, i32), crate::paramset::FloDisplayParamSet>,
    pub patch_banks: Vec<PatchBankConfig>,
    /// C++ `<fluidsynth>` declarations from the primary configuration file.
    pub fluidsynth: FluidSynthConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FluidSynthConfig {
    pub stereo: bool,
    /// The raw FluidSynth interpolation-method value.  It is retained even
    /// when a backend cannot expose a setter for it.
    pub interpolation: i32,
    pub channel: u8,
    pub tuning_cents: f64,
    pub settings: Vec<FluidSetting>,
    pub soundfonts: Vec<PathBuf>,
}

impl Default for FluidSynthConfig {
    fn default() -> Self {
        Self {
            stereo: true,
            interpolation: 4,
            channel: 0,
            tuning_cents: 0.0,
            settings: Vec::new(),
            soundfonts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceConfig {
    pub id: i32,
    pub switchable: bool,
    pub setup: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutConfig {
    pub interface_id: i32,
    pub id: i32,
    pub name: Option<String>,
    pub show: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontConfig {
    pub name: String,
    pub file: PathBuf,
    pub size: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoConfig {
    pub width: u32,
    pub height: u32,
    pub delay_ms: u32,
    pub fonts: Vec<FontConfig>,
    pub layouts: Vec<LayoutConfig>,
    pub display_count: usize,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            width: 640,
            height: 480,
            delay_ms: 50,
            fonts: Vec::new(),
            layouts: Vec::new(),
            display_count: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PatchBankConfig {
    pub interface_id: i32,
    pub patches: PathBuf,
    pub midi_port: u32,
    pub separate_channels: bool,
    pub suppress_program_changes: bool,
    pub tag: Option<i32>,
}

/// Input binding matrix.  The legacy implementation is the configuration
/// subsystem's event-binding owner; retain the established Rust owner while
/// exposing the original parity name to callers.
pub type InputMatrix = FloConfig;

impl FloConfig {
    pub const NUM_PREALLOCATED_AUDIO_BLOCKS: usize = 40;
    pub const NUM_PREALLOCATED_TIME_MARKERS: usize = 40;
    pub const AUDIO_MEMORY_LEN: f32 = 10.0;

    pub fn new() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        FloConfig {
            variables: HashMap::new(),
            key_bindings: HashMap::new(),
            midi_bindings: HashMap::new(),
            is_macos: cfg!(target_os = "macos"),
            audio_memory_len: Self::AUDIO_MEMORY_LEN,
            num_preallocated_audio_blocks: Self::NUM_PREALLOCATED_AUDIO_BLOCKS,
            num_preallocated_time_markers: Self::NUM_PREALLOCATED_TIME_MARKERS,
            midi_outputs: 1,
            midi_sync_outputs: Vec::new(),
            midi_transpose: 0,
            external_audio_input_stereo: Vec::new(),
            audio_input_monitoring: Vec::new(),
            stream_inputs: Vec::new(),
            stream_final_mix: false,
            stream_loop_mix: false,
            max_play_volume: 0.0,
            max_limiter_gain: 1.0,
            limiter_threshold: 0.9,
            limiter_release_rate: 0.000_020,
            vorbis_encode_quality: 0.5,
            preferred_audio_buffer_frames: 128,
            data_dir: Self::default_data_dir(),
            library_dir: format!("{}/.fweelin/fw-lib", home),
            fader_max_db: 12.0,
            loop_output_format: crate::block::Codec::Vorbis,
            stream_output_format: crate::block::Codec::Vorbis,
            binding_registry: BindingRegistry::default(),
            interfaces: Vec::new(),
            video: VideoConfig::default(),
            paramsets: HashMap::new(),
            patch_banks: Vec::new(),
            fluidsynth: FluidSynthConfig::default(),
        }
    }

    fn default_data_dir() -> String {
        if cfg!(target_os = "macos") {
            // For macOS, check bundle Resources first, then fall back
            if let Ok(exe) = std::env::current_exe()
                && let Some(parent) = exe.parent()
            {
                let bundle_resources = parent.join("..").join("Resources");
                if bundle_resources.exists() {
                    return bundle_resources.to_string_lossy().to_string();
                }
            }
            // Fall back to compiled-in data dir
            std::env::var("FWEELIN_DATADIR")
                .unwrap_or_else(|_| "/opt/homebrew/share/fweelin".to_string())
        } else {
            "/usr/local/share/fweelin".to_string()
        }
    }

    pub fn build_config_path(dir: &Path, name: &str) -> PathBuf {
        dir.join(name)
    }

    pub fn copy_file_contents(src: &Path, dst: &Path) -> Result<(), String> {
        let bytes =
            fs::read(src).map_err(|e| format!("INIT: Error reading '{}': {}", src.display(), e))?;
        fs::write(dst, bytes)
            .map_err(|e| format!("INIT: Error writing '{}': {}", dst.display(), e))?;
        Ok(())
    }

    pub fn user_config_dir_from_home(home: &Path) -> PathBuf {
        Self::build_config_path(home, FWEELIN_CONFIG_DIR)
    }

    pub fn user_config_path_from_home(home: &Path, cfgname: &str) -> PathBuf {
        Self::build_config_path(&Self::user_config_dir_from_home(home), cfgname)
    }

    pub fn next_backup_path(existing_path: &Path) -> PathBuf {
        for idx in 1u16..=255 {
            let candidate = PathBuf::from(format!("{}.backup.{}", existing_path.display(), idx));
            if !candidate.exists() {
                return candidate;
            }
        }
        PathBuf::from(format!("{}.backup.255", existing_path.display()))
    }

    pub fn copy_config_file_in_paths(
        &self,
        cfgname: &str,
        copyall: bool,
        data_dir: &Path,
        home: &Path,
    ) -> Result<(), String> {
        if copyall {
            let entries = fs::read_dir(data_dir).map_err(|e| {
                format!(
                    "INIT: Error reading shared config folder '{}': {}",
                    data_dir.display(),
                    e
                )
            })?;
            for entry in entries {
                let entry =
                    entry.map_err(|e| format!("INIT: Error scanning config folder: {}", e))?;
                let path = entry.path();
                let name = match path.file_name().and_then(|s| s.to_str()) {
                    Some(name) if name.ends_with(FWEELIN_CONFIG_EXT) => name.to_string(),
                    _ => continue,
                };
                self.copy_config_file_in_paths(&name, false, data_dir, home)?;
            }
            return Ok(());
        }

        let src = Self::build_config_path(data_dir, cfgname);
        if !src.exists() {
            return Ok(());
        }

        let config_dir = Self::user_config_dir_from_home(home);
        fs::create_dir_all(&config_dir).map_err(|e| {
            format!(
                "INIT: Error creating config folder '{}': {}",
                config_dir.display(),
                e
            )
        })?;
        let dst = Self::build_config_path(&config_dir, cfgname);

        if dst.exists() {
            let backup = Self::next_backup_path(&dst);
            Self::copy_file_contents(&dst, &backup)?;
        }

        Self::copy_file_contents(&src, &dst)
    }

    pub fn prepare_load_config_file_in_paths(
        &self,
        cfgname: &str,
        basecfg: bool,
        quiet: bool,
        data_dir: &Path,
        home: &Path,
    ) -> Result<PathBuf, String> {
        let config_dir = Self::user_config_dir_from_home(home);
        let user_cfg = Self::build_config_path(&config_dir, cfgname);
        if user_cfg.exists() {
            return Ok(user_cfg);
        }

        fs::create_dir_all(&config_dir).map_err(|e| {
            format!(
                "INIT: Error creating config folder '{}': {}",
                config_dir.display(),
                e
            )
        })?;

        for static_asset in ["bcf2000-help.txt", "bcf2000-preset.mid", "config-help.txt"] {
            let _ = self.copy_config_file_in_paths(static_asset, false, data_dir, home);
        }
        self.copy_config_file_in_paths(cfgname, basecfg, data_dir, home)?;

        if user_cfg.exists() {
            Ok(user_cfg)
        } else if quiet {
            Err(format!("Config file '{}' not found", cfgname))
        } else {
            Err(format!(
                "INIT: Can't find configuration file '{}'.",
                user_cfg.display()
            ))
        }
    }

    pub fn copy_config_file(&self, cfgname: &str, copyall: bool) -> Result<(), String> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        self.copy_config_file_in_paths(
            cfgname,
            copyall,
            Path::new(&self.data_dir),
            Path::new(&home),
        )
    }

    pub fn prepare_load_config_file(
        &self,
        cfgname: &str,
        basecfg: bool,
        quiet: bool,
    ) -> Result<PathBuf, String> {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        self.prepare_load_config_file_in_paths(
            cfgname,
            basecfg,
            quiet,
            Path::new(&self.data_dir),
            Path::new(&home),
        )
    }

    /// Get a variable by name
    pub fn get_variable(&self, name: &str) -> Option<&UserVariable> {
        self.variables.get(name)
    }

    /// Get a mutable variable by name
    pub fn get_variable_mut(&mut self, name: &str) -> Option<&mut UserVariable> {
        self.variables.get_mut(name)
    }

    /// Set a variable value
    pub fn set_variable(&mut self, name: &str, value: UserVariable) {
        self.variables.insert(name.to_string(), value);
    }

    /// Copy configuration and system variables for renderer synchronization.
    ///
    /// The result is name-sorted, capped at [`MAX_CONFIG_VARIABLE_SNAPSHOT`],
    /// and contains no references into this config or mutation capability.
    pub fn variable_snapshot(&self) -> ConfigVariableSnapshotSet {
        let mut names: Vec<&String> = self.variables.keys().collect();
        names.sort_unstable();
        let truncated = names.len() > MAX_CONFIG_VARIABLE_SNAPSHOT;
        let variables = names
            .into_iter()
            .take(MAX_CONFIG_VARIABLE_SNAPSHOT)
            .filter_map(|name| {
                let value = self.variables.get(name)?;
                Some(ConfigVariableSnapshot {
                    name: name.clone(),
                    type_: value.get_type(),
                    value: match value.get_type() {
                        CoreDataType::Char => ConfigVariableValue::Char(value.as_char()),
                        CoreDataType::Int => ConfigVariableValue::Int(value.as_i32()),
                        CoreDataType::Long => ConfigVariableValue::Long(value.as_i64()),
                        CoreDataType::Float => ConfigVariableValue::Float(value.as_f32()),
                        CoreDataType::Range => ConfigVariableValue::Range(value.as_range()),
                        CoreDataType::Variable
                        | CoreDataType::VariableRef
                        | CoreDataType::Invalid => {
                            ConfigVariableValue::Raw(value.get_value().try_into().unwrap())
                        }
                    },
                    is_system: value.is_system_variable(),
                })
            })
            .collect();
        ConfigVariableSnapshotSet {
            variables,
            truncated,
        }
    }

    /// Set a variable by name and float value
    pub fn set_float_variable(&mut self, name: &str, value: f32) {
        let mut v = UserVariable::new();
        v.set_float(value);
        self.variables.insert(name.to_string(), v);
    }

    /// Set a variable by name and int value
    pub fn set_int_variable(&mut self, name: &str, value: i32) {
        let mut v = UserVariable::new();
        v.set_int(value);
        self.variables.insert(name.to_string(), v);
    }

    /// Get a float variable value
    pub fn get_float(&self, name: &str) -> Option<f32> {
        self.variables.get(name).map(|v| v.as_f32())
    }

    /// Get an int variable value
    pub fn get_int(&self, name: &str) -> Option<i32> {
        self.variables.get(name).map(|v| v.as_i32())
    }

    /// Parse a configuration file
    pub fn parse_file(&mut self, path: &str) -> Result<(), String> {
        self.load_authoritative(Path::new(path))
    }

    /// Parse configuration string
    pub fn parse_string(&mut self, data: &str) -> Result<(), String> {
        let doc = roxmltree::Document::parse(data)
            .map_err(|e| format!("Invalid configuration XML: {e}"))?;
        let root = doc.root_element();
        if root.tag_name().name() != "freewheeling" {
            return Err("Config file format invalid: expected <freewheeling>".into());
        }
        self.load_documents(vec![(0, PathBuf::new(), data.to_owned())])
    }

    /// Load the FreeWheeling XML configuration, including DTD entities and
    /// interface/patch includes. User-relative include lookup follows the C++
    /// rule: the selected base configuration's directory wins.
    pub fn load_authoritative(&mut self, path: &Path) -> Result<(), String> {
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let main = expand_external_entities(path, &mut Vec::new())?;
        let doc = roxmltree::Document::parse(&main)
            .map_err(|e| format!("Invalid config '{}': {e}", path.display()))?;
        let root = doc.root_element();
        if root.tag_name().name() != "freewheeling" {
            return Err("Config file format invalid: expected <freewheeling>".into());
        }
        if root.attribute("version") != Some("0.6") {
            return Err(format!(
                "Unsupported FreeWheeling config version {:?}",
                root.attribute("version")
            ));
        }

        let mut specs = Vec::new();
        let mut switch_id = 1;
        let mut fixed_id = 1000;
        for node in root.descendants().filter(|n| n.has_tag_name("interface")) {
            let Some(setup) = node.attribute("setup") else {
                continue;
            };
            let switchable = node.attribute("switchable") != Some("0");
            let id = if switchable {
                let id = switch_id;
                switch_id += 1;
                id
            } else {
                let id = fixed_id;
                fixed_id += 1;
                id
            };
            let include = base.join(setup);
            let xml = expand_external_entities(&include, &mut Vec::new())?;
            let included = roxmltree::Document::parse(&xml)
                .map_err(|e| format!("Invalid interface '{}': {e}", include.display()))?;
            if !included.root_element().has_tag_name("interface") {
                return Err(format!(
                    "Interface '{}' must start with <interface>",
                    include.display()
                ));
            }
            specs.push((
                InterfaceConfig {
                    id,
                    switchable,
                    setup: include.clone(),
                },
                xml,
            ));
        }
        self.interfaces = specs.iter().map(|(s, _)| s.clone()).collect();
        let mut documents = vec![(0, path.to_path_buf(), main)];
        documents.extend(specs.into_iter().map(|(s, xml)| (s.id, s.setup, xml)));
        self.load_documents(documents)
    }

    fn load_documents(&mut self, documents: Vec<(i32, PathBuf, String)>) -> Result<(), String> {
        let mut registry = BindingRegistry::default();
        self.video = VideoConfig::default();
        self.paramsets.clear();
        self.patch_banks.clear();
        self.fluidsynth = FluidSynthConfig::default();
        // C++ performs a declaration pass over every interface before bindings.
        for (_, _, xml) in &documents {
            let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
            for node in doc.descendants().filter(|n| n.has_tag_name("declare")) {
                self.parse_binding_declare_node(node)?;
            }
        }
        for (iid, source, xml) in &documents {
            let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
            self.parse_runtime_nodes(
                *iid,
                source.parent().unwrap_or_else(|| Path::new(".")),
                &doc,
            )?;
            for node in doc.descendants().filter(|n| n.has_tag_name("binding")) {
                self.parse_binding_node(*iid, node, &mut registry)?;
            }
        }
        self.binding_registry = registry;
        Ok(())
    }

    fn parse_runtime_nodes(
        &mut self,
        iid: i32,
        base: &Path,
        doc: &roxmltree::Document<'_>,
    ) -> Result<(), String> {
        for node in doc.descendants().filter(|n| n.is_element()) {
            if node.has_tag_name("var") {
                for attr in node.attributes() {
                    match attr.name() {
                        "numloopids" => self
                            .set_int_variable("CONFIG_numloopids", positive_i32(attr.value(), 1)?),
                        "maxsnapshots" => self.set_int_variable(
                            "CONFIG_maxsnapshots",
                            positive_i32(attr.value(), 1)?,
                        ),
                        "librarypath" => {
                            self.library_dir =
                                expand_home(attr.value())?.trim_end_matches('/').to_owned()
                        }
                        "externalaudioinputs" => {
                            self.external_audio_input_stereo = attr
                                .value()
                                .bytes()
                                .map(|input| matches!(input, b'S' | b's'))
                                .collect();
                            // C++ allocates these after the topology is known.
                            self.audio_input_monitoring =
                                vec![false; self.external_audio_input_stereo.len()];
                            self.stream_inputs =
                                vec![false; self.external_audio_input_stereo.len()];
                        }
                        "audioinputmonitoring" => {
                            let len = self.external_audio_input_stereo.len();
                            self.audio_input_monitoring = (0..len)
                                .map(|index| {
                                    matches!(attr.value().as_bytes().get(index), Some(b'Y' | b'y'))
                                })
                                .collect();
                        }
                        "streaminputs" => {
                            let len = self.external_audio_input_stereo.len();
                            self.stream_inputs = (0..len)
                                .map(|index| {
                                    matches!(attr.value().as_bytes().get(index), Some(b'Y' | b'y'))
                                })
                                .collect();
                        }
                        "streamfinalmix" => {
                            self.stream_final_mix = attr
                                .value()
                                .as_bytes()
                                .first()
                                .is_some_and(|value| matches!(*value, b'Y' | b'y'))
                        }
                        "streamloopmix" => {
                            self.stream_loop_mix = attr
                                .value()
                                .as_bytes()
                                .first()
                                .is_some_and(|value| matches!(*value, b'Y' | b'y'))
                        }
                        "maxplayvol" => {
                            self.max_play_volume = attr
                                .value()
                                .parse::<f32>()
                                .map_err(|_| "invalid maxplayvol")?
                                .max(0.0)
                        }
                        "maxlimitergain" => {
                            let value = attr
                                .value()
                                .parse::<f32>()
                                .map_err(|_| "invalid maxlimitergain")?;
                            self.max_limiter_gain = if value < 0.0 { 1.0 } else { value };
                        }
                        "limiterthreshhold" => {
                            let value = attr
                                .value()
                                .parse::<f32>()
                                .map_err(|_| "invalid limiterthreshhold")?;
                            self.limiter_threshold = if value < 0.0 { 0.9 } else { value };
                        }
                        "limiterreleaserate" => {
                            let value = attr
                                .value()
                                .parse::<f32>()
                                .map_err(|_| "invalid limiterreleaserate")?;
                            self.limiter_release_rate = if value < 0.0 { 0.000_020 } else { value };
                        }
                        "fadermaxdb" => {
                            self.fader_max_db =
                                attr.value().parse().map_err(|_| "invalid fadermaxdb")?
                        }
                        "loopoutformat" => self.loop_output_format = parse_codec(attr.value())?,
                        "streamoutformat" => self.stream_output_format = parse_codec(attr.value())?,
                        "oggquality" => {
                            let value = attr
                                .value()
                                .parse::<f32>()
                                .map_err(|_| "invalid oggquality")?;
                            if value > 0.0 {
                                self.vorbis_encode_quality = value;
                            }
                        }
                        "midiouts" => {
                            self.midi_outputs = positive_i32(attr.value(), 1)? as usize;
                        }
                        "midisyncouts" => {
                            self.midi_sync_outputs = attr
                                .value()
                                .split(',')
                                .map(|port| {
                                    let port = port
                                        .trim()
                                        .parse::<usize>()
                                        .map_err(|_| "invalid midisyncouts")?;
                                    port.checked_sub(1).ok_or("midisyncouts ports start at 1")
                                })
                                .collect::<Result<Vec<_>, _>>()?;
                        }
                        "resolution" => {
                            let v = parse_pair(attr.value())?;
                            self.video.width = v.0 as u32;
                            self.video.height = v.1 as u32;
                        }
                        "videodelay" => {
                            self.video.delay_ms =
                                attr.value().parse().map_err(|_| "invalid videodelay")?
                        }
                        "audiobuffersize" => {
                            let value = attr
                                .value()
                                .parse::<i32>()
                                .map_err(|_| "invalid audiobuffersize")?;
                            if value > 0 {
                                self.preferred_audio_buffer_frames = value as u32;
                            }
                        }
                        _ => {}
                    }
                }
            } else if node.has_tag_name("font") {
                if let (Some(name), Some(file)) = (node.attribute("name"), node.attribute("file")) {
                    self.video.fonts.push(FontConfig {
                        name: name.into(),
                        file: base.join(file),
                        size: node
                            .attribute("size")
                            .unwrap_or("12")
                            .parse()
                            .map_err(|_| "invalid font size")?,
                    });
                }
            } else if node.has_tag_name("layout") {
                self.video.layouts.push(LayoutConfig {
                    interface_id: iid,
                    id: node
                        .attribute("id")
                        .unwrap_or("0")
                        .parse()
                        .map_err(|_| "invalid layout id")?,
                    name: node.attribute("name").map(str::to_owned),
                    show: node.attribute("show").unwrap_or("0") != "0",
                });
            } else if node.has_tag_name("display") {
                self.video.display_count += 1;
                if node.attribute("type") == Some("paramset") {
                    self.parse_paramset_node(iid, node)?;
                }
            } else if node.has_tag_name("fluidsynth") {
                if let Some(name) = node.attribute("param") {
                    let setting = if let Some(value) = node.attribute("setint") {
                        Some(FluidSetting::Integer {
                            name: name.to_owned(),
                            value: value.parse().map_err(|_| "invalid fluidsynth setint")?,
                        })
                    } else if let Some(value) = node.attribute("setnum") {
                        Some(FluidSetting::Number {
                            name: name.to_owned(),
                            value: value.parse().map_err(|_| "invalid fluidsynth setnum")?,
                        })
                    } else if let Some(value) = node.attribute("setstr") {
                        Some(FluidSetting::Text {
                            name: name.to_owned(),
                            value: value.to_owned(),
                        })
                    } else {
                        None
                    };
                    if let Some(setting) = setting {
                        self.fluidsynth.settings.push(setting);
                    }
                } else if let Some(font) = node.attribute("soundfont") {
                    let path = Path::new(font);
                    self.fluidsynth.soundfonts.push(if path.is_absolute() {
                        path.to_path_buf()
                    } else {
                        base.join(path)
                    });
                } else if let Some(value) = node.attribute("interpolation") {
                    self.fluidsynth.interpolation = value
                        .parse()
                        .map_err(|_| "invalid fluidsynth interpolation")?;
                } else if let Some(value) = node.attribute("tuning") {
                    self.fluidsynth.tuning_cents =
                        value.parse().map_err(|_| "invalid fluidsynth tuning")?;
                } else if let Some(value) = node.attribute("channel") {
                    self.fluidsynth.channel = value
                        .parse::<u8>()
                        .map_err(|_| "invalid fluidsynth channel")?
                        .min(15);
                } else if let Some(value) = node.attribute("stereo") {
                    self.fluidsynth.stereo = value != "0";
                }
            } else if node.has_tag_name("patchbank")
                && let Some(file) = node.attribute("patches")
            {
                let patches = base.join(file);
                let text = fs::read_to_string(&patches)
                    .map_err(|e| format!("Failed to read patches '{}': {e}", patches.display()))?;
                let pd = roxmltree::Document::parse(&text)
                    .map_err(|e| format!("Invalid patches '{}': {e}", patches.display()))?;
                if !pd.root_element().has_tag_name("patchlist") {
                    return Err(format!(
                        "Patches '{}' must start with <patchlist>",
                        patches.display()
                    ));
                }
                self.patch_banks.push(PatchBankConfig {
                    interface_id: iid,
                    patches,
                    midi_port: node
                        .attribute("midiport")
                        .unwrap_or("1")
                        .parse()
                        .map_err(|_| "invalid midiport")?,
                    separate_channels: node.attribute("separatechannels") == Some("1"),
                    suppress_program_changes: node.attribute("suppressprogramchanges") == Some("1"),
                    tag: node
                        .attribute("tag")
                        .map(str::parse)
                        .transpose()
                        .map_err(|_| "invalid patch tag")?,
                });
            }
        }
        Ok(())
    }

    fn parse_paramset_node(
        &mut self,
        iid: i32,
        node: roxmltree::Node<'_, '_>,
    ) -> Result<(), String> {
        let display_id = node
            .attribute("id")
            .map(|value| self.parse_expression(value, false).evaluate(self).as_i32())
            .unwrap_or(-1);
        let name = node.attribute("name").unwrap_or("NONAME");
        let num_active = node
            .attribute("numactiveparams")
            .unwrap_or("8")
            .parse::<usize>()
            .map_err(|_| format!("invalid paramset numactiveparams for {name}"))?;
        let banks = node
            .children()
            .filter(|child| child.has_tag_name("bank"))
            .collect::<Vec<_>>();
        let mut display = crate::paramset::FloDisplayParamSet::new(
            name,
            iid,
            display_id,
            num_active,
            banks.len(),
            100,
            100,
        );
        for (bank_index, bank_node) in banks.into_iter().enumerate() {
            let params = bank_node
                .children()
                .filter(|child| child.has_tag_name("param"))
                .collect::<Vec<_>>();
            let max_value = bank_node
                .attribute("maxvalue")
                .unwrap_or("1.0")
                .parse::<f32>()
                .map_err(|_| format!("invalid paramset maxvalue for {name}"))?;
            let bank_name = bank_node.attribute("name");
            let bank = display
                .banks
                .get_mut(bank_index)
                .ok_or_else(|| format!("invalid paramset bank index for {name}"))?;
            bank.setup(bank_name, params.len(), max_value);
            for (param_index, param_node) in params.into_iter().enumerate() {
                if let Some(param) = bank.params.get_mut(param_index) {
                    param.set_name(param_node.attribute("name"));
                    if let Some(init) = param_node.attribute("init") {
                        param.value = init
                            .parse()
                            .map_err(|_| format!("invalid paramset init for {name}"))?;
                    }
                }
            }
        }
        display.link_active_params();
        self.paramsets.insert((iid, display_id), display);
        Ok(())
    }

    /// Add an input binding
    pub fn add_binding(&mut self, input: &str, action: &str) {
        if let Some(input) = input.strip_prefix("key:") {
            self.key_bindings
                .entry(input.to_string())
                .or_default()
                .push(action.to_string());
        } else if let Some(input) = input.strip_prefix("midi:") {
            self.midi_bindings
                .entry(input.to_string())
                .or_default()
                .push(action.to_string());
        }
    }

    pub fn parse_binding_registry_xml(
        &self,
        interfaceid: i32,
        xml: &str,
    ) -> Result<BindingRegistry, String> {
        let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
        let mut registry = BindingRegistry::default();

        for node in doc
            .descendants()
            .filter(|node| node.has_tag_name("binding"))
        {
            self.parse_binding_node(interfaceid, node, &mut registry)?;
        }

        Ok(registry)
    }

    pub fn load_bindings_section_xml(
        &mut self,
        interfaceid: i32,
        xml: &str,
    ) -> Result<BindingRegistry, String> {
        let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
        let bindings = doc
            .descendants()
            .find(|node| node.has_tag_name("bindings"))
            .ok_or_else(|| "Missing <bindings> section".to_string())?;

        let mut registry = BindingRegistry::default();

        for node in bindings.children().filter(|node| node.is_element()) {
            if node.has_tag_name("declare") {
                self.parse_binding_declare_node(node)?;
            }
        }

        for node in bindings.children().filter(|node| node.is_element()) {
            if node.has_tag_name("binding") {
                self.parse_binding_node(interfaceid, node, &mut registry)?;
            }
        }

        Ok(registry)
    }

    pub fn load_bindings_section_file(
        &mut self,
        interfaceid: i32,
        path: &Path,
    ) -> Result<BindingRegistry, String> {
        let xml = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read bindings file '{}': {}", path.display(), e))?;
        self.load_bindings_section_xml(interfaceid, &xml)
    }

    fn parse_binding_declare_node(&mut self, node: roxmltree::Node<'_, '_>) -> Result<(), String> {
        let name = node
            .attribute("var")
            .ok_or_else(|| "Variable declaration is missing 'var'".to_string())?;
        let dtype_name = node
            .attribute("type")
            .ok_or_else(|| format!("Variable '{}' is missing type", name))?;
        let dtype = match dtype_name {
            "char" => CoreDataType::Char,
            "int" => CoreDataType::Int,
            "long" => CoreDataType::Long,
            "float" => CoreDataType::Float,
            "range" => CoreDataType::Range,
            _ => {
                return Err(format!(
                    "Variable '{}' uses unsupported type '{}'",
                    name, dtype_name
                ));
            }
        };

        let mut variable = UserVariable::with_name(name, dtype);
        if let Some(init) = node.attribute("init") {
            let value = self.parse_expression(init, false).evaluate(self);
            match self.store_parameter_value(dtype, &value) {
                StoredParameterValue::Char(v) => variable.set_char(v),
                StoredParameterValue::Int(v) => variable.set_int(v),
                StoredParameterValue::Long(v) => variable.set_long(v),
                StoredParameterValue::Float(v) => variable.set_float(v),
                StoredParameterValue::Range(v) => variable.set_range(v.lo, v.hi),
                _ => return Err(format!("Variable '{}' could not be initialized", name)),
            }
        }
        self.set_variable(name, variable);
        Ok(())
    }

    fn parse_binding_node(
        &self,
        interfaceid: i32,
        node: roxmltree::Node<'_, '_>,
        registry: &mut BindingRegistry,
    ) -> Result<(), String> {
        let input_name = node
            .attribute("input")
            .ok_or_else(|| "Invalid binding: missing input attribute".to_string())?;
        let input_type = EventType::from_name(input_name)
            .ok_or_else(|| format!("Invalid input event '{}'", input_name))?;
        if input_type as usize >= EventType::LastBindable as usize {
            return Err(format!("Event '{}' cannot be used as an input", input_name));
        }

        let table = registry.tables.entry(input_type).or_insert_with(|| {
            let params = input_type.parameters();
            if let Some(param) = params.iter().copied().find(|param| param.max_index >= 0) {
                IndexedBindingTable {
                    indexed_param: Some(param),
                    buckets: vec![Vec::new(); (param.max_index + 1) as usize],
                }
            } else {
                IndexedBindingTable {
                    indexed_param: None,
                    buckets: vec![Vec::new()],
                }
            }
        });

        let input_params = input_type.parameters();
        let indexed_param = table.indexed_param;
        let indexed_param_name = indexed_param.map(|param| param.name);
        let mut first_binding = EventBinding {
            echo: node
                .attribute("echo")
                .map(|value| value != "0")
                .unwrap_or(false),
            ..EventBinding::default()
        };
        // pckeyboard.xml historically used SDL/ASCII keysyms (97..122) as
        // loop ids. Keep this opt-in and structural; never infer it for an
        // arbitrary loop-id expression or arbitrary native input.
        let pckeyboard_loop_ids = input_type == EventType::InputKey
            && node
                .attribute("conditions")
                .is_some_and(|v| v.contains("key=VAR_pckeyrange"));
        let bucket_index = self.create_conditions_from_attr(
            interfaceid,
            &mut first_binding,
            node.attribute("conditions"),
            input_params,
            indexed_param,
        )?;

        let mut outputs = Vec::new();
        let mut output_idx = 0usize;
        loop {
            let attr = if output_idx == 0 {
                "output".to_string()
            } else {
                format!("output{}", output_idx)
            };
            let Some(output_name) = node.attribute(attr.as_str()) else {
                if output_idx == 0 {
                    output_idx += 1;
                    continue;
                }
                break;
            };
            let output_type = EventType::from_name(output_name)
                .ok_or_else(|| format!("Invalid output event '{}'", output_name))?;
            // Numbered chains in the shipped XML commonly begin at output1.
            // Conditions and echo belong to the first output that exists,
            // regardless of its numeric suffix.
            let mut binding = if outputs.is_empty() {
                first_binding.clone()
            } else {
                EventBinding::default()
            };
            binding.pckeyboard_loop_ids = pckeyboard_loop_ids;
            binding.output_event = Some(output_type);
            let params_attr = if output_idx == 0 {
                node.attribute("parameters")
            } else {
                node.attribute(format!("parameters{}", output_idx).as_str())
            };
            self.create_parameter_sets_from_attr(
                interfaceid,
                &mut binding,
                params_attr,
                input_params,
                output_type.parameters(),
            )?;
            outputs.push(binding);
            output_idx += 1;
        }

        if outputs.is_empty() {
            return Err("Invalid binding: no output attribute".to_string());
        }

        for idx in 0..outputs.len().saturating_sub(1) {
            outputs[idx].continued = true;
        }
        for binding in outputs {
            self.add_binding_to_table(table, bucket_index, binding);
        }

        if indexed_param_name.is_some() && bucket_index >= table.buckets.len() {
            return Err("Computed bucket index out of range".to_string());
        }

        Ok(())
    }

    fn create_conditions_from_attr(
        &self,
        interfaceid: i32,
        bind: &mut EventBinding,
        conditions_attr: Option<&str>,
        input_params: &[EventParameter],
        indexed_param: Option<EventParameter>,
    ) -> Result<usize, String> {
        let mut bucket_index = indexed_param
            .map(|param| param.max_index as usize)
            .unwrap_or(0usize);
        let mut interface_set = false;

        if let Some(conditions) = conditions_attr {
            for token in conditions.split(" and ") {
                let Some((lv, rv)) = token.split_once('=') else {
                    continue;
                };
                let lvalue = self.remove_spaces(lv);
                let rvalue = self.remove_spaces(rv);
                if let Some(param) = input_params
                    .iter()
                    .copied()
                    .find(|param| param.name == lvalue)
                {
                    if param.name == INTERFACEID {
                        interface_set = true;
                    }
                    let enable_keynames = param.name == "key";
                    let exp = self.parse_expression_with_event_schema(
                        rvalue,
                        input_params,
                        enable_keynames,
                    );
                    if indexed_param.map(|indexed| indexed.name) == Some(param.name) {
                        bucket_index = self.binding_bucket_index_from_expression(param, &exp)?;
                    }
                    bind.tokenconds.push(DynamicToken {
                        token: CfgToken {
                            cvt: CfgTokenType::EventParameter,
                            var_name: None,
                            evparam: Some(param),
                            val: UserVariable::new(),
                        },
                        exp,
                    });
                    continue;
                }
                if self.get_variable(lvalue).is_some() {
                    bind.tokenconds.push(DynamicToken {
                        token: CfgToken {
                            cvt: CfgTokenType::UserVariable,
                            var_name: Some(lvalue.to_string()),
                            evparam: None,
                            val: UserVariable::new(),
                        },
                        exp: self.parse_expression_with_event_schema(rvalue, input_params, false),
                    });
                }
            }
        }

        if !interface_set
            && let Some(param) = input_params
                .iter()
                .copied()
                .find(|param| param.name == INTERFACEID)
        {
            let exp = self.parse_expression_with_event_schema(
                &interfaceid.to_string(),
                input_params,
                false,
            );
            if indexed_param.map(|indexed| indexed.name) == Some(param.name) {
                bucket_index = self.binding_bucket_index_from_expression(param, &exp)?;
            }
            bind.tokenconds.push(DynamicToken {
                token: CfgToken {
                    cvt: CfgTokenType::EventParameter,
                    var_name: None,
                    evparam: Some(param),
                    val: UserVariable::new(),
                },
                exp,
            });
        }

        Ok(bucket_index)
    }

    fn create_parameter_sets_from_attr(
        &self,
        interfaceid: i32,
        bind: &mut EventBinding,
        params_attr: Option<&str>,
        input_params: &[EventParameter],
        output_params: &[EventParameter],
    ) -> Result<(), String> {
        let mut interface_set = false;
        if let Some(parameters) = params_attr {
            for token in parameters.split(" and ") {
                let Some((lv, rv)) = token.split_once('=') else {
                    continue;
                };
                let lvalue = self.remove_spaces(lv);
                let rvalue = self.remove_spaces(rv);
                let Some(param) = output_params
                    .iter()
                    .copied()
                    .find(|param| param.name == lvalue)
                else {
                    continue;
                };
                if param.name == INTERFACEID {
                    interface_set = true;
                }
                let enable_keynames = param.name == "key";
                let exp =
                    self.parse_expression_with_event_schema(rvalue, input_params, enable_keynames);
                if exp.is_static() {
                    let value = exp.evaluate(self);
                    bind.static_paramsets.push((
                        param.name.to_string(),
                        self.store_parameter_value(param.dtype, &value),
                    ));
                } else {
                    bind.paramsets.push(DynamicToken {
                        token: CfgToken {
                            cvt: CfgTokenType::EventParameter,
                            var_name: None,
                            evparam: Some(param),
                            val: UserVariable::new(),
                        },
                        exp,
                    });
                }
            }
        }

        if !interface_set
            && let Some(param) = output_params
                .iter()
                .copied()
                .find(|param| param.name == INTERFACEID)
        {
            let exp = self.parse_expression_with_event_schema(
                &interfaceid.to_string(),
                input_params,
                false,
            );
            let value = exp.evaluate(self);
            bind.static_paramsets.push((
                param.name.to_string(),
                self.store_parameter_value(param.dtype, &value),
            ));
        }

        Ok(())
    }

    fn parse_expression_with_event_schema(
        &self,
        expr: &str,
        reference_params: &[EventParameter],
        enable_keynames: bool,
    ) -> ParsedExpression {
        let operators = ['/', '*', '+', '-'];
        let mut indices = expr.char_indices().filter_map(|(idx, ch)| {
            if operators.contains(&ch) && idx != 0 {
                Some((idx, ch))
            } else {
                None
            }
        });

        let parse_token =
            |token: &str| self.parse_token_from_schema(token, reference_params, enable_keynames);
        if let Some((idx, op)) = indices.next() {
            let start = parse_token(&expr[..idx]);
            let mut ops = Vec::new();
            let mut prev_op = op;
            let mut prev_idx = idx;
            for (next_idx, next_op) in indices {
                ops.push(CfgMathOperation {
                    otype: prev_op,
                    operand: parse_token(&expr[prev_idx + 1..next_idx]),
                });
                prev_idx = next_idx;
                prev_op = next_op;
            }
            ops.push(CfgMathOperation {
                otype: prev_op,
                operand: parse_token(&expr[prev_idx + 1..]),
            });
            ParsedExpression { start, ops }
        } else {
            ParsedExpression {
                start: parse_token(expr),
                ops: Vec::new(),
            }
        }
    }

    fn parse_token_from_schema(
        &self,
        token: &str,
        reference_params: &[EventParameter],
        enable_keynames: bool,
    ) -> CfgToken {
        let token = token.trim();
        if token.is_empty() {
            return CfgToken::none();
        }

        if enable_keynames {
            let keysym = crate::sdlio::get_sdl_key(token);
            if keysym != crate::sdlio::FWL_SDLK_UNKNOWN {
                let mut val = UserVariable::new();
                val.set_int(keysym);
                return CfgToken {
                    cvt: CfgTokenType::Static,
                    var_name: None,
                    evparam: None,
                    val,
                };
            }
        }

        if let Some(param) = reference_params
            .iter()
            .copied()
            .find(|param| param.name == token)
        {
            return CfgToken {
                cvt: CfgTokenType::EventParameter,
                var_name: None,
                evparam: Some(param),
                val: UserVariable::new(),
            };
        }

        self.parse_token(token, false)
    }

    fn binding_bucket_index_from_expression(
        &self,
        indexed_param: EventParameter,
        exp: &ParsedExpression,
    ) -> Result<usize, String> {
        if exp.is_static() {
            Ok(self
                .binding_bucket_index(indexed_param, &exp.evaluate(self))
                .unwrap_or(indexed_param.max_index as usize))
        } else {
            Ok(indexed_param.max_index as usize)
        }
    }

    /// Check if a variable exists
    pub fn has_variable(&self, name: &str) -> bool {
        self.variables.contains_key(name)
    }

    /// List all variables
    pub fn list_variables(&self) -> Vec<String> {
        self.variables.keys().cloned().collect()
    }

    /// Create default configuration
    pub fn create_default(&mut self) {
        self.set_float_variable("master-out-volume", 0.8);
        self.set_float_variable("master-in-volume", 0.9);
        self.set_float_variable("new-loop-volume", 0.7);
        self.set_int_variable("num-input-channels", 2);
        self.set_int_variable("num-output-channels", 2);
        self.set_int_variable("buffer-size", 256);
        self.set_int_variable("sample-rate", 48000);
    }

    pub fn fader_max_db(&self) -> f32 {
        self.fader_max_db
    }

    pub fn is_stereo_input(&self, input_idx: usize) -> bool {
        self.external_audio_input_stereo
            .get(input_idx)
            .copied()
            // The C++ internal FluidSynth input follows all configured
            // external inputs and inherits its `<fluidsynth stereo>` flag.
            .unwrap_or_else(|| {
                input_idx == self.external_audio_input_stereo.len() && self.fluidsynth.stereo
            })
    }

    pub fn is_stereo_output(&self, _output_idx: usize) -> bool {
        self.is_stereo_master()
    }

    pub fn is_stereo_master(&self) -> bool {
        self.external_audio_input_stereo
            .iter()
            .any(|stereo| *stereo)
            || self.fluidsynth.stereo
    }

    pub fn ext_audio_inputs(&self) -> usize {
        if self.external_audio_input_stereo.is_empty() {
            self.get_int("num-input-channels").unwrap_or(2).max(0) as usize
        } else {
            self.external_audio_input_stereo.len()
        }
    }

    pub fn set_variable_from_string(&mut self, name: &str, value: &str) -> Result<(), String> {
        let var = self
            .get_variable_mut(name)
            .ok_or_else(|| format!("Unknown variable '{}'", name))?;
        set_user_variable_from_string(var, value)
    }

    pub fn read_event_parameter(&self, ev: &dyn Event, param: EventParameter) -> UserVariable {
        let mut value = UserVariable::new();
        match ev.get_type() {
            crate::event::EventType::InputKey => {
                if let Some(key) = ev.as_any().downcast_ref::<crate::event::KeyInputEvent>() {
                    match param.name {
                        "keydown" => value.set_char(if key.down { 1 } else { 0 }),
                        "key" => value.set_int(key.keysym),
                        "unicode" => value.set_int(key.unicode),
                        _ => {}
                    }
                }
            }
            crate::event::EventType::GoSub => {
                if let Some(sub) = ev.as_any().downcast_ref::<crate::event::GoSubEvent>() {
                    match param.name {
                        "sub" => value.set_int(sub.sub),
                        "param1" => value.set_float(sub.param1),
                        "param2" => value.set_float(sub.param2),
                        "param3" => value.set_float(sub.param3),
                        _ => {}
                    }
                }
            }
            crate::event::EventType::LoopClicked => {
                if let Some(loop_click) =
                    ev.as_any().downcast_ref::<crate::event::LoopClickedEvent>()
                {
                    match param.name {
                        "down" => value.set_char(if loop_click.down { 1 } else { 0 }),
                        "button" => value.set_int(loop_click.button),
                        "loopid" => value.set_int(loop_click.loopid),
                        "in" => value.set_char(if loop_click.in_layout { 1 } else { 0 }),
                        _ => {}
                    }
                }
            }
            crate::event::EventType::InputJoystickButton => {
                if let Some(joy) = ev
                    .as_any()
                    .downcast_ref::<crate::event::JoystickButtonInputEvent>()
                {
                    match param.name {
                        "down" => value.set_char(if joy.down { 1 } else { 0 }),
                        "button" => value.set_int(joy.button),
                        "joystick" => value.set_int(joy.joystick),
                        _ => {}
                    }
                }
            }
            crate::event::EventType::InputMouseButton => {
                if let Some(mouse) = ev
                    .as_any()
                    .downcast_ref::<crate::event::MouseButtonInputEvent>()
                {
                    match param.name {
                        "down" => value.set_char(if mouse.down { 1 } else { 0 }),
                        "button" => value.set_int(mouse.button),
                        "x" => value.set_int(mouse.x),
                        "y" => value.set_int(mouse.y),
                        _ => {}
                    }
                }
            }
            crate::event::EventType::InputMouseMotion => {
                if let Some(mouse) = ev
                    .as_any()
                    .downcast_ref::<crate::event::MouseMotionInputEvent>()
                {
                    match param.name {
                        "x" => value.set_int(mouse.x),
                        "y" => value.set_int(mouse.y),
                        _ => {}
                    }
                }
            }
            crate::event::EventType::InputMIDIKey => {
                if let Some(midi) = ev
                    .as_any()
                    .downcast_ref::<crate::event::MIDIKeyInputEvent>()
                {
                    match param.name {
                        "outport" => value.set_int(midi.outport),
                        "keydown" => value.set_char(if midi.down { 1 } else { 0 }),
                        "midichannel" => value.set_int(midi.channel as i32),
                        "notenum" => value.set_int(midi.notenum as i32),
                        "velocity" => value.set_int(midi.vel as i32),
                        "routethroughpatch" => value.set_char(if midi.echo { 1 } else { 0 }),
                        _ => {}
                    }
                }
            }
            crate::event::EventType::InputMIDIController => {
                if let Some(midi) = ev
                    .as_any()
                    .downcast_ref::<crate::event::MIDIControllerInputEvent>()
                {
                    match param.name {
                        "outport" => value.set_int(midi.outport),
                        "midichannel" => value.set_int(midi.channel as i32),
                        "controlnum" => value.set_int(midi.ctrl as i32),
                        "controlval" => value.set_int(midi.val as i32),
                        "routethroughpatch" => value.set_char(if midi.echo { 1 } else { 0 }),
                        _ => {}
                    }
                }
            }
            crate::event::EventType::InputMIDIProgramChange => {
                if let Some(midi) = ev
                    .as_any()
                    .downcast_ref::<crate::event::MIDIProgramChangeInputEvent>()
                {
                    match param.name {
                        "outport" => value.set_int(midi.outport),
                        "midichannel" => value.set_int(midi.channel as i32),
                        "programval" => value.set_int(midi.val as i32),
                        "routethroughpatch" => value.set_char(if midi.echo { 1 } else { 0 }),
                        _ => {}
                    }
                }
            }
            crate::event::EventType::InputMIDIChannelPressure => {
                if let Some(midi) = ev
                    .as_any()
                    .downcast_ref::<crate::event::MIDIChannelPressureInputEvent>()
                {
                    match param.name {
                        "outport" => value.set_int(midi.outport),
                        "midichannel" => value.set_int(midi.channel as i32),
                        "pressureval" => value.set_int(midi.val as i32),
                        "routethroughpatch" => value.set_char(if midi.echo { 1 } else { 0 }),
                        _ => {}
                    }
                }
            }
            crate::event::EventType::InputMIDIPitchBend => {
                if let Some(midi) = ev
                    .as_any()
                    .downcast_ref::<crate::event::MIDIPitchBendInputEvent>()
                {
                    match param.name {
                        "outport" => value.set_int(midi.outport),
                        "midichannel" => value.set_int(midi.channel as i32),
                        "pitchval" => value.set_int(midi.val),
                        "routethroughpatch" => value.set_char(if midi.echo { 1 } else { 0 }),
                        _ => {}
                    }
                }
            }
            crate::event::EventType::InputMIDIClock => {
                if let Some(midi) = ev
                    .as_any()
                    .downcast_ref::<crate::event::MIDIClockInputEvent>()
                    && param.name == "outport"
                {
                    value.set_int(midi.outport);
                }
            }
            crate::event::EventType::InputMIDIStartStop => {
                if let Some(midi) = ev
                    .as_any()
                    .downcast_ref::<crate::event::MIDIStartStopInputEvent>()
                {
                    match param.name {
                        "outport" => value.set_int(midi.outport),
                        "start" => value.set_char(if midi.start { 1 } else { 0 }),
                        _ => {}
                    }
                }
            }
            crate::event::EventType::BrowserMoveToItem => {
                if let Some(browser) = ev
                    .as_any()
                    .downcast_ref::<crate::event::BrowserMoveToItemEvent>()
                {
                    match param.name {
                        "browserid" => value.set_int(browser.browserid),
                        "adjust" => value.set_int(browser.adjust),
                        "jumpadjust" => value.set_int(browser.jump_adjust),
                        _ => {}
                    }
                }
            }
            crate::event::EventType::BrowserMoveToItemAbsolute => {
                if let Some(browser) = ev
                    .as_any()
                    .downcast_ref::<crate::event::BrowserMoveToItemAbsoluteEvent>()
                {
                    match param.name {
                        "browserid" => value.set_int(browser.browserid),
                        "idx" => value.set_int(browser.index),
                        _ => {}
                    }
                }
            }
            crate::event::EventType::StartInterface => {
                if let Some(start) = ev
                    .as_any()
                    .downcast_ref::<crate::event::StartInterfaceEvent>()
                    && param.name == crate::event::INTERFACEID
                {
                    value.set_int(start.interfaceid);
                }
            }
            crate::event::EventType::SlideInVolume => {
                if let Some(slide) = ev
                    .as_any()
                    .downcast_ref::<crate::event::SlideInVolumeEvent>()
                {
                    match param.name {
                        "input" => value.set_int(slide.input),
                        "slide" => value.set_float(slide.slide),
                        _ => {}
                    }
                }
            }
            crate::event::EventType::SetInVolume => {
                if let Some(set) = ev.as_any().downcast_ref::<crate::event::SetInVolumeEvent>() {
                    match param.name {
                        "input" => value.set_int(set.input),
                        "vol" => value.set_float(set.vol),
                        "fadervol" => value.set_float(set.fadervol),
                        _ => {}
                    }
                }
            }
            crate::event::EventType::TriggerLoop => {
                if let Some(loop_ev) = ev.as_any().downcast_ref::<crate::event::TriggerLoopEvent>()
                {
                    match param.name {
                        "loopid" => value.set_int(loop_ev.index),
                        "vol" => value.set_float(loop_ev.vol),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        value
    }

    pub fn remove_spaces<'a>(&self, text: &'a str) -> &'a str {
        text.trim_matches(' ')
    }

    pub fn add_one_key(&self, keys: &mut Vec<SdlKeyList>, text: &str) {
        let keyname = self.remove_spaces(text);
        let keysym = crate::sdlio::get_sdl_key(keyname);
        if keysym != crate::sdlio::FWL_SDLK_UNKNOWN {
            keys.push(SdlKeyList { key: keysym });
        }
    }

    pub fn extract_keys(&self, text: &str) -> Vec<SdlKeyList> {
        let mut keys = Vec::new();
        for token in text.split(',') {
            self.add_one_key(&mut keys, token);
        }
        keys
    }

    pub fn parse_token(&self, token: &str, enable_keynames: bool) -> CfgToken {
        self.parse_token_with_event(token, None, enable_keynames)
    }

    pub fn parse_token_with_event(
        &self,
        token: &str,
        reference_event: Option<&dyn Event>,
        enable_keynames: bool,
    ) -> CfgToken {
        let token = token.trim();
        if token.is_empty() {
            return CfgToken::none();
        }

        if enable_keynames {
            let keysym = crate::sdlio::get_sdl_key(token);
            if keysym != crate::sdlio::FWL_SDLK_UNKNOWN {
                let mut val = UserVariable::new();
                val.set_int(keysym);
                return CfgToken {
                    cvt: CfgTokenType::Static,
                    var_name: None,
                    evparam: None,
                    val,
                };
            }
        }

        if let Some(reference_event) = reference_event {
            for idx in 0..reference_event.get_num_params() {
                if let Some(param) = reference_event.get_param(idx)
                    && param.name == token
                {
                    return CfgToken {
                        cvt: CfgTokenType::EventParameter,
                        var_name: None,
                        evparam: Some(param),
                        val: UserVariable::new(),
                    };
                }
            }
        }

        if self.get_variable(token).is_some() {
            return CfgToken {
                cvt: CfgTokenType::UserVariable,
                var_name: Some(token.to_string()),
                evparam: None,
                val: UserVariable::new(),
            };
        }

        if !token
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, ' ' | '.' | '>' | ',' | '-'))
        {
            return CfgToken::none();
        }

        let mut val = UserVariable::new();
        if set_user_variable_from_string(&mut val, token).is_err() {
            return CfgToken::none();
        }

        CfgToken {
            cvt: CfgTokenType::Static,
            var_name: None,
            evparam: None,
            val,
        }
    }

    pub fn parse_expression(&self, expr: &str, enable_keynames: bool) -> ParsedExpression {
        self.parse_expression_with_event(expr, None, enable_keynames)
    }

    pub fn parse_expression_with_event(
        &self,
        expr: &str,
        reference_event: Option<&dyn Event>,
        enable_keynames: bool,
    ) -> ParsedExpression {
        let operators = ['/', '*', '+', '-'];
        let mut indices = expr.char_indices().filter_map(|(idx, ch)| {
            if operators.contains(&ch) && idx != 0 {
                Some((idx, ch))
            } else {
                None
            }
        });

        let first = indices.next();
        let start = if let Some((idx, op)) = first {
            let start = self.parse_token_with_event(&expr[..idx], reference_event, enable_keynames);
            let mut ops = Vec::new();
            let mut prev_op = op;
            let mut prev_idx = idx;
            for (next_idx, next_op) in indices {
                let operand = self.parse_token_with_event(
                    &expr[prev_idx + 1..next_idx],
                    reference_event,
                    enable_keynames,
                );
                ops.push(CfgMathOperation {
                    otype: prev_op,
                    operand,
                });
                prev_idx = next_idx;
                prev_op = next_op;
            }
            let operand = self.parse_token_with_event(
                &expr[prev_idx + 1..],
                reference_event,
                enable_keynames,
            );
            ops.push(CfgMathOperation {
                otype: prev_op,
                operand,
            });
            return ParsedExpression { start, ops };
        } else {
            self.parse_token_with_event(expr, reference_event, enable_keynames)
        };

        ParsedExpression {
            start,
            ops: Vec::new(),
        }
    }

    pub fn store_parameter_value(
        &self,
        dtype: CoreDataType,
        value: &UserVariable,
    ) -> StoredParameterValue {
        match dtype {
            CoreDataType::Char => StoredParameterValue::Char(value.as_char()),
            CoreDataType::Int => StoredParameterValue::Int(value.as_i32()),
            CoreDataType::Long => StoredParameterValue::Long(value.as_i64()),
            CoreDataType::Float => StoredParameterValue::Float(value.as_f32()),
            CoreDataType::Range => StoredParameterValue::Range(value.as_range()),
            CoreDataType::Variable => StoredParameterValue::Variable(value.clone()),
            CoreDataType::VariableRef => StoredParameterValue::VariableRef(value.name.clone()),
            CoreDataType::Invalid => StoredParameterValue::VariableRef(None),
        }
    }

    pub fn store_variable_ref_from_expression(
        &self,
        expression: &ParsedExpression,
    ) -> StoredParameterValue {
        if expression.start.cvt == CfgTokenType::UserVariable {
            StoredParameterValue::VariableRef(expression.start.var_name.clone())
        } else {
            StoredParameterValue::VariableRef(None)
        }
    }

    pub fn check_conditions(&self, input: &dyn Event, bind: &EventBinding) -> bool {
        for cond in &bind.tokenconds {
            let cmp1 = cond.token.evaluate(self, Some(input));
            let cmp2 = cond.exp.evaluate_with_event(self, Some(input));
            if !condition_values_match(&cmp1, &cmp2) {
                return false;
            }
        }
        true
    }

    pub fn set_dynamic_parameters(
        &self,
        input: &dyn Event,
        bind: &EventBinding,
    ) -> Vec<(String, StoredParameterValue)> {
        let mut out = bind.static_paramsets.clone();
        for paramset in &bind.paramsets {
            if paramset.token.cvt != CfgTokenType::EventParameter {
                continue;
            }
            let Some(evparam) = paramset.token.evparam else {
                continue;
            };
            let stored = if evparam.dtype == CoreDataType::VariableRef {
                self.store_variable_ref_from_expression(&paramset.exp)
            } else {
                let value = paramset.exp.evaluate_with_event(self, Some(input));
                self.store_parameter_value(evparam.dtype, &value)
            };
            out.push((evparam.name.to_string(), stored));
        }
        out
    }

    pub fn match_binding<'a>(
        &self,
        input: &dyn Event,
        bindings: &'a [EventBinding],
    ) -> Option<&'a EventBinding> {
        self.match_binding_index(input, bindings)
            .and_then(|idx| bindings.get(idx))
    }

    pub fn match_binding_index(
        &self,
        input: &dyn Event,
        bindings: &[EventBinding],
    ) -> Option<usize> {
        let mut prev_cont = false;
        for (idx, bind) in bindings.iter().enumerate() {
            if !prev_cont && self.check_conditions(input, bind) {
                return Some(idx);
            }
            prev_cont = bind.continued;
        }
        None
    }

    pub fn collect_continued_bindings<'a>(
        &self,
        first_index: usize,
        bindings: &'a [EventBinding],
    ) -> Vec<&'a EventBinding> {
        let mut matched = Vec::new();
        let mut idx = first_index;
        while let Some(bind) = bindings.get(idx) {
            matched.push(bind);
            if !bind.continued {
                break;
            }
            idx += 1;
        }
        matched
    }

    pub fn first_indexed_param(ev: &dyn Event) -> Option<EventParameter> {
        (0..ev.get_num_params())
            .filter_map(|idx| ev.get_param(idx))
            .find(|param| param.max_index >= 0)
    }

    pub fn create_indexed_binding_table(&self, ev: &dyn Event) -> IndexedBindingTable {
        if let Some(param) = Self::first_indexed_param(ev) {
            IndexedBindingTable {
                indexed_param: Some(param),
                buckets: vec![Vec::new(); (param.max_index + 1) as usize],
            }
        } else {
            IndexedBindingTable {
                indexed_param: None,
                buckets: vec![Vec::new()],
            }
        }
    }

    pub fn binding_bucket_index(
        &self,
        param: EventParameter,
        value: &UserVariable,
    ) -> Option<usize> {
        if param.dtype != CoreDataType::Int || param.max_index < 0 {
            return None;
        }
        let raw = value.as_i32();
        if raw < 0 {
            Some(param.max_index as usize)
        } else {
            Some((raw % param.max_index) as usize)
        }
    }

    pub fn event_bucket_index(&self, ev: &dyn Event, table: &IndexedBindingTable) -> Option<usize> {
        let param = table.indexed_param?;
        let value = self.read_event_parameter(ev, param);
        self.binding_bucket_index(param, &value)
    }

    pub fn add_binding_to_table(
        &self,
        table: &mut IndexedBindingTable,
        bucket_index: usize,
        binding: EventBinding,
    ) {
        if let Some(bucket) = table.buckets.get_mut(bucket_index) {
            bucket.push(binding);
        }
    }

    pub fn match_binding_from_table<'a>(
        &self,
        input: &dyn Event,
        table: &'a IndexedBindingTable,
    ) -> Option<&'a EventBinding> {
        if table.indexed_param.is_none() {
            return self.match_binding(input, table.buckets.first()?);
        }

        let exact_idx = self.event_bucket_index(input, table)?;
        if let Some(exact_bucket) = table.buckets.get(exact_idx)
            && let Some(found) = self.match_binding(input, exact_bucket)
        {
            return Some(found);
        }

        let wildcard_idx = table.indexed_param?.max_index as usize;
        self.match_binding(input, table.buckets.get(wildcard_idx)?)
    }

    fn resolve_dispatch_from_bucket(
        &self,
        input: &dyn Event,
        bindings: &[EventBinding],
    ) -> Option<BindingDispatchResult> {
        let first_index = self.match_binding_index(input, bindings)?;
        let matched = self
            .collect_continued_bindings(first_index, bindings)
            .into_iter()
            .map(|binding| ResolvedBinding {
                binding: binding.clone(),
                parameters: self.set_dynamic_parameters(input, binding),
            })
            .collect::<Vec<_>>();
        Some(BindingDispatchResult {
            echo: bindings[first_index].echo,
            matched,
        })
    }

    pub fn dispatch_event_bindings(
        &self,
        input: &dyn Event,
        table: &IndexedBindingTable,
    ) -> BindingDispatchResult {
        if table.indexed_param.is_none() {
            if let Some(result) = table
                .buckets
                .first()
                .and_then(|bucket| self.resolve_dispatch_from_bucket(input, bucket))
            {
                return result;
            }
        } else if let Some(exact_idx) = self.event_bucket_index(input, table) {
            if let Some(result) = table
                .buckets
                .get(exact_idx)
                .and_then(|bucket| self.resolve_dispatch_from_bucket(input, bucket))
            {
                return result;
            }

            let wildcard_idx = table.indexed_param.map(|param| param.max_index as usize);
            if let Some(result) = wildcard_idx
                .and_then(|idx| table.buckets.get(idx))
                .and_then(|bucket| self.resolve_dispatch_from_bucket(input, bucket))
            {
                return result;
            }
        }

        let echo = !matches!(
            input.get_type(),
            crate::event::EventType::GoSub | crate::event::EventType::StartInterface
        );
        BindingDispatchResult {
            echo,
            matched: Vec::new(),
        }
    }

    pub fn dispatch_registered_event_bindings(
        &self,
        input: &dyn Event,
        registry: &BindingRegistry,
    ) -> BindingDispatchResult {
        if let Some(table) = registry.table_for(input.get_type()) {
            self.dispatch_event_bindings(input, table)
        } else {
            let echo = !matches!(
                input.get_type(),
                crate::event::EventType::GoSub | crate::event::EventType::StartInterface
            );
            BindingDispatchResult {
                echo,
                matched: Vec::new(),
            }
        }
    }

    pub fn emit_bound_events(
        &self,
        input: &dyn Event,
        table: &IndexedBindingTable,
    ) -> Result<(bool, Vec<Box<dyn Event>>), String> {
        let dispatch = self.dispatch_event_bindings(input, table);
        let mut out = Vec::with_capacity(dispatch.matched.len());
        for resolved in &dispatch.matched {
            out.push(self.instantiate_bound_event(resolved)?);
        }
        Ok((dispatch.echo, out))
    }

    pub fn emit_registered_events(
        &self,
        input: &dyn Event,
        registry: &BindingRegistry,
    ) -> Result<(bool, Vec<Box<dyn Event>>), String> {
        if let Some(table) = registry.table_for(input.get_type()) {
            self.emit_bound_events(input, table)
        } else {
            let echo = !matches!(
                input.get_type(),
                crate::event::EventType::GoSub | crate::event::EventType::StartInterface
            );
            Ok((echo, Vec::new()))
        }
    }

    fn instantiate_bound_event(
        &self,
        resolved: &ResolvedBinding,
    ) -> Result<Box<dyn Event>, String> {
        let output = resolved
            .binding
            .output_event
            .ok_or_else(|| "Resolved binding is missing output event type".to_string())?;

        match output {
            EventType::StartSession => Ok(Box::new(crate::event::StartSessionEvent::new())),
            EventType::ExitSession => Ok(Box::new(crate::event::ExitSessionEvent::new())),
            EventType::GoSub => {
                let sub = self.required_int_param(&resolved.parameters, "sub")?;
                let param1 = self.required_float_param(&resolved.parameters, "param1")?;
                let param2 = self.required_float_param(&resolved.parameters, "param2")?;
                let param3 = self.required_float_param(&resolved.parameters, "param3")?;
                Ok(Box::new(crate::event::GoSubEvent::new(
                    sub, param1, param2, param3,
                )))
            }
            EventType::ALSAMixerControlSet => {
                let hwid = self.required_int_param(&resolved.parameters, "hwid")?;
                let numid = self.required_int_param(&resolved.parameters, "numid")?;
                let val1 = self.required_int_param(&resolved.parameters, "val1")?;
                let val2 = self.required_int_param(&resolved.parameters, "val2")?;
                let val3 = self.required_int_param(&resolved.parameters, "val3")?;
                let val4 = self.required_int_param(&resolved.parameters, "val4")?;
                Ok(Box::new(crate::event::ALSAMixerControlSetEvent::new(
                    hwid, numid, val1, val2, val3, val4,
                )))
            }
            EventType::ParamSetGetAbsoluteParamIdx => {
                let interfaceid = self.required_int_param(&resolved.parameters, INTERFACEID)?;
                let displayid = self.required_int_param(&resolved.parameters, "displayid")?;
                let paramidx = self.required_int_param(&resolved.parameters, "paramidx")?;
                let absidx_name =
                    self.required_variable_ref_param(&resolved.parameters, "absidx")?;
                Ok(Box::new(
                    crate::event::ParamSetGetAbsoluteParamIdxEvent::new(
                        interfaceid,
                        displayid,
                        paramidx,
                        Some(absidx_name),
                    ),
                ))
            }
            EventType::ParamSetGetParam => {
                let interfaceid = self.required_int_param(&resolved.parameters, INTERFACEID)?;
                let displayid = self.required_int_param(&resolved.parameters, "displayid")?;
                let paramidx = self.required_int_param(&resolved.parameters, "paramidx")?;
                let var_name = self.required_variable_ref_param(&resolved.parameters, "var")?;
                Ok(Box::new(crate::event::ParamSetGetParamEvent::new(
                    interfaceid,
                    displayid,
                    paramidx,
                    Some(var_name),
                )))
            }
            EventType::ParamSetSetParam => {
                let interfaceid = self.required_int_param(&resolved.parameters, INTERFACEID)?;
                let displayid = self.required_int_param(&resolved.parameters, "displayid")?;
                let paramidx = self.required_int_param(&resolved.parameters, "paramidx")?;
                let value = self.required_float_param(&resolved.parameters, "value")?;
                Ok(Box::new(crate::event::ParamSetSetParamEvent::new(
                    interfaceid,
                    displayid,
                    paramidx,
                    value,
                )))
            }
            EventType::LogFaderVolToLinear => {
                let var_name = self.required_variable_ref_param(&resolved.parameters, "var")?;
                let fadervol = self.required_variable_param(&resolved.parameters, "fadervol")?;
                let scale = self.required_float_param(&resolved.parameters, "scale")?;
                Ok(Box::new(crate::event::LogFaderVolToLinearEvent::new(
                    Some(var_name),
                    fadervol,
                    scale,
                )))
            }
            EventType::TriggerLoop => {
                let loopid = self.required_int_param(&resolved.parameters, "loopid")?;
                let vol = self.required_float_param(&resolved.parameters, "vol")?;
                Ok(Box::new(crate::event::TriggerLoopEvent::new(loopid, vol)))
            }
            EventType::SetInVolume => {
                let input = self.required_int_param(&resolved.parameters, "input")?;
                let vol = self.required_float_param(&resolved.parameters, "vol")?;
                let fadervol = self.required_float_param(&resolved.parameters, "fadervol")?;
                Ok(Box::new(crate::event::SetInVolumeEvent::new(
                    input, vol, fadervol,
                )))
            }
            EventType::SlideInVolume => {
                let input = self.required_int_param(&resolved.parameters, "input")?;
                let slide = self.required_float_param(&resolved.parameters, "slide")?;
                Ok(Box::new(crate::event::SlideInVolumeEvent::new(
                    input, slide,
                )))
            }
            EventType::StartInterface => {
                let interfaceid = self.required_int_param(&resolved.parameters, INTERFACEID)?;
                Ok(Box::new(crate::event::StartInterfaceEvent::new(
                    interfaceid,
                )))
            }
            EventType::VideoSwitchInterface => {
                let interfaceid = self.required_int_param(&resolved.parameters, INTERFACEID)?;
                Ok(Box::new(crate::event::VideoSwitchInterfaceEvent::new(
                    interfaceid,
                )))
            }
            EventType::VideoShowDisplay => {
                let interfaceid = self.required_int_param(&resolved.parameters, INTERFACEID)?;
                let displayid = self.required_int_param(&resolved.parameters, "displayid")?;
                let show = self.required_bool_param(&resolved.parameters, "show")?;
                Ok(Box::new(crate::event::VideoShowDisplayEvent::new(
                    interfaceid,
                    displayid,
                    show,
                )))
            }
            EventType::VideoShowLayout => {
                let interfaceid = self.required_int_param(&resolved.parameters, INTERFACEID)?;
                let layoutid = self.required_int_param(&resolved.parameters, "layoutid")?;
                let show = self.required_bool_param(&resolved.parameters, "show")?;
                let hideothers = self.required_bool_param(&resolved.parameters, "hideothers")?;
                Ok(Box::new(crate::event::VideoShowLayoutEvent::new(
                    interfaceid,
                    layoutid,
                    show,
                    hideothers,
                )))
            }
            EventType::VideoShowHelp => {
                let page = self.required_int_param(&resolved.parameters, "page")?;
                Ok(Box::new(crate::event::VideoShowHelpEvent::new(page)))
            }
            EventType::VideoFullScreen => {
                let fullscreen = self.required_bool_param(&resolved.parameters, "fullscreen")?;
                Ok(Box::new(crate::event::VideoFullScreenEvent::new(
                    fullscreen,
                )))
            }
            EventType::ShowDebugInfo => {
                let show = self.required_bool_param(&resolved.parameters, "show")?;
                Ok(Box::new(crate::event::ShowDebugInfoEvent::new(show)))
            }
            EventType::VideoShowLoop => {
                let interfaceid = self.required_int_param(&resolved.parameters, INTERFACEID)?;
                let layoutid = self.required_int_param(&resolved.parameters, "layoutid")?;
                let loopid = self.required_range_param(&resolved.parameters, "loopid")?;
                Ok(Box::new(crate::event::VideoShowLoopEvent::new(
                    interfaceid,
                    layoutid,
                    loopid,
                )))
            }
            EventType::VideoShowSnapshotPage => {
                let interfaceid = self.required_int_param(&resolved.parameters, INTERFACEID)?;
                let displayid = self.required_int_param(&resolved.parameters, "displayid")?;
                let page = self.required_int_param(&resolved.parameters, "page")?;
                Ok(Box::new(crate::event::VideoShowSnapshotPageEvent::new(
                    interfaceid,
                    displayid,
                    page,
                )))
            }
            EventType::VideoShowParamSetBank => {
                let interfaceid = self.required_int_param(&resolved.parameters, INTERFACEID)?;
                let displayid = self.required_int_param(&resolved.parameters, "displayid")?;
                let bank = self.required_int_param(&resolved.parameters, "bank")?;
                Ok(Box::new(crate::event::VideoShowParamSetBankEvent::new(
                    interfaceid,
                    displayid,
                    bank,
                )))
            }
            EventType::VideoShowParamSetPage => {
                let interfaceid = self.required_int_param(&resolved.parameters, INTERFACEID)?;
                let displayid = self.required_int_param(&resolved.parameters, "displayid")?;
                let page = self.required_int_param(&resolved.parameters, "page")?;
                Ok(Box::new(crate::event::VideoShowParamSetPageEvent::new(
                    interfaceid,
                    displayid,
                    page,
                )))
            }
            EventType::FluidSynthEnable => {
                let enable = self.required_bool_param(&resolved.parameters, "enable")?;
                Ok(Box::new(crate::event::FluidSynthEnableEvent::new(enable)))
            }
            EventType::SetMidiTuning => {
                let tuning = self.required_float_param(&resolved.parameters, "tuning")?;
                Ok(Box::new(crate::event::SetMidiTuningEvent::new(tuning)))
            }
            EventType::SlideMasterInVolume => {
                let slide = self.required_float_param(&resolved.parameters, "slide")?;
                Ok(Box::new(crate::event::SlideMasterInVolumeEvent::new(slide)))
            }
            EventType::SlideMasterOutVolume => {
                let slide = self.required_float_param(&resolved.parameters, "slide")?;
                Ok(Box::new(crate::event::SlideMasterOutVolumeEvent::new(
                    slide,
                )))
            }
            EventType::BrowserMoveToItem => {
                let browserid = self.required_int_param(&resolved.parameters, "browserid")?;
                let adjust = self.required_int_param(&resolved.parameters, "adjust")?;
                let jumpadjust = self.required_int_param(&resolved.parameters, "jumpadjust")?;
                Ok(Box::new(crate::event::BrowserMoveToItemEvent::new(
                    browserid, adjust, jumpadjust,
                )))
            }
            EventType::BrowserMoveToItemAbsolute => {
                let browserid = self.required_int_param(&resolved.parameters, "browserid")?;
                let idx = self.required_int_param(&resolved.parameters, "idx")?;
                Ok(Box::new(crate::event::BrowserMoveToItemAbsoluteEvent::new(
                    browserid, idx,
                )))
            }
            EventType::BrowserSelectItem => {
                let browserid = self.required_int_param(&resolved.parameters, "browserid")?;
                Ok(Box::new(crate::event::BrowserSelectItemEvent::new(
                    browserid,
                )))
            }
            EventType::BrowserRenameItem => {
                let browserid = self.required_int_param(&resolved.parameters, "browserid")?;
                Ok(Box::new(crate::event::BrowserRenameItemEvent::new(
                    browserid,
                )))
            }
            EventType::BrowserItemBrowsed => {
                let browserid = self.required_int_param(&resolved.parameters, "browserid")?;
                Ok(Box::new(crate::event::BrowserItemBrowsedEvent::new(
                    browserid,
                )))
            }
            EventType::PatchBrowserMoveToBank => {
                let direction = self.required_int_param(&resolved.parameters, "direction")?;
                Ok(Box::new(crate::event::PatchBrowserMoveToBankEvent::new(
                    direction,
                )))
            }
            EventType::PatchBrowserMoveToBankByIndex => {
                let idx = self.required_int_param(&resolved.parameters, "idx")?;
                Ok(Box::new(
                    crate::event::PatchBrowserMoveToBankByIndexEvent::new(idx),
                ))
            }
            EventType::SetMasterInVolume => {
                let vol = self.required_float_param(&resolved.parameters, "vol")?;
                let fadervol = self.required_float_param(&resolved.parameters, "fadervol")?;
                Ok(Box::new(crate::event::SetMasterInVolumeEvent::new(
                    vol, fadervol,
                )))
            }
            EventType::SetMasterOutVolume => {
                let vol = self.required_float_param(&resolved.parameters, "vol")?;
                let fadervol = self.required_float_param(&resolved.parameters, "fadervol")?;
                Ok(Box::new(crate::event::SetMasterOutVolumeEvent::new(
                    vol, fadervol,
                )))
            }
            EventType::ToggleInputRecord => {
                let input = self.required_int_param(&resolved.parameters, "input")?;
                Ok(Box::new(crate::event::ToggleInputRecordEvent::new(input)))
            }
            EventType::SetMidiEchoPort => {
                let echoport = self.required_int_param(&resolved.parameters, "echoport")?;
                Ok(Box::new(crate::event::SetMidiEchoPortEvent::new(echoport)))
            }
            EventType::SetMidiEchoChannel => {
                let echochannel = self.required_int_param(&resolved.parameters, "echochannel")?;
                Ok(Box::new(crate::event::SetMidiEchoChannelEvent::new(
                    echochannel,
                )))
            }
            EventType::AdjustMidiTranspose => {
                let adjust = self.required_int_param(&resolved.parameters, "adjust")?;
                Ok(Box::new(crate::event::AdjustMidiTransposeEvent::new(
                    adjust,
                )))
            }
            EventType::SetTriggerVolume => {
                let loopid = self.required_int_param(&resolved.parameters, "loopid")?;
                let vol = self.required_float_param(&resolved.parameters, "vol")?;
                Ok(Box::new(crate::event::SetTriggerVolumeEvent::new(
                    loopid, vol,
                )))
            }
            EventType::SlideLoopAmp => {
                let loopid = self.required_int_param(&resolved.parameters, "loopid")?;
                let slide = self.required_float_param(&resolved.parameters, "slide")?;
                Ok(Box::new(crate::event::SlideLoopAmpEvent::new(
                    loopid, slide,
                )))
            }
            EventType::SetLoopAmp => {
                let loopid = self.required_int_param(&resolved.parameters, "loopid")?;
                let amp = self.required_float_param(&resolved.parameters, "amp")?;
                Ok(Box::new(crate::event::SetLoopAmpEvent::new(loopid, amp)))
            }
            EventType::AdjustLoopAmp => {
                let loopid = self.required_int_param(&resolved.parameters, "loopid")?;
                let ampfactor = self.required_float_param(&resolved.parameters, "ampfactor")?;
                Ok(Box::new(crate::event::AdjustLoopAmpEvent::new(
                    loopid, ampfactor,
                )))
            }
            EventType::RenameLoop => {
                let loopid = self.required_int_param(&resolved.parameters, "loopid")?;
                let in_layout = self.required_bool_param(&resolved.parameters, "in")?;
                Ok(Box::new(crate::event::RenameLoopEvent::new(
                    loopid, in_layout,
                )))
            }
            EventType::ToggleSelectLoop => {
                let setid = self.required_int_param(&resolved.parameters, "setid")?;
                let loopid = self.required_int_param(&resolved.parameters, "loopid")?;
                Ok(Box::new(crate::event::ToggleSelectLoopEvent::new(
                    setid, loopid,
                )))
            }
            EventType::SelectOnlyPlayingLoops => {
                let setid = self.required_int_param(&resolved.parameters, "setid")?;
                let playing = self.required_bool_param(&resolved.parameters, "playing")?;
                Ok(Box::new(crate::event::SelectOnlyPlayingLoopsEvent::new(
                    setid, playing,
                )))
            }
            EventType::SelectAllLoops => {
                let setid = self.required_int_param(&resolved.parameters, "setid")?;
                let select = self.required_bool_param(&resolved.parameters, "select")?;
                Ok(Box::new(crate::event::SelectAllLoopsEvent::new(
                    setid, select,
                )))
            }
            EventType::TriggerSelectedLoops => {
                let setid = self.required_int_param(&resolved.parameters, "setid")?;
                let vol = self.required_float_param(&resolved.parameters, "vol")?;
                let toggleloops = self.required_bool_param(&resolved.parameters, "toggleloops")?;
                Ok(Box::new(crate::event::TriggerSelectedLoopsEvent::new(
                    setid,
                    vol,
                    toggleloops,
                )))
            }
            EventType::SetSelectedLoopsTriggerVolume => {
                let setid = self.required_int_param(&resolved.parameters, "setid")?;
                let vol = self.required_float_param(&resolved.parameters, "vol")?;
                Ok(Box::new(
                    crate::event::SetSelectedLoopsTriggerVolumeEvent::new(setid, vol),
                ))
            }
            EventType::AdjustSelectedLoopsAmp => {
                let setid = self.required_int_param(&resolved.parameters, "setid")?;
                let ampfactor = self.required_float_param(&resolved.parameters, "ampfactor")?;
                Ok(Box::new(crate::event::AdjustSelectedLoopsAmpEvent::new(
                    setid, ampfactor,
                )))
            }
            EventType::InvertSelection => {
                let setid = self.required_int_param(&resolved.parameters, "setid")?;
                Ok(Box::new(crate::event::InvertSelectionEvent::new(setid)))
            }
            EventType::CreateSnapshot => {
                let snapid = self.required_int_param(&resolved.parameters, "snapid")?;
                Ok(Box::new(crate::event::CreateSnapshotEvent::new(snapid)))
            }
            EventType::SwapSnapshots => {
                let snapid1 = self.required_int_param(&resolved.parameters, "snapid1")?;
                let snapid2 = self.required_int_param(&resolved.parameters, "snapid2")?;
                Ok(Box::new(crate::event::SwapSnapshotsEvent::new(
                    snapid1, snapid2,
                )))
            }
            EventType::RenameSnapshot => {
                let snapid = self.required_int_param(&resolved.parameters, "snapid")?;
                Ok(Box::new(crate::event::RenameSnapshotEvent::new(snapid)))
            }
            EventType::TriggerSnapshot => {
                let snapid = self.required_int_param(&resolved.parameters, "snapid")?;
                Ok(Box::new(crate::event::TriggerSnapshotEvent::new(snapid)))
            }
            EventType::MoveLoop => {
                let oldloopid = self.required_int_param(&resolved.parameters, "oldloopid")?;
                let newloopid = self.required_int_param(&resolved.parameters, "newloopid")?;
                Ok(Box::new(crate::event::MoveLoopEvent::new(
                    oldloopid, newloopid,
                )))
            }
            EventType::EraseLoop => {
                let loopid = self.required_int_param(&resolved.parameters, "loopid")?;
                Ok(Box::new(crate::event::EraseLoopEvent::new(loopid)))
            }
            EventType::EraseAllLoops => Ok(Box::new(crate::event::EraseAllLoopsEvent::new())),
            EventType::EraseSelectedLoops => {
                let setid = self.required_int_param(&resolved.parameters, "setid")?;
                Ok(Box::new(crate::event::EraseSelectedLoopsEvent::new(setid)))
            }
            EventType::ToggleDiskOutput => Ok(Box::new(crate::event::ToggleDiskOutputEvent::new())),
            EventType::SetAutoLoopSaving => {
                let save = self.required_bool_param(&resolved.parameters, "save")?;
                Ok(Box::new(crate::event::SetAutoLoopSavingEvent::new(save)))
            }
            EventType::SaveLoop => {
                let loopid = self.required_int_param(&resolved.parameters, "loopid")?;
                Ok(Box::new(crate::event::SaveLoopEvent::new(loopid)))
            }
            EventType::SaveNewScene => Ok(Box::new(crate::event::SaveNewSceneEvent::new())),
            EventType::SaveCurrentScene => Ok(Box::new(crate::event::SaveCurrentSceneEvent::new())),
            EventType::SetLoadLoopId => {
                let loopid = self.required_int_param(&resolved.parameters, "loopid")?;
                Ok(Box::new(crate::event::SetLoadLoopIdEvent::new(loopid)))
            }
            EventType::SetDefaultLoopPlacement => {
                let looprange = self.required_range_param(&resolved.parameters, "looprange")?;
                Ok(Box::new(crate::event::SetDefaultLoopPlacementEvent::new(
                    looprange,
                )))
            }
            EventType::SelectPulse => {
                let pulse = self.required_int_param(&resolved.parameters, "pulse")?;
                Ok(Box::new(crate::event::SelectPulseEvent::new(pulse)))
            }
            EventType::DeletePulse => {
                let pulse = self.required_int_param(&resolved.parameters, "pulse")?;
                Ok(Box::new(crate::event::DeletePulseEvent::new(pulse)))
            }
            EventType::TapPulse => {
                let pulse = self.required_int_param(&resolved.parameters, "pulse")?;
                let newlen = self.required_bool_param(&resolved.parameters, "newlen")?;
                Ok(Box::new(crate::event::TapPulseEvent::new(pulse, newlen)))
            }
            EventType::SwitchMetronome => {
                let pulse = self.required_int_param(&resolved.parameters, "pulse")?;
                let metronome = self.required_bool_param(&resolved.parameters, "metronome")?;
                Ok(Box::new(crate::event::SwitchMetronomeEvent::new(
                    pulse, metronome,
                )))
            }
            EventType::SetSyncType => {
                let stype = self.required_bool_param(&resolved.parameters, "stype")?;
                Ok(Box::new(crate::event::SetSyncTypeEvent::new(stype)))
            }
            EventType::SetSyncSpeed => {
                let sspd = self.required_int_param(&resolved.parameters, "sspd")?;
                Ok(Box::new(crate::event::SetSyncSpeedEvent::new(sspd)))
            }
            EventType::SetMidiSync => {
                let midisync = self.required_int_param(&resolved.parameters, "midisync")?;
                Ok(Box::new(crate::event::SetMidiSyncEvent::new(midisync)))
            }
            EventType::PulseSync => Ok(Box::new(crate::event::PulseSyncEvent::new())),
            EventType::SlideLoopAmpStopAll => {
                Ok(Box::new(crate::event::SlideLoopAmpStopAllEvent::new()))
            }
            EventType::TransmitPlayingLoopsToDAW => {
                Ok(Box::new(crate::event::TransmitPlayingLoopsToDAWEvent::new()))
            }
            EventType::SetVariable => {
                let var_name = self.required_variable_ref_param(&resolved.parameters, "var")?;
                let value = self.required_variable_param(&resolved.parameters, "value")?;
                let maxjumpcheck = self
                    .required_bool_param(&resolved.parameters, "maxjumpcheck")
                    .unwrap_or(false);
                let maxjump = self
                    .optional_variable_param(&resolved.parameters, "maxjump")
                    .unwrap_or_default();
                Ok(Box::new(crate::event::SetVariableEvent::new(
                    Some(var_name),
                    value,
                    maxjumpcheck,
                    maxjump,
                )))
            }
            EventType::ToggleVariable => {
                let var_name = self.required_variable_ref_param(&resolved.parameters, "var")?;
                let maxvalue = self.required_int_param(&resolved.parameters, "maxvalue")?;
                let minvalue = self.required_int_param(&resolved.parameters, "minvalue")?;
                Ok(Box::new(crate::event::ToggleVariableEvent::new(
                    Some(var_name),
                    maxvalue,
                    minvalue,
                )))
            }
            EventType::SplitVariableMSBLSB => {
                let var = self.required_variable_param(&resolved.parameters, "var")?;
                let msb_name = self.required_variable_ref_param(&resolved.parameters, "msb")?;
                let lsb_name = self.required_variable_ref_param(&resolved.parameters, "lsb")?;
                Ok(Box::new(crate::event::SplitVariableMSBLSBEvent::new(
                    var,
                    Some(msb_name),
                    Some(lsb_name),
                )))
            }
            other => Err(format!(
                "Output event '{}' is not yet constructible from config bindings",
                other.name()
            )),
        }
    }

    fn required_int_param(
        &self,
        params: &[(String, StoredParameterValue)],
        name: &str,
    ) -> Result<i32, String> {
        let value = params
            .iter()
            .find(|(key, _)| key == name)
            .ok_or_else(|| format!("Missing required int parameter '{}'", name))?;
        match &value.1 {
            StoredParameterValue::Int(v) => Ok(*v),
            StoredParameterValue::Char(v) => Ok(*v as i32),
            StoredParameterValue::Long(v) => Ok(*v as i32),
            StoredParameterValue::Float(v) => Ok(*v as i32),
            other => Err(format!(
                "Parameter '{}' is not coercible to int: {:?}",
                name, other
            )),
        }
    }

    fn required_float_param(
        &self,
        params: &[(String, StoredParameterValue)],
        name: &str,
    ) -> Result<f32, String> {
        let value = params
            .iter()
            .find(|(key, _)| key == name)
            .ok_or_else(|| format!("Missing required float parameter '{}'", name))?;
        match &value.1 {
            StoredParameterValue::Float(v) => Ok(*v),
            StoredParameterValue::Int(v) => Ok(*v as f32),
            StoredParameterValue::Char(v) => Ok(*v as f32),
            StoredParameterValue::Long(v) => Ok(*v as f32),
            other => Err(format!(
                "Parameter '{}' is not coercible to float: {:?}",
                name, other
            )),
        }
    }

    fn required_bool_param(
        &self,
        params: &[(String, StoredParameterValue)],
        name: &str,
    ) -> Result<bool, String> {
        Ok(self.required_int_param(params, name)? != 0)
    }

    fn required_range_param(
        &self,
        params: &[(String, StoredParameterValue)],
        name: &str,
    ) -> Result<Range, String> {
        let value = params
            .iter()
            .find(|(key, _)| key == name)
            .ok_or_else(|| format!("Missing required range parameter '{}'", name))?;
        match &value.1 {
            StoredParameterValue::Range(v) => Ok(*v),
            other => Err(format!(
                "Parameter '{}' is not coercible to range: {:?}",
                name, other
            )),
        }
    }

    fn required_variable_param(
        &self,
        params: &[(String, StoredParameterValue)],
        name: &str,
    ) -> Result<UserVariable, String> {
        self.optional_variable_param(params, name)
            .ok_or_else(|| format!("Missing required variable parameter '{}'", name))
    }

    fn optional_variable_param(
        &self,
        params: &[(String, StoredParameterValue)],
        name: &str,
    ) -> Option<UserVariable> {
        params
            .iter()
            .find(|(key, _)| key == name)
            .and_then(|(_, value)| match value {
                StoredParameterValue::Variable(v) => Some(v.clone()),
                _ => None,
            })
    }

    fn required_variable_ref_param(
        &self,
        params: &[(String, StoredParameterValue)],
        name: &str,
    ) -> Result<String, String> {
        let value = params
            .iter()
            .find(|(key, _)| key == name)
            .ok_or_else(|| format!("Missing required variable-ref parameter '{}'", name))?;
        match &value.1 {
            StoredParameterValue::VariableRef(Some(v)) => Ok(v.clone()),
            StoredParameterValue::VariableRef(None) => Err(format!(
                "Parameter '{}' did not resolve to a variable reference",
                name
            )),
            other => Err(format!(
                "Parameter '{}' is not coercible to variable-ref: {:?}",
                name, other
            )),
        }
    }
}

impl Default for FloConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl crate::core_startup::StartupConfig for FloConfig {
    fn add_int_constant(&mut self, name: &str, value: i32) {
        self.set_int_variable(name, value);
    }
    fn add_empty_variable(&mut self, name: &str) {
        self.variables
            .entry(name.to_owned())
            .or_insert_with(|| UserVariable::with_name(name, CoreDataType::Int));
    }
    fn parse(&mut self) -> Result<(), String> {
        let path = self.prepare_load_config_file(FWEELIN_CONFIG_FILE, true, false)?;
        self.load_authoritative(&path)
    }
    fn start(&mut self) -> Result<(), String> {
        if self.interfaces.is_empty() {
            return Err("configuration has no loaded interfaces".into());
        }
        fs::create_dir_all(&self.library_dir).map_err(|e| {
            format!(
                "failed to create library directory '{}': {e}",
                self.library_dir
            )
        })
    }
}

fn positive_i32(value: &str, minimum: i32) -> Result<i32, String> {
    value
        .parse::<i32>()
        .map(|v| v.max(minimum))
        .map_err(|_| format!("invalid integer '{value}'"))
}

fn parse_codec(value: &str) -> Result<crate::block::Codec, String> {
    match value.trim().to_ascii_uppercase().as_str() {
        "OGG" | "VORBIS" => Ok(crate::block::Codec::Vorbis),
        "WAV" | "WAVE" => Ok(crate::block::Codec::Wav),
        "FLAC" => Ok(crate::block::Codec::Flac),
        "AU" | "SND" => Ok(crate::block::Codec::Au),
        _ => Err(format!("invalid audio output format '{value}'")),
    }
}

fn parse_pair(value: &str) -> Result<(i32, i32), String> {
    let mut values = value.split(',').map(str::trim);
    let x = values
        .next()
        .ok_or_else(|| format!("invalid pair '{value}'"))?
        .parse()
        .map_err(|_| format!("invalid pair '{value}'"))?;
    let y = values
        .next()
        .ok_or_else(|| format!("invalid pair '{value}'"))?
        .parse()
        .map_err(|_| format!("invalid pair '{value}'"))?;
    Ok((x, y))
}

fn expand_home(value: &str) -> Result<String, String> {
    if value == "~" || value.starts_with("~/") {
        let home = std::env::var("HOME").map_err(|_| format!("path '{value}' requires HOME"))?;
        return Ok(format!("{home}{}", &value[1..]));
    }
    Ok(value.to_owned())
}

fn expand_external_entities(path: &Path, stack: &mut Vec<PathBuf>) -> Result<String, String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("Failed to locate config '{}': {e}", path.display()))?;
    if stack.contains(&canonical) {
        return Err(format!("Recursive XML include at '{}'", path.display()));
    }
    stack.push(canonical);
    let mut xml = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read config '{}': {e}", path.display()))?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let mut entities = Vec::new();
    if let Some(start) = xml.find("<!DOCTYPE")
        && let Some(rel_end) = xml[start..].find("]>")
    {
        let end = start + rel_end + 2;
        let subset = &xml[start..end];
        for line in subset.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("<!ENTITY ")
                && let Some((name, rest)) = rest.split_once(" SYSTEM ")
                && let Some(file) = rest
                    .trim()
                    .strip_prefix('"')
                    .and_then(|s| s.split('"').next())
            {
                entities.push((name.trim().to_owned(), file.to_owned()));
            }
        }
        xml.replace_range(start..end, "");
    }
    for (name, file) in entities {
        let included = expand_external_entities(&base.join(file), stack)?;
        xml = xml.replace(&format!("&{name};"), &included);
    }
    stack.pop();
    Ok(xml)
}

fn set_user_variable_from_string(var: &mut UserVariable, value: &str) -> Result<(), String> {
    let value = value.trim();
    if let Some((lo, hi)) = value.split_once('>') {
        let lo = lo.trim().parse::<i32>().map_err(|e| e.to_string())?;
        let hi = hi.trim().parse::<i32>().map_err(|e| e.to_string())?;
        var.set_range(lo, hi);
    } else if value.contains('.') {
        let parsed = value.parse::<f32>().map_err(|e| e.to_string())?;
        var.set_float(parsed);
    } else {
        let parsed = value.parse::<i32>().map_err(|e| e.to_string())?;
        var.set_int(parsed);
    }
    Ok(())
}

fn user_variables_equal(left: &UserVariable, right: &UserVariable) -> bool {
    use CoreDataType::*;
    match (left.get_type(), right.get_type()) {
        (Range, Range) => left.as_range() == right.as_range(),
        (Float, _) | (_, Float) => (left.as_f32() - right.as_f32()).abs() < 0.0001,
        (Long, _) | (_, Long) => left.as_i64() == right.as_i64(),
        _ => left.as_i32() == right.as_i32(),
    }
}

/// Binding conditions in the XML use ranges as membership tests, for example
/// `key=VAR_pckeyrange` where `VAR_pckeyrange` is `97>122`.  General variable
/// equality intentionally retains its old scalar coercion, but conditions
/// must accept any scalar contained by either operand's range.
fn condition_values_match(left: &UserVariable, right: &UserVariable) -> bool {
    use CoreDataType::Range;

    match (left.get_type(), right.get_type()) {
        (Range, Range) => left.as_range() == right.as_range(),
        (Range, _) => {
            let range = left.as_range();
            (range.lo..=range.hi).contains(&right.as_i32())
        }
        (_, Range) => {
            let range = right.as_range();
            (range.lo..=range.hi).contains(&left.as_i32())
        }
        _ => user_variables_equal(left, right),
    }
}

#[cfg(test)]
mod authoritative_xml_tests {
    use super::*;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/fweelin.xml")
    }

    #[test]
    fn loads_shipped_config_and_all_interface_includes() {
        let mut cfg = FloConfig::new();
        cfg.load_authoritative(&fixture()).unwrap();
        assert_eq!(cfg.get_int("CONFIG_numloopids"), Some(1024));
        assert_eq!(cfg.get_int("CONFIG_maxsnapshots"), Some(100));
        assert_eq!(cfg.loop_output_format, crate::block::Codec::Vorbis);
        assert_eq!(cfg.stream_output_format, crate::block::Codec::Vorbis);
        assert_eq!(cfg.midi_outputs, 4);
        assert_eq!(cfg.midi_sync_outputs, vec![0]);
        assert_eq!(
            cfg.external_audio_input_stereo,
            vec![false, false, true, true]
        );
        assert_eq!(cfg.audio_input_monitoring, vec![true, true, true, true]);
        assert_eq!(cfg.stream_inputs, vec![false, true, true, false]);
        assert!(!cfg.stream_final_mix);
        assert!(cfg.stream_loop_mix);
        assert_eq!(cfg.max_play_volume, 0.0);
        assert_eq!(cfg.max_limiter_gain, 1.0);
        assert_eq!(cfg.limiter_threshold, 0.75);
        assert_eq!(cfg.limiter_release_rate, 0.000_020);
        assert_eq!(cfg.vorbis_encode_quality, 0.5);
        assert!(cfg.is_stereo_input(2));
        assert!(!cfg.is_stereo_input(0));
        assert!(cfg.is_stereo_master());
        assert_eq!(cfg.interfaces.len(), 8);
        assert_eq!(cfg.interfaces.iter().filter(|i| i.switchable).count(), 4);
        assert!(cfg.get_variable("VAR_overdubfeedback").is_some());
        assert!(!cfg.binding_registry.tables.is_empty());
        assert!(cfg.fluidsynth.stereo);
        assert_eq!(cfg.fluidsynth.interpolation, 1);
        assert_eq!(cfg.fluidsynth.channel, 0);
        assert!((cfg.fluidsynth.tuning_cents + 31.76).abs() < 0.001);
        assert!(cfg
            .fluidsynth
            .settings
            .iter()
            .any(|setting| matches!(setting, FluidSetting::Integer { name, value: 64 } if name == "synth.polyphony")));
        assert!(
            cfg.fluidsynth
                .soundfonts
                .iter()
                .any(|path| path.ends_with("basic.sf2"))
        );
    }

    #[test]
    fn loads_graphics_layouts_and_patch_lists_from_real_data() {
        let mut cfg = FloConfig::new();
        cfg.load_authoritative(&fixture()).unwrap();
        assert_eq!((cfg.video.width, cfg.video.height), (640, 480));
        assert!(cfg.video.fonts.iter().any(|f| f.name == "main"));
        assert!(
            cfg.video
                .layouts
                .iter()
                .any(|l| l.name.as_deref() == Some("PC Keyboard"))
        );
        assert!(cfg.video.display_count > 20);
        assert!(
            cfg.patch_banks
                .iter()
                .any(|bank| bank.patches.ends_with("patches-channels.xml"))
        );
    }

    #[test]
    fn variable_snapshot_is_sorted_typed_and_copies_values() {
        let mut cfg = FloConfig::new();
        cfg.set_float_variable("zeta", 1.25);
        cfg.set_int_variable("alpha", 42);
        let mut range = UserVariable::new();
        range.set_range(-2, 9);
        cfg.set_variable("middle", range);

        let snapshot = cfg.variable_snapshot();
        assert!(!snapshot.truncated);
        assert_eq!(
            snapshot
                .variables
                .iter()
                .map(|v| v.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "middle", "zeta"]
        );
        assert_eq!(snapshot.variables[0].type_, CoreDataType::Int);
        assert_eq!(snapshot.variables[0].value, ConfigVariableValue::Int(42));
        assert_eq!(
            snapshot.variables[1].value,
            ConfigVariableValue::Range(Range::new(-2, 9))
        );
        assert_eq!(
            snapshot.variables[2].value,
            ConfigVariableValue::Float(1.25)
        );

        cfg.set_int_variable("alpha", 99);
        assert_eq!(snapshot.variables[0].value, ConfigVariableValue::Int(42));
    }

    #[test]
    fn variable_snapshot_is_bounded_and_reports_truncation() {
        let mut cfg = FloConfig::new();
        for index in 0..=MAX_CONFIG_VARIABLE_SNAPSHOT {
            cfg.set_int_variable(&format!("var-{index:03}"), index as i32);
        }

        let snapshot = cfg.variable_snapshot();
        assert_eq!(snapshot.variables.len(), MAX_CONFIG_VARIABLE_SNAPSHOT);
        assert!(snapshot.truncated);
        assert_eq!(snapshot.variables[0].name, "var-000");
        assert_eq!(snapshot.variables.last().unwrap().name, "var-255");
    }

    #[test]
    fn midi_route_parameters_are_available_to_binding_expressions() {
        let cfg = FloConfig::new();
        let key = crate::event::MIDIKeyInputEvent::with_route(3, 2, 64, 90, true, true);
        assert_eq!(
            cfg.read_event_parameter(&key, key.get_param(0).unwrap())
                .as_i32(),
            3
        );
        assert_eq!(
            cfg.read_event_parameter(&key, key.get_param(5).unwrap())
                .as_i32(),
            1
        );

        let controller = crate::event::MIDIControllerInputEvent::with_route(4, 1, 7, 99, false);
        assert_eq!(
            cfg.read_event_parameter(&controller, controller.get_param(0).unwrap())
                .as_i32(),
            4
        );
        assert_eq!(
            cfg.read_event_parameter(&controller, controller.get_param(4).unwrap())
                .as_i32(),
            0
        );

        let clock = crate::event::MIDIClockInputEvent::with_outport(2);
        assert_eq!(
            cfg.read_event_parameter(&clock, clock.get_param(0).unwrap())
                .as_i32(),
            2
        );
        let transport = crate::event::MIDIStartStopInputEvent::with_outport(4, true);
        assert_eq!(
            cfg.read_event_parameter(&transport, transport.get_param(0).unwrap())
                .as_i32(),
            4
        );
        assert_eq!(
            cfg.read_event_parameter(&transport, transport.get_param(1).unwrap())
                .as_i32(),
            1
        );
    }
}
