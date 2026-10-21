//! Platform input backends.
//!
//! Status (see README):
//! * **Windows** — implemented: `WH_KEYBOARD_LL` hook + `SendInput`.
//! * Linux — an in-progress evdev/uinput backend lives in `linux.rs`; it is not
//!   part of the module tree yet, so every entry point below returns a clear
//!   error on non-Windows platforms.
//! * macOS — not started yet (`CGEventTap` is the planned mechanism).
//!
//! The shared [`Host`] contract is platform independent: physical key
//! transitions go in, "swallow it" plus the events to inject come out.

use std::fmt;

use keyforge_core::engine::{KeyState, Outcome, Output};
use keyforge_core::KeyCode;

#[cfg(windows)]
mod windows;
// Kept for the next milestone; not part of the module tree yet so that the
// crate builds (and is testable) without the Linux dependencies.
// #[cfg(target_os = "linux")] mod linux;

/// Something the daemon has to provide to a backend.
pub trait Host {
    /// A physical key changed state. Return whether to swallow the event and
    /// which keys to inject.
    fn on_key(&mut self, key: KeyCode, state: KeyState) -> Outcome;
    /// Periodic timer work (tap-hold deadlines, config reload, app polling).
    fn on_tick(&mut self) -> Vec<Output>;
    /// Manually reload configuration, even when file watching is disabled.
    fn reload_config(&mut self) -> Vec<Output> {
        Vec::new()
    }
    /// Suspend/resume mappings, releasing owned keys before suspension.
    fn toggle_pause(&mut self) -> Vec<Output> {
        Vec::new()
    }
    fn is_paused(&self) -> bool {
        false
    }
    fn config_path(&self) -> Option<&std::path::Path> {
        None
    }
    /// Execute a `command` mapping.
    fn run_command(&mut self, command: &str);
    /// Polled every loop iteration; returning `true` shuts the backend down.
    fn should_stop(&mut self) -> bool {
        false
    }
    /// Called once when the backend is about to stop. Whatever is returned is
    /// delivered before the hook is removed, so held keys can be released.
    fn on_shutdown(&mut self) -> Vec<Output> {
        Vec::new()
    }
    /// How long the backend may block before calling [`Host::on_tick`].
    fn next_timeout_ms(&self) -> u32 {
        250
    }
    /// Non fatal problem report (permissions, unsupported output key, ...).
    fn warn(&mut self, message: &str) {
        let _ = message;
    }
}

