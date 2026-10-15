//! keyforge-core — platform independent remapping engine.
//!
//! * [`keycode`] — canonical key names and per-platform code tables.
//! * [`config`] — TOML configuration model and compiler.
//! * [`engine`] — the matching state machine (combos, layers, dual-role keys).
//! * [`logging`] / [`paths`] — small helpers shared by the binaries.

pub mod config;
pub mod engine;
pub mod keycode;
pub mod logging;
pub mod paths;

pub use config::{
    parse_chord, Action, CompiledConfig, CompiledMap, CompiledProfile, Config, General, LogLevel,
    MapDef, ProfileDef, Report, DEFAULT_CONFIG,
};
pub use engine::{Engine, KeyState, Outcome, Output, Stats};
pub use keycode::{GenericMod, KeyCode, KeyMatcher};

/// Version of the tool, taken from Cargo.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
