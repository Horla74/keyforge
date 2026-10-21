//! Minimal dependency-free logger (stderr + optional file).
//!
//! A logging crate would be another dependency for very little gain here: the
//! daemon only needs level filtering and a timestamp.

use std::fmt;
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::LogLevel;

mod file;
use file::RollingFile;

struct Sink {
    level: LogLevel,
    file: Option<Mutex<RollingFile>>,
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
    let opened = match file {
        Some(path) => Some(Mutex::new(RollingFile::open(
            path,
            max_bytes,
            retention_days,
        )?)),
        None => None,
    };
    let _ = SINK.set(Sink {
        level,
        file: opened,
        stderr,
    });
    Ok(())
}

/// Apply rotation settings from a successfully validated config reload.
pub fn set_rotation(max_size_mb: u64, retention_days: u32) {
    if let Some(max_bytes) = max_size_mb.checked_mul(1024 * 1024).filter(|n| *n > 0) {
        if retention_days == 0 {
            return;
        }
        if let Some(file) = SINK.get().and_then(|s| s.file.as_ref()) {
            if let Ok(mut file) = file.lock() {
                file.set_limits(max_bytes, retention_days);
            }
        }
    }
}

pub fn level_enabled(level: LogLevel) -> bool {
    match SINK.get() {
        Some(s) => level <= s.level,
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
            if let Some(file) = &sink.file {
                if let Ok(mut f) = file.lock() {
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
    ($($arg:tt)*) => { $crate::logging::log($crate::config::LogLevel::Error, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => { $crate::logging::log($crate::config::LogLevel::Warn, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::logging::log($crate::config::LogLevel::Info, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => { $crate::logging::log($crate::config::LogLevel::Debug, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! log_trace {
    ($($arg:tt)*) => { $crate::logging::log($crate::config::LogLevel::Trace, format_args!($($arg)*)) };
}
