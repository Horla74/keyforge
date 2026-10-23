//! Configuration file model: raw TOML structs plus the compiled form the
//! engine actually matches against.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::keycode::{KeyCode, KeyMatcher};

/// Shipped example / `keyforge init` template.
pub const DEFAULT_CONFIG: &str = include_str!("../assets/default_config.toml");

// ---------------------------------------------------------------------------
// raw (serialisable) model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct General {
    /// Profile used when no layer is active.
    #[serde(default = "default_profile_name")]
    pub default_profile: String,
    /// How long `tap_hold` waits before deciding "hold".
    #[serde(default = "default_tap_hold_ms")]
    pub tap_hold_ms: u64,
    /// `exact`: the trigger keys must be exactly the pressed keys.
    /// `subset`: the trigger keys must all be pressed (extra keys allowed).
    #[serde(default)]
    pub combo_mode: ComboMode,
    #[serde(default)]
    pub log_level: LogLevel,
    /// Disable all daemon logging and file maintenance when false.
    #[serde(default = "default_log_enabled")]
    pub log_enabled: bool,
    /// Optional log file (in addition to stderr).
    #[serde(default)]
    pub log_file: Option<String>,
    /// Maximum size of each log file, in MiB.
    #[serde(default = "default_log_max_size_mb")]
    pub log_max_size_mb: u64,
    /// Days to retain rotated log archives.
    #[serde(default = "default_log_retention_days")]
    pub log_retention_days: u32,
    /// How the focused application is detected on Linux.
    #[serde(default)]
    pub app_probe: AppProbe,
}

impl Default for General {
    fn default() -> Self {
        General {
            default_profile: default_profile_name(),
            tap_hold_ms: default_tap_hold_ms(),
            combo_mode: ComboMode::default(),
            log_level: LogLevel::default(),
            log_enabled: true,
            log_file: None,
            log_max_size_mb: default_log_max_size_mb(),
            log_retention_days: default_log_retention_days(),
            app_probe: AppProbe::default(),
        }
    }
}

fn default_profile_name() -> String {
    "default".to_string()
}

fn default_log_max_size_mb() -> u64 {
    10
}

fn default_log_enabled() -> bool { true }
fn default_log_retention_days() -> u32 {
    7
}

#[cfg(test)]
mod log_config_tests {
    use super::*;

    #[test]
    fn logging_defaults_and_limits_are_validated() {
        let cfg = CompiledConfig::from_toml_str("[profiles.default]").unwrap();
        assert!(cfg.general.log_enabled);
        let disabled = CompiledConfig::from_toml_str("[general]\nlog_enabled = false\n[profiles.default]").unwrap();
        assert!(!disabled.general.log_enabled);
        assert_eq!(cfg.general.log_max_size_mb, 10);
        assert_eq!(cfg.general.log_retention_days, 7);
        let cfg = CompiledConfig::from_toml_str("[general]\nlog_max_size_mb = 2\nlog_retention_days = 3\n[profiles.default]").unwrap();
        assert_eq!(cfg.general.log_max_size_mb, 2);
        assert_eq!(cfg.general.log_retention_days, 3);
        for invalid in ["log_max_size_mb = 0", "log_retention_days = 0", "log_max_size_mb = -1", "log_max_size_mb = 9223372036854775807"] {
            assert!(CompiledConfig::from_toml_str(&format!("[general]\n{invalid}\n[profiles.default]")).is_err());
        }
    }
}

