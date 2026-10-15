//! Minimal dependency-free logger (stderr + optional file).
//!
//! A logging crate would be another dependency for very little gain here: the
//! daemon only needs level filtering and a timestamp.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::LogLevel;

struct Sink {
    level: LogLevel,
    file: Option<Mutex<File>>,
    stderr: bool,
}

static SINK: OnceLock<Sink> = OnceLock::new();

/// Install the global logger. Safe to call more than once; the first call wins.
pub fn init(level: LogLevel, file: Option<&Path>, stderr: bool) -> std::io::Result<()> {
    let opened = match file {
        Some(path) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            Some(Mutex::new(
                OpenOptions::new().create(true).append(true).open(path)?,
            ))
        }
        None => None,
    };
    let _ = SINK.set(Sink {
        level,
        file: opened,
        stderr,
    });
    Ok(())
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
                    let _ = writeln!(f, "{line}");
                    let _ = f.flush();
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