#[derive(Debug, Clone)]
pub struct Error {
    pub message: String,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Error {
        Error {
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// Options for the platform backend.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Linux only (future): only grab devices whose name contains one of these.
    pub device_filter: Vec<String>,
    /// Whether the focused application may be probed (`only_in` / `except_in`).
    pub app_probe: bool,
    /// Linux only (future): how often to look for hot-plugged keyboards.
    pub rescan_interval: std::time::Duration,
}

impl Options {
    pub fn new() -> Options {
        Options {
            device_filter: Vec::new(),
            app_probe: true,
            rescan_interval: std::time::Duration::from_secs(3),
        }
    }
}

/// Run the backend until the host asks to stop or the platform fails.
pub fn run(host: Box<dyn Host>, options: Options) -> Result<()> {
    #[cfg(windows)]
    {
        let _ = options;
        windows::run(host)
    }
    #[cfg(not(windows))]
    {
        let _ = (host, options);
        Err(Error::new(format!(
            "the {} backend is not implemented in this build yet; \
             only Windows is supported at the moment",
            platform_name()
        )))
    }
}

/// Ask a running backend to return from [`run`] (Ctrl+C equivalent).
pub fn request_stop() {
    #[cfg(windows)]
    windows::request_stop();
}

/// Ask *another* keyforge instance to stop gracefully.
///
/// Returns `Ok(false)` when no instance is listening.
pub fn signal_stop() -> Result<bool> {
    #[cfg(windows)]
    {
        windows::signal_stop().map_err(Error::new)
    }
    #[cfg(not(windows))]
    {
        Err(Error::new("`stop` is only implemented on Windows for now"))
    }
}

/// Is a keyforge daemon currently listening for the stop signal?
pub fn instance_running() -> bool {
    #[cfg(windows)]
    {
        windows::instance_running()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Is a working backend compiled into this binary?
pub fn backend_available() -> bool {
    cfg!(windows)
}

/// Does the backend drop every event it does not swallow, so that the engine
/// has to re-inject the pass-through keys itself?
///
/// * Windows (`WH_KEYBOARD_LL`) and macOS (`CGEventTap`): **no** — returning
///   from the hook without swallowing lets the OS deliver the real event.
/// * Linux (evdev grab + uinput): **yes** — the physical device is grabbed, so
///   nothing reaches the system unless we emit it on the virtual device.
pub const REINJECTS_PASSTHROUGH: bool = cfg!(target_os = "linux");

/// Warnings that must be shown when the daemon starts (empty means all good).
///
/// The important one on Windows: a *low integrity* process can install the hook
/// but UIPI only delivers input that targets processes at the same or a lower
/// integrity level. The practical symptom is "remapping only works inside the
/// terminal I started keyforge from".
pub fn startup_warnings() -> Vec<String> {
    #[cfg(windows)]
    {
        let mut warnings = Vec::new();
        if let Some(rid) = windows::integrity_rid() {
            if rid < 0x2000 {
                warnings.push(format!(
                    "keyforge runs at {} integrity (0x{rid:x}). Windows only delivers input of processes \
                     at the same or a lower integrity level to such a process, so remapping will only \
                     affect that narrow set of windows (typically just the terminal that started it).",
                    windows::integrity_name(rid)
                ));
                warnings.push(
                    "How to fix: a process' integrity level is the lower of its parent token and the \
                     executable file's mandatory label, so when the binaries come from a sandbox/agent \
                     workspace a double click does not help. Either\n  \
                     (a) move the binaries to a normal folder, e.g. %LOCALAPPDATA%\\Programs\\keyforge, \
                     and start them from there (`icacls \"<exe>\" /setintegritylevel medium` also works), or\n  \
                     (b) build them yourself in a normal shell (cargo build --release), or\n  \
                     (c) start keyforge from a normal cmd/PowerShell (Win+R -> `wt`).\n\
                     Verify with `keyforge doctor`: process integrity level must read Medium (0x2000) \
                     and `hook round trip: OK`."
                        .to_string(),
                );
            }
        }
        warnings
    }
    #[cfg(not(windows))]
    {
        vec![format!(
            "the {} backend is not implemented in this build (Windows only for now)",
            platform_name()
        )]
    }
}

/// Is the current process below medium integrity?
///
/// Such a process must not expect to see other windows' keystrokes, so the CLI
/// prints a concrete fix instead of just a warning.
pub fn integrity_below_medium() -> bool {
    #[cfg(windows)]
    {
        windows::integrity_rid().map(|rid| rid < 0x2000).unwrap_or(false)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Concrete, copy-pasteable commands that move the binaries out of a
/// low-integrity tree (the usual cause: they were built inside a sandbox or
/// agent workspace whose folder carries an inheritable low mandatory label).
pub fn relocation_commands() -> Vec<String> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("keyforge.exe"));
    let daemon = exe.with_file_name("keyforged.exe");
    let dest = "%LOCALAPPDATA%\\Programs\\keyforge";
    vec![
        format!("mkdir \"$env:LOCALAPPDATA\\Programs\\keyforge\""),
        format!("copy \"{}\" \"{dest}\\\"", exe.display()),
        format!("copy \"{}\" \"{dest}\\\"", daemon.display()),
        format!("& \"$env:LOCALAPPDATA\\Programs\\keyforge\\keyforge.exe\" doctor"),
    ]
}

pub fn platform_name() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

/// What the user has to do before the daemon can see keystrokes.
pub fn permission_hint() -> Option<&'static str> {
    #[cfg(windows)]
    {
        None
    }
    #[cfg(not(windows))]
    {
        Some("only the Windows backend is implemented in this build")
    }
}

/// Identifier of the focused application, or `None` when it cannot be
/// determined. Used by the `only_in` / `except_in` filters.
pub fn frontmost_app(options: &Options) -> Option<String> {
    if !options.app_probe {
        return None;
    }
    #[cfg(windows)]
    {
        windows::frontmost_app()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Human readable list of the input devices that would be remapped.
pub fn list_devices(options: &Options) -> Result<Vec<String>> {
    let _ = options;
    #[cfg(windows)]
    {
        Err(Error::new(
            "on Windows keyforge hooks the whole session, so there is nothing to list \
             (all keyboards are covered)",
        ))
    }
    #[cfg(not(windows))]
    {
        Err(Error::new("device listing is only implemented on Linux"))
    }
}

/// Platform specific health report for `keyforge doctor`.
pub fn diagnostics() -> Vec<String> {
    #[cfg(windows)]
    {
        let mut lines = Vec::new();
        let integrity = windows::integrity_level();
        lines.push(format!("process integrity level: {integrity}"));
        if integrity.starts_with("Low") || integrity.starts_with("Untrusted") {
            lines.push(
                "  note: low integrity processes are subject to UIPI — injected input can be \
                 dropped and the hook may not see input produced by higher integrity processes. \
                 Run keyforge in a normal (medium integrity) session."
                    .to_string(),
            );
        }

        match windows::probe_install_hook() {
            Ok(()) => lines.push("low level keyboard hook: installable".to_string()),
            Err(e) => lines.push(format!("low level keyboard hook: NOT available ({e})")),
        }
        match windows::probe_hook_roundtrip() {
            Ok(true) => lines.push(
                "hook round trip: OK (an injected F13 was observed by the hook)".to_string(),
            ),
            Ok(false) => lines.push(
                "hook round trip: no event observed — normally caused by UIPI (low integrity \
                 session) or by another process blocking input"
                    .to_string(),
            ),
            Err(e) => lines.push(format!("hook round trip: failed ({e})")),
        }
        match windows::probe_send_input() {
            Ok(true) => lines.push("SendInput: OK (can inject key events)".to_string()),
            Ok(false) => lines.push("SendInput: no event accepted".to_string()),
            Err(e) => lines.push(format!("SendInput: failed ({e})")),
        }
        // The decisive check: what is in front of us, and can it even talk to us?
        let app = frontmost_app(&Options::new());
        let app_rid = windows::frontmost_integrity_rid();
        let ours = windows::integrity_rid();
        match app {
            Some(name) => match app_rid {
                Some(rid) => {
                    lines.push(format!(
                        "foreground application: {name} ({} integrity)",
                        windows::integrity_name(rid)
                    ));
                    if let Some(ours) = ours {
                        if rid > ours {
                            lines.push(format!(
                                "MISMATCH: that window runs at {} integrity while keyforge runs at {}; \
                                 Windows never delivers a higher integrity window's keystrokes to a lower \
                                 integrity hook, so mappings cannot apply there. This is the \
                                 \"works in some windows only\" symptom.",
                                windows::integrity_name(rid),
                                windows::integrity_name(ours)
                            ));
                        }
                    }
                }
                None => {
                    lines.push(format!("foreground application: {name}"));
                    // Reading the token of a higher integrity process is denied,
                    // so "access denied" IS the answer.
                    match ours {
                        Some(ours) if ours < 0x2000 => lines.push(
                            "MISMATCH: its integrity level cannot be read, which means that process is \
                             protected from keyforge (higher integrity) — its keystrokes will not reach \
                             the hook. This is the \"works in some windows only\" symptom."
                                .to_string(),
                        ),
                        _ => lines.push(
                            "note: could not read the integrity level of the foreground process \
                             (access denied)"
                                .to_string(),
                        ),
                    }
                }
            },
            None => lines.push("foreground application: unknown".to_string()),
        }
        lines
    }
    #[cfg(not(windows))]
    {
        vec![format!(
            "{} backend is not implemented in this build; only Windows is supported for now",
            platform_name()
        )]
    }
}
