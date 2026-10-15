//! Where keyforge keeps its configuration on each platform.

use std::path::{Path, PathBuf};

/// `keyforge`'s per-user directory.
///
/// * Windows: `%APPDATA%\keyforge`
/// * macOS: `~/Library/Application Support/keyforge`
/// * Linux: `$XDG_CONFIG_HOME/keyforge` (default `~/.config/keyforge`)
pub fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("KEYFORGE_CONFIG_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("keyforge");
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = home_dir() {
            return home.join("Library/Application Support/keyforge");
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg).join("keyforge");
            }
        }
        if let Some(home) = home_dir() {
            return home.join(".config/keyforge");
        }
    }
    PathBuf::from(".").join("keyforge")
}

pub fn default_config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn default_log_path() -> PathBuf {
    config_dir().join("keyforge.log")
}

pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var("USERPROFILE").ok().map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
}

/// Expand a leading `~` in a user supplied path.
pub fn expand_user(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(path)
}

/// Resolve the config file to use: explicit flag, then `$KEYFORGE_CONFIG`,
/// then the per-user default.
pub fn resolve_config_path(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return p.to_path_buf();
    }
    if let Ok(env) = std::env::var("KEYFORGE_CONFIG") {
        if !env.is_empty() {
            return PathBuf::from(env);
        }
    }
    default_config_path()
}
