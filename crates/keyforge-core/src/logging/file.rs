//! Size/daily rotation with narrowly scoped archive retention.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DAY: u64 = 86400;

pub(super) struct RollingFile {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    started: u64,
    last_cleanup: u64,
    max_bytes: u64,
    keep_days: u32,
}

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl RollingFile {
    pub(super) fn open(path: &Path, max_bytes: u64, keep_days: u32) -> io::Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let metadata = file.metadata()?;
        let started = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|t| t.as_secs())
            .unwrap_or_else(now);
        let mut log = Self {
            path: path.to_owned(),
            file: Some(file),
            size: metadata.len(),
            started,
            last_cleanup: 0,
            max_bytes,
            keep_days,
        };
        log.cleanup(now())?;
        Ok(log)
    }

    pub(super) fn set_limits(&mut self, max_bytes: u64, keep_days: u32) {
        self.max_bytes = max_bytes;
        self.keep_days = keep_days;
        self.last_cleanup = 0;
    }

    fn prefix(&self) -> String {
        format!(
            "{}.archive-",
            self.path.file_name().unwrap().to_string_lossy()
        )
    }

    fn parent(&self) -> &Path {
        self.path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
    }

    fn cleanup(&mut self, now: u64) -> io::Result<()> {
        let prefix = self.prefix();
        for entry in fs::read_dir(self.parent())? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(stamp) = name
                .strip_prefix(&prefix)
                .and_then(|s| s.strip_suffix(".log"))
            else {
                continue;
            };
            let Some((seconds, serial)) = stamp.split_once('-') else {
                continue;
            };
            // Only files bearing our exact archive naming convention are eligible.
            if serial.parse::<u32>().is_err() {
                continue;
            }
            if let Ok(seconds) = seconds.parse::<u64>() {
                if now.saturating_sub(seconds) >= u64::from(self.keep_days) * DAY {
                    fs::remove_file(entry.path())?;
                }
            }
        }
        self.last_cleanup = now;
        Ok(())
    }

    fn rotate(&mut self, now: u64) -> io::Result<()> {
        // Close before renaming: Windows does not allow renaming an open log.
        self.file.take();
        let mut serial = 0u32;
        let archive = loop {
            let candidate = self
                .parent()
                .join(format!("{}{now}-{serial}.log", self.prefix()));
            if !candidate.exists() {
                break candidate;
            }
            serial = serial.checked_add(1).ok_or_else(|| {
                io::Error::new(io::ErrorKind::AlreadyExists, "too many log archives")
            })?;
        };
        let renamed = fs::rename(&self.path, archive);
        // Reopen even when rename failed, so later messages can still be saved.
        self.file = Some(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?,
        );
        renamed?;
        self.size = 0;
        self.started = now;
        Ok(())
    }

    pub(super) fn write(&mut self, line: &str, now: u64) -> io::Result<()> {
        if now.saturating_sub(self.last_cleanup) >= 3600 {
            if let Err(error) = self.cleanup(now) {
                let _ = writeln!(io::stderr(), "cannot clean up log archives: {error}");
                self.last_cleanup = now;
            }
        }
        let bytes = line.len() as u64 + 1;
        if self.size != 0
            && (self.size.saturating_add(bytes) > self.max_bytes
                || now.saturating_sub(self.started) >= DAY)
        {
            if let Err(error) = self.rotate(now) {
                let _ = writeln!(io::stderr(), "cannot rotate log file: {error}");
            }
        }
        if self.file.is_none() {
            self.file = Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            );
        }
        let file = self.file.as_mut().unwrap();
        writeln!(file, "{line}")?;
        file.flush()?;
        self.size = self.size.saturating_add(bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_without_losing_lines_and_retains_only_own_recent_archives() {
        let dir = std::env::temp_dir().join(format!("keyforge-log-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.log");
        let time = now();
        let mut log = RollingFile::open(&path, 6, 7).unwrap();
        log.write("hello", time).unwrap();
        log.write("world", time).unwrap();
        log.write("again", time).unwrap();
        let archive = dir.join(format!("test.log.archive-{time}-0.log"));
        assert_eq!(fs::read_to_string(&archive).unwrap(), "hello\n");
        assert_eq!(
            fs::read_to_string(dir.join(format!("test.log.archive-{time}-1.log"))).unwrap(),
            "world\n"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "again\n");
        let unrelated = dir.join("test.log.archive-not-a-timestamp.log");
        fs::write(&unrelated, "keep me").unwrap();
        log.set_limits(100, 1);
        log.write("new day", time + DAY).unwrap();
        assert!(!archive.exists());
        assert!(unrelated.exists());
        assert_eq!(fs::read_to_string(&path).unwrap(), "new day\n");
        drop(log);
        // Test directory contains only files created in this test.
        for entry in fs::read_dir(&dir).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        fs::remove_dir(dir).unwrap();
    }
}
