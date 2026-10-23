//! Minimal dependency-free logger (stderr + optional file).
//!
//! A logging crate would be another dependency for very little gain here: the
//! daemon only needs level filtering and a timestamp.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::LogLevel;

mod file;
use file::RollingFile;

struct Sink {
    level: LogLevel,
    enabled: AtomicBool,
    path: Option<PathBuf>,
    file: Mutex<Option<RollingFile>>,
    stderr: bool,
}

static SINK: OnceLock<Sink> = OnceLock::new();

/// Install the global logger. Safe to call more than once; the first call wins.
pub fn init(level: LogLevel, file: Option<&Path>, stderr: bool) -> std::io::Result<()> {
    init_with_rotation(level, file, stderr, 10, 7)
}

pub fn init_with_rotation(
    level: LogLevel,
    file: Option<&Path>,
    stderr: bool,
    max_size_mb: u64,
    retention_days: u32,
) -> std::io::Result<()> {
    init_configured(level, file, stderr, max_size_mb, retention_days, true)
}

pub fn init_configured(
    level: LogLevel,
    file: Option<&Path>,
    stderr: bool,
    max_size_mb: u64,
    retention_days: u32,
    enabled: bool,
) -> std::io::Result<()> {
    let max_bytes = max_size_mb
        .checked_mul(1024 * 1024)
        .filter(|n| *n > 0)
        .filter(|_| retention_days > 0)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "log size and retention days must be positive and within range",
            )
        })?;
    let opened = match file.filter(|_| enabled) {
        Some(path) => Some(RollingFile::open(path, max_bytes, retention_days)?),
        None => None,
    };
    let _ = SINK.set(Sink {
        level,
        enabled: AtomicBool::new(enabled),
        path: file.map(Path::to_path_buf),
        file: Mutex::new(opened),
        stderr,
    });
    Ok(())
}

/// Apply logging and rotation settings from a validated config reload.
pub fn reconfigure(enabled: bool, max_size_mb: u64, retention_days: u32) -> std::io::Result<()> {
    let max_bytes = max_size_mb
        .checked_mul(1024 * 1024)
        .filter(|n| *n > 0)
        .filter(|_| retention_days > 0)
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid log rotation limits",
            )
        })?;
    if let Some(sink) = SINK.get() {
        let mut file = sink.file.lock().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::Other, "log file lock poisoned")
        })?;
        if enabled {
            if let Some(file) = file.as_mut() {
                file.set_limits(max_bytes, retention_days);
            } else if let Some(path) = &sink.path {
                *file = Some(RollingFile::open(path, max_bytes, retention_days)?);
            }
        } else {
            file.take(); // close the file and stop both writes and archive cleanup
        }
        sink.enabled.store(enabled, Ordering::Relaxed);
    }
    Ok(())
}

/// Run retention independently of log traffic and the configured log level.
pub fn maintenance() -> std::io::Result<()> {
    if let Some(sink) = SINK.get().filter(|s| s.enabled.load(Ordering::Relaxed)) {
        let mut file = sink.file.lock().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::Other, "log file lock poisoned")
        })?;
        if let Some(file) = file.as_mut() {
            file.maintain(file::now())?;
        }
    }
    Ok(())
}

pub fn level_enabled(level: LogLevel) -> bool {
    match SINK.get() {
        Some(s) => s.enabled.load(Ordering::Relaxed) && level <= s.level,
        None => level <= LogLevel::Info,
    }
}

pub fn log(level: LogLevel, args: fmt::Arguments<'_>) {
    if !level_enabled(level) {
        return;
    }
    let tag = match level {
        LogLevel::Error => "ERROR",
        LogLevel::Warn => "WARN ",
        LogLevel::Info => "INFO ",
        LogLevel::Debug => "DEBUG",
        LogLevel::Trace => "TRACE",
    };
    let line = format!("[{}] {} {}", timestamp(), tag, args);
    match SINK.get() {
        Some(sink) => {
            if sink.stderr {
                let _ = writeln!(std::io::stderr(), "{line}");
            }
            if let Ok(mut file) = sink.file.lock() {
                if let Some(f) = file.as_mut() {
                    if let Err(error) = f.write(&line, file::now()) {
                        let _ = writeln!(std::io::stderr(), "log file error: {error}\n{line}");
                    }
                }
            }
        }
        None => {
            let _ = writeln!(std::io::stderr(), "{line}");
        }
    }
}

fn timestamp() -> String {
    // Seconds since the epoch is enough for a daemon log and needs no crates.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {{
        if $crate::logging::level_enabled($crate::config::LogLevel::Error) {
            $crate::logging::log($crate::config::LogLevel::Error, format_args!($($arg)*));
        }
    }};
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {{
        if $crate::logging::level_enabled($crate::config::LogLevel::Warn) {
            $crate::logging::log($crate::config::LogLevel::Warn, format_args!($($arg)*));
        }
    }};
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {{
        if $crate::logging::level_enabled($crate::config::LogLevel::Info) {
            $crate::logging::log($crate::config::LogLevel::Info, format_args!($($arg)*));
        }
    }};
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {{
        if $crate::logging::level_enabled($crate::config::LogLevel::Debug) {
            $crate::logging::log($crate::config::LogLevel::Debug, format_args!($($arg)*));
        }
    }};
}

#[macro_export]
macro_rules! log_trace {
    ($($arg:tt)*) => {{
        if $crate::logging::level_enabled($crate::config::LogLevel::Trace) {
            $crate::logging::log($crate::config::LogLevel::Trace, format_args!($($arg)*));
        }
    }};
}