fn default_tap_hold_ms() -> u64 {
    200
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ComboMode {
    #[default]
    Exact,
    Subset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

/// Application detection strategy (only meaningful on Linux).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AppProbe {
    /// Use `xdotool` when it is installed, otherwise report "unknown".
    #[default]
    Auto,
    /// Never detect the focused application.
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProfileDef {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub maps: Vec<MapDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct MapDef {
    /// Trigger keys, e.g. `["ctrl", "h"]`.
    #[serde(default)]
    pub keys: Vec<String>,
    /// Convenience form for a single trigger key: `key = "capslock"`.
    #[serde(default)]
    pub key: Option<String>,

    /// Hold-style remap; supports chords such as `"ctrl+shift+t"`.
    #[serde(default)]
    pub remap: Option<String>,
    /// Press-and-release a chord.
    #[serde(default)]
    pub tap: Option<String>,
    /// One chord (`"ctrl+c"`) or a list of chords executed in order
    /// (`["shift+home", "backspace"]`).
    #[serde(default)]
    pub sequence: Option<ChordList>,
    /// Run a shell command when the mapping activates.
    #[serde(default)]
    pub command: Option<String>,
    /// Activate a layer for as long as the trigger is held.
    #[serde(default)]
    pub layer: Option<String>,
    /// Toggle a layer on activation.
    #[serde(default)]
    pub toggle_layer: Option<String>,
    /// Swallow the trigger and emit nothing.
    #[serde(default)]
    pub block: Option<bool>,
    /// Dual role: tap when pressed briefly, hold when held / combined.
    #[serde(default)]
    pub tap_hold: Option<TapHoldDef>,

    /// Only apply while one of these applications is focused.
    #[serde(default)]
    pub only_in: Vec<String>,
    /// Never apply while one of these applications is focused.
    #[serde(default)]
    pub except_in: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// `sequence = "ctrl+c"` or `sequence = ["shift+home", "backspace"]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ChordList {
    One(String),
    Many(Vec<String>),
}

impl ChordList {
    pub fn steps(&self) -> Vec<&str> {
        match self {
            ChordList::One(s) => vec![s.as_str()],
            ChordList::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TapHoldDef {
    pub tap: String,
    pub hold: String,
    /// Overrides `general.tap_hold_ms`.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

// ---------------------------------------------------------------------------
// compiled model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Press the chord while the trigger is held.
    Remap(Vec<KeyCode>),
    /// Press then release the chord.
    Tap(Vec<KeyCode>),
    /// Press and release several chords, in order — e.g. select to the start of
    /// the line and delete it: `["shift+home", "backspace"]`.
    Sequence(Vec<Vec<KeyCode>>),
    Command(String),
    /// Momentary layer (held).
    Layer(String),
    /// Persisting layer (toggled).
    ToggleLayer(String),
    Block,
    TapHold {
        tap: Vec<KeyCode>,
        hold: Vec<KeyCode>,
        timeout_ms: u64,
    },
}

impl Action {
    /// Every key this action can press (used by the config lint).
    pub fn output_keys(&self) -> Vec<KeyCode> {
        match self {
            Action::Remap(keys) | Action::Tap(keys) => keys.clone(),
            Action::Sequence(steps) => steps.iter().flatten().copied().collect(),
            Action::TapHold { tap, hold, .. } => {
                let mut keys = tap.clone();
                keys.extend(hold.iter().copied());
                keys
            }
            Action::Command(_) | Action::Layer(_) | Action::ToggleLayer(_) | Action::Block => {
                Vec::new()
            }
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Action::Remap(ks) => format!("remap {}", chord_label(ks)),
            Action::Tap(ks) => format!("tap {}", chord_label(ks)),
            Action::Sequence(steps) => format!(
                "sequence {}",
                steps
                    .iter()
                    .map(|s| chord_label(s))
                    .collect::<Vec<_>>()
                    .join(" → ")
            ),
            Action::Command(c) => format!("command `{c}`"),
            Action::Layer(l) => format!("layer {l} (hold)"),
            Action::ToggleLayer(l) => format!("layer {l} (toggle)"),
            Action::Block => "block".to_string(),
            Action::TapHold { tap, hold, .. } => {
                format!("tap-hold {} / {}", chord_label(tap), chord_label(hold))
            }
        }
    }
}

pub fn chord_label(ks: &[KeyCode]) -> String {
    ks.iter()
        .map(|k| k.name())
        .collect::<Vec<_>>()
        .join("+")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledMap {
    pub trigger: Vec<KeyMatcher>,
    pub action: Action,
    pub only_in: Vec<String>,
    pub except_in: Vec<String>,
    /// `profiles.<profile>.maps[<n>]`, used in log messages.
    pub origin: String,
}

impl CompiledMap {
    pub fn trigger_label(&self) -> String {
        self.trigger
            .iter()
            .map(|m| m.label())
            .collect::<Vec<_>>()
            .join("+")
    }

    /// Does this mapping use the Windows/Meta key as a trigger?
    ///
    /// Such mappings need the engine to hold Win back: Windows reacts to the key
    /// itself (Start menu on a lone tap, Feedback Hub on `Win+F`).
    pub fn uses_meta(&self) -> bool {
        self.trigger.iter().any(|m| match m {
            KeyMatcher::Mod(crate::keycode::GenericMod::Meta) => true,
            KeyMatcher::Key(k) => matches!(k, KeyCode::LeftMeta | KeyCode::RightMeta),
            _ => false,
        })
    }

    /// `None` means "no information about the focused app": mappings that are
    /// restricted with `only_in` are then skipped, `except_in` ones still run.
    pub fn app_allows(&self, front_app: Option<&str>) -> bool {
        if self.only_in.is_empty() && self.except_in.is_empty() {
            return true;
        }
        let app = match front_app {
            Some(a) => a.to_ascii_lowercase(),
            None => return self.only_in.is_empty(),
        };
        if self.only_in.iter().any(|p| app.contains(p)) {
            return true;
        }
        if !self.only_in.is_empty() {
            return false;
        }
        !self.except_in.iter().any(|p| app.contains(p))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledProfile {
    pub name: String,
    pub description: Option<String>,
    pub maps: Vec<CompiledMap>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledConfig {
    pub general: General,
    pub profiles: Vec<CompiledProfile>,
}

impl CompiledConfig {
    pub fn profile(&self, name: &str) -> Option<&CompiledProfile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    pub fn base_profile(&self) -> &CompiledProfile {
        self.profile(&self.general.default_profile)
            .unwrap_or_else(|| &self.profiles[0])
    }

    pub fn profile_names(&self) -> Vec<&str> {
        self.profiles.iter().map(|p| p.name.as_str()).collect()
    }

    /// Number of active mappings across all profiles.
    pub fn map_count(&self) -> usize {
        self.profiles.iter().map(|p| p.maps.len()).sum()
    }

    /// Does any mapping depend on the focused application?
    pub fn has_app_filters(&self) -> bool {
        self.profiles
            .iter()
            .any(|p| p.maps.iter().any(|m| !m.only_in.is_empty() || !m.except_in.is_empty()))
    }

    /// Does any mapping trigger on the Windows/Meta key?
    pub fn uses_meta_trigger(&self) -> bool {
        self.profiles
            .iter()
            .any(|p| p.maps.iter().any(|m| m.uses_meta()))
    }

    /// Configuration smells that are not errors but usually not what the user
    /// wants. Shown by `keyforge check` and `keyforge doctor`.
    pub fn lint(&self) -> Vec<String> {
        let mut notes = Vec::new();

        let caps_triggered = self.profiles.iter().any(|p| {
            p.maps.iter().any(|m| {
                m.trigger
                    .iter()
                    .any(|t| matches!(t, KeyMatcher::Key(KeyCode::CapsLock)))
            })
        });
        let caps_produced = self.profiles.iter().any(|p| {
            p.maps
                .iter()
                .any(|m| m.action.output_keys().contains(&KeyCode::CapsLock))
        });
        if !caps_triggered {
            notes.push(
                "no mapping uses the physical `capslock` key, so pressing CapsLock on its own still \
                 toggles capitalisation instead of acting as Ctrl/Layer. Two ways to make it useful:\n\
                 \x20     # a) CapsLock = Ctrl (capitalisation then needs another key)\n\
                 \x20     [[profiles.default.maps]]\n\
                 \x20     key = \"capslock\"\n\
                 \x20     remap = \"leftctrl\"\n\
                 \x20     # b) tap CapsLock = capitalisation, hold CapsLock = Ctrl\n\
                 \x20     [[profiles.default.maps]]\n\
                 \x20     key = \"capslock\"\n\
                 \x20     tap_hold = { tap = \"capslock\", hold = \"leftctrl\", timeout_ms = 180 }"
                    .to_string(),
            );
        } else if !caps_produced {
            notes.push(
                "the physical `capslock` key is remapped and no mapping outputs `capslock` any more, \
                 so you cannot toggle capitalisation at all. Keep both by making it dual role:\n\
                 \x20     [[profiles.default.maps]]\n\
                 \x20     key = \"capslock\"\n\
                 \x20     tap_hold = { tap = \"capslock\", hold = \"leftctrl\", timeout_ms = 180 }\n\
                 (tap = CapsLock, hold = Ctrl), or move CapsLock onto a spare key, e.g.\n\
                 \x20     [[profiles.default.maps]]\n\
                 \x20     key = \"rightctrl\"\n\
                 \x20     remap = \"capslock\""
                    .to_string(),
            );
        }

        notes
    }}

// ---------------------------------------------------------------------------
// compilation
// ---------------------------------------------------------------------------

/// Collected problems from a config file.
#[derive(Debug, Default, Clone)]
pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl Report {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn merge(&mut self, other: Report) {
        self.errors.extend(other.errors);
        self.warnings.extend(other.warnings);
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for w in &self.warnings {
            out.push_str(&format!("warning: {w}\n"));
        }
        for e in &self.errors {
            out.push_str(&format!("error: {e}\n"));
        }
        out
    }
}

/// Parse `"ctrl+shift+t"` (or a single key name) into a chord.
pub fn parse_chord(s: &str) -> Result<Vec<KeyCode>, String> {
    let mut out = Vec::new();
    for part in s.split('+') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        out.push(KeyCode::parse(part)?);
    }
    if out.is_empty() {
        return Err(format!("empty key chord `{s}`"));
    }
    Ok(out)
}

impl CompiledConfig {
    pub fn compile(config: &Config) -> Result<CompiledConfig, Report> {
        let mut report = Report::default();
        if config.general.log_max_size_mb == 0
            || config.general.log_max_size_mb > u64::MAX / (1024 * 1024)
        {
            report.errors.push("general.log_max_size_mb must be positive and fit in bytes".into());
        }
        if config.general.log_retention_days == 0 {
            report.errors.push("general.log_retention_days must be positive".into());
        }
        if config.profiles.is_empty() {
            report.errors.push(
                "no profiles defined; add a `[profiles.default]` section".to_string(),
            );
            return Err(report);
        }
        if !config.profiles.contains_key(&config.general.default_profile) {
            report.errors.push(format!(
                "general.default_profile = \"{}\" but that profile does not exist",
                config.general.default_profile
            ));
        }

        let names: Vec<String> = config.profiles.keys().cloned().collect();
        let mut profiles = Vec::new();
        for (pname, pdef) in &config.profiles {
            let mut maps = Vec::new();
            let mut seen: BTreeMap<String, usize> = BTreeMap::new();
            for (i, mdef) in pdef.maps.iter().enumerate() {
                let origin = format!("profiles.{pname}.maps[{i}]");
                match compile_map(mdef, &origin, &names, config.general.tap_hold_ms) {
                    Ok(map) => {
                        let label = map.trigger_label();
                        if let Some(prev) = seen.get(&label) {
                            report.warnings.push(format!(
                                "{origin}: trigger `{label}` already bound at maps[{prev}]; the first entry wins"
                            ));
                        } else {
                            seen.insert(label, i);
                        }
                        maps.push(map);
                    }
                    Err(e) => report.errors.extend(e),
                }
            }
            profiles.push(CompiledProfile {
                name: pname.clone(),
                description: pdef.description.clone(),
                maps,
            });
        }

        if !report.is_ok() {
            return Err(report);
        }
        Ok(CompiledConfig {
            general: config.general.clone(),
            profiles,
        })
    }

    /// Load and compile from a TOML string.
    pub fn from_toml_str(s: &str) -> Result<CompiledConfig, Report> {
        let config = parse_toml(s)?;
        CompiledConfig::compile(&config)
    }
}

/// Parse a config file, turning TOML errors into a [`Report`].
pub fn parse_toml(s: &str) -> Result<Config, Report> {
    match toml::from_str::<Config>(s) {
        Ok(c) => Ok(c),
        Err(e) => {
            let mut r = Report::default();
            r.errors.push(format!("invalid TOML: {e}"));
            Err(r)
        }
    }
}

pub fn load_config_file(path: &Path) -> Result<Config, Report> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            let mut r = Report::default();
            r.errors.push(format!("cannot read {}: {e}", path.display()));
            return Err(r);
        }
    };
    parse_toml(&text)
}

fn compile_map(
    def: &MapDef,
    origin: &str,
    profile_names: &[String],
    default_tap_hold_ms: u64,
) -> Result<CompiledMap, Vec<String>> {
    let mut errors = Vec::new();

    // --- trigger -----------------------------------------------------------
    let mut raw_keys: Vec<String> = def.keys.clone();
    if let Some(k) = &def.key {
        raw_keys.push(k.clone());
    }
    if raw_keys.is_empty() {
        errors.push(format!("{origin}: no trigger keys (`keys` or `key`)"));
    }
    let mut trigger = Vec::new();
    for k in &raw_keys {
        match KeyMatcher::parse(k) {
            Ok(m) => trigger.push(m),
            Err(e) => errors.push(format!("{origin}: {e}")),
        }
    }

    // --- action ------------------------------------------------------------
    let mut actions: Vec<Action> = Vec::new();
    let tap_field = def.tap.as_ref();
    if let Some(spec) = &def.remap {
        match parse_chord(spec) {
            Ok(ks) => actions.push(Action::Remap(ks)),
            Err(e) => errors.push(format!("{origin}.remap: {e}")),
        }
    }
    if let Some(spec) = tap_field {
        match parse_chord(spec) {
            Ok(ks) => actions.push(Action::Tap(ks)),
            Err(e) => errors.push(format!("{origin}.tap: {e}")),
        }
    }
    if let Some(list) = &def.sequence {
        let raw_steps = list.steps();
        if raw_steps.is_empty() {
            errors.push(format!("{origin}.sequence: no steps"));
        }
        let mut steps: Vec<Vec<KeyCode>> = Vec::new();
        for (i, spec) in raw_steps.iter().enumerate() {
            match parse_chord(spec) {
                Ok(ks) => steps.push(ks),
                Err(e) => errors.push(format!("{origin}.sequence[{i}]: {e}")),
            }
        }
        if !steps.is_empty() && steps.len() == 1 {
            // A one step sequence is just a tap.
            actions.push(Action::Tap(steps.remove(0)));
        } else if !steps.is_empty() {
            actions.push(Action::Sequence(steps));
        }
    }
    if let Some(cmd) = &def.command {
        if cmd.trim().is_empty() {
            errors.push(format!("{origin}.command: empty command"));
        } else {
            actions.push(Action::Command(cmd.clone()));
        }
    }
    if let Some(layer) = &def.layer {
        check_layer(layer, origin, "layer", profile_names, &mut errors);
        actions.push(Action::Layer(layer.clone()));
    }
    if let Some(layer) = &def.toggle_layer {
        check_layer(layer, origin, "toggle_layer", profile_names, &mut errors);
        actions.push(Action::ToggleLayer(layer.clone()));
    }
    if def.block == Some(true) {
        actions.push(Action::Block);
    }
    if let Some(th) = &def.tap_hold {
        let tap = parse_chord(&th.tap)
            .map_err(|e| errors.push(format!("{origin}.tap_hold.tap: {e}")))
            .unwrap_or_default();
        let hold = parse_chord(&th.hold)
            .map_err(|e| errors.push(format!("{origin}.tap_hold.hold: {e}")))
            .unwrap_or_default();
        if trigger.len() != 1 {
            errors.push(format!(
                "{origin}.tap_hold: dual-role mappings need exactly one trigger key, found {}",
                trigger.len()
            ));
        }
        actions.push(Action::TapHold {
            tap,
            hold,
            timeout_ms: th.timeout_ms.unwrap_or(default_tap_hold_ms),
        });
    }

    if actions.is_empty() {
        errors.push(format!(
            "{origin}: no action; use one of remap / tap / sequence (one chord or a list of chords) / \
             command / layer / toggle_layer / block / tap_hold"
        ));
    } else if actions.len() > 1 {
        errors.push(format!(
            "{origin}: {} actions set ({}); use exactly one",
            actions.len(),
            actions.iter().map(|a| a.describe()).collect::<Vec<_>>().join(", ")
        ));
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    Ok(CompiledMap {
        trigger,
        action: actions.remove(0),
        only_in: def.only_in.iter().map(|s| s.to_ascii_lowercase()).collect(),
        except_in: def.except_in.iter().map(|s| s.to_ascii_lowercase()).collect(),
        origin: origin.to_string(),
    })
}

fn check_layer(
    layer: &str,
    origin: &str,
    field: &str,
    profile_names: &[String],
    errors: &mut Vec<String>,
) {
    if !profile_names.iter().any(|n| n == layer) {
        errors.push(format!(
            "{origin}.{field}: unknown profile `{layer}` (known: {})",
            profile_names.join(", ")
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[general]
default_profile = "default"
tap_hold_ms = 180

[profiles.default]
description = "base"

[[profiles.default.maps]]
key = "capslock"
tap_hold = { tap = "escape", hold = "leftctrl" }

[[profiles.default.maps]]
keys = ["ctrl", "h"]
remap = "left"

[[profiles.default.maps]]
keys = ["leftctrl", "leftalt", "delete"]
command = "echo hi"

[[profiles.default.maps]]
key = "f1"
layer = "nav"

[profiles.nav]
[[profiles.nav.maps]]
key = "h"
remap = "left"
"#;

    #[test]
    fn compiles_sample() {
        let cfg = parse_toml(SAMPLE).expect("parse");
        let compiled = CompiledConfig::compile(&cfg).expect("compile");
        assert_eq!(compiled.map_count(), 5);
        assert_eq!(compiled.base_profile().name, "default");
        let td = &compiled.profile("default").unwrap().maps[0];
        match &td.action {
            Action::TapHold { tap, hold, timeout_ms } => {
                assert_eq!(tap, &[KeyCode::Escape]);
                assert_eq!(hold, &[KeyCode::LeftCtrl]);
                assert_eq!(*timeout_ms, 180);
            }
            other => panic!("unexpected action {other:?}"),
        }
    }

    #[test]
    fn reports_multiple_errors() {
        let bad = r#"
[profiles.default]
[[profiles.default.maps]]
keys = ["nope"]
remap = "esc"
[[profiles.default.maps]]
key = "a"
remap = "b"
tap = "c"
[[profiles.default.maps]]
key = "b"
layer = "ghost"
"#;
        let cfg = parse_toml(bad).unwrap();
        let err = CompiledConfig::compile(&cfg).unwrap_err();
        assert_eq!(err.errors.len(), 3, "{:?}", err.errors);
    }

    #[test]
    fn app_filters() {
        let cfg = parse_toml(SAMPLE).unwrap();
        let compiled = CompiledConfig::compile(&cfg).unwrap();
        let m = &compiled.base_profile().maps[1];
        assert!(m.app_allows(None));
        let mut m2 = m.clone();
        m2.only_in = vec!["firefox".into()];
        assert!(m2.app_allows(Some("Firefox.exe")));
        assert!(!m2.app_allows(Some("code.exe")));
        assert!(!m2.app_allows(None));
        let mut m3 = m.clone();
        m3.except_in = vec!["game.exe".into()];
        assert!(m3.app_allows(None));
        assert!(!m3.app_allows(Some("GAME.EXE")));
    }

    #[test]
    fn shipped_default_config_is_valid() {
        let cfg = parse_toml(DEFAULT_CONFIG).expect("shipped config parses");
        let compiled = CompiledConfig::compile(&cfg).expect("shipped config compiles");
        assert!(compiled.map_count() >= 5);
        // The shipped example maps capslock, so that note must stay quiet.
        let notes = compiled.lint();
        assert!(
            !notes.iter().any(|n| n.contains("no mapping uses the physical")),
            "{notes:?}"
        );
    }

    #[test]
    fn lint_flags_a_capslock_that_can_no_longer_toggle() {
        let cfg = parse_toml(
            r#"
[profiles.default]
[[profiles.default.maps]]
key = "capslock"
remap = "leftctrl"

[[profiles.default.maps]]
keys = ["ctrl", "h"]
remap = "backspace"
"#,
        )
        .unwrap();
        let compiled = CompiledConfig::compile(&cfg).unwrap();
        let notes = compiled.lint();
        assert!(
            notes.iter().any(|n| n.contains("cannot toggle capitalisation")),
            "{notes:?}"
        );

        // The dual role variant keeps capitalisation, so it must be quiet.
        let cfg = parse_toml(
            r#"
[profiles.default]
[[profiles.default.maps]]
key = "capslock"
tap_hold = { tap = "capslock", hold = "leftctrl", timeout_ms = 180 }
"#,
        )
        .unwrap();
        let compiled = CompiledConfig::compile(&cfg).unwrap();
        assert!(compiled.lint().is_empty(), "{:?}", compiled.lint());
    }

    #[test]
    fn lint_flags_an_unmapped_capslock() {
        let cfg = parse_toml(
            r#"
[profiles.default]
[[profiles.default.maps]]
keys = ["ctrl", "h"]
remap = "backspace"
"#,
        )
        .unwrap();
        let compiled = CompiledConfig::compile(&cfg).unwrap();
        let notes = compiled.lint();
        assert!(
            notes.iter().any(|n| n.contains("capslock")),
            "capslock must be reported: {notes:?}"
        );
    }
}
