//! Linux backend: grab keyboards through evdev, emit through a uinput device.
//!
//! Grabbing (EVIOCGRAB) is what makes remapping lossless: the physical events
//! never reach the rest of the system, so the virtual device is the only
//! source of input. Anything the engine does not swallow is re-emitted.

use std::collections::{HashMap, HashSet};
use std::io;

mod control;
pub use control::{force_stop, instance_running, signal_stop};
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use evdev::{AttributeSet, Device, EventType, InputEvent, KeyCode as LinuxKey};
use keyforge_core::{KeyCode, KeyState, Output};

use crate::{Error, Host, Options, Result};

/// Name of the virtual device; also used to avoid grabbing ourselves.
const VIRTUAL_NAME: &str = "keyforge virtual keyboard";

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_signal(_sig: i32) {
    STOP.store(true, Ordering::SeqCst);
}

/// Ask the running backend to shut down.
pub fn request_stop() {
    STOP.store(true, Ordering::SeqCst);
}

struct Grabbed {
    path: PathBuf,
    name: String,
    device: Device,
    held: HashSet<u16>,
}

pub fn run(mut host: Box<dyn Host>, options: Options) -> Result<()> {
    STOP.store(false, Ordering::SeqCst);
    let _signals = SignalHandlers::install();
    let control = control::Control::bind()
        .map_err(|e| Error::new(format!("cannot start Linux control socket: {e}")))?;

    let mut virtual_device = build_virtual_device().map_err(|e| {
        Error::new(format!(
            "cannot create the uinput virtual keyboard: {e}\n{}",
            crate::permission_hint().unwrap_or("")
        ))
    })?;

    let mut devices: Vec<Grabbed> = Vec::new();
    rescan(&mut devices, &options, host.as_mut());
    if devices.is_empty() {
        return Err(Error::new(format!(
            "no usable keyboard found under /dev/input that this process may read.\n{}",
            crate::permission_hint().unwrap_or("")
        )));
    }

    let mut inputs = InputState::default();
    let mut last_scan = Instant::now();
    let mut poll_fds: Vec<libc::pollfd> = Vec::with_capacity(devices.len());

    let result = (|| -> io::Result<()> {
        loop {
            if control.stop_requested()? || STOP.load(Ordering::SeqCst) || host.should_stop() {
                break;
            }

            if last_scan.elapsed() >= options.rescan_interval {
                rescan(&mut devices, &options, host.as_mut());
                last_scan = Instant::now();
            }

            poll_fds.clear();
            for d in &devices {
                poll_fds.push(libc::pollfd {
                    fd: d.device.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                });
            }

            let timeout = host.next_timeout_ms().clamp(1, 500) as i32;
            let ready = unsafe {
                libc::poll(
                    poll_fds.as_mut_ptr(),
                    poll_fds.len() as libc::nfds_t,
                    timeout,
                )
            };

            if ready > 0 {
                let mut failed: Vec<PathBuf> = Vec::new();
                for (i, pfd) in poll_fds.iter().enumerate() {
                    if pfd.revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP | libc::POLLNVAL)
                        == 0
                    {
                        continue;
                    }
                    if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                        failed.push(devices[i].path.clone());
                        continue;
                    }
                    let result = pump_device(
                        &mut devices[i],
                        &mut inputs,
                        host.as_mut(),
                        &mut virtual_device,
                    );
                    match result {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                        Err(e) if e.raw_os_error() == Some(libc::ENODEV) => {
                            host.warn(&format!(
                                "input device {} ({}) stopped: {e}",
                                devices[i].path.display(),
                                devices[i].name
                            ));
                            failed.push(devices[i].path.clone());
                        }
                        Err(e) => return Err(e),
                    }
                }
                if !failed.is_empty() {
                    for path in failed {
                        if let Some(pos) = devices.iter().position(|d| d.path == path) {
                            let mut d = devices.remove(pos);
                            release_device(
                                &mut d,
                                &mut inputs,
                                host.as_mut(),
                                &mut virtual_device,
                            )?;
                            let _ = d.device.ungrab();
                        }
                    }
                    last_scan = Instant::now() - options.rescan_interval;
                }
            } else if ready < 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() != Some(libc::EINTR) {
                    return Err(err);
                }
            }

            let outputs = host.on_tick();
            if !outputs.is_empty() {
                deliver(&mut virtual_device, host.as_mut(), outputs)?;
            }
        }

        Ok(())
    })();
    let outputs = host.on_shutdown();
    let cleanup = deliver(&mut virtual_device, host.as_mut(), outputs)
        .and_then(|()| virtual_device.release_all());
    // Dropping Device ungrabs even when output or cleanup failed.
    drop(devices);
    result
        .and(cleanup)
        .map_err(|e| Error::new(format!("Linux input loop failed: {e}")))
}

struct SignalHandlers {
    int: libc::sighandler_t,
    term: libc::sighandler_t,
}
impl SignalHandlers {
    fn install() -> Self {
        unsafe {
            Self {
                int: libc::signal(
                    libc::SIGINT,
                    handle_signal as *const () as libc::sighandler_t,
                ),
                term: libc::signal(
                    libc::SIGTERM,
                    handle_signal as *const () as libc::sighandler_t,
                ),
            }
        }
    }
}
impl Drop for SignalHandlers {
    fn drop(&mut self) {
        unsafe {
            libc::signal(libc::SIGINT, self.int);
            libc::signal(libc::SIGTERM, self.term);
        }
    }
}

/// Merge identical keys held on multiple keyboards; only the last release wins.
#[derive(Default)]
struct InputState {
    counts: HashMap<u16, usize>,
}
impl InputState {
    fn transition(&mut self, device: &mut HashSet<u16>, code: u16, value: i32) -> bool {
        match value {
            0 => {
                if !device.remove(&code) {
                    return false;
                }
                let count = self.counts.entry(code).or_default();
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.counts.remove(&code);
                    true
                } else {
                    false
                }
            }
            1 | 2 => {
                if !device.insert(code) {
                    return value == 2;
                }
                let count = self.counts.entry(code).or_default();
                *count += 1;
                *count == 1
            }
            _ => false,
        }
    }
}

/// Track kernel-visible state: repeated downs must be EV_KEY value 2.
struct KeyboardOutput {
    device: evdev::uinput::VirtualDevice,
    held: HashSet<u16>,
}
impl KeyboardOutput {
    fn emit(&mut self, code: u16, value: i32) -> io::Result<()> {
        let value = output_value(&self.held, code, value);
        self.device
            .emit(&[InputEvent::new(EventType::KEY.0, code, value)])?;
        if value == 0 {
            self.held.remove(&code);
        } else {
            self.held.insert(code);
        }
        Ok(())
    }
    fn release_all(&mut self) -> io::Result<()> {
        for code in self.held.clone() {
            self.emit(code, 0)?;
        }
        Ok(())
    }
}
fn output_value(held: &HashSet<u16>, code: u16, value: i32) -> i32 {
    if value != 0 {
        if held.contains(&code) {
            2
        } else {
            1
        }
    } else {
        0
    }
}

fn build_virtual_device() -> io::Result<KeyboardOutput> {
    let mut keys: AttributeSet<LinuxKey> = AttributeSet::new();
    // Include unmapped keyboard codes too, so unknown keys pass through.
    for code in 1..=0x2ff {
        // BTN_* capabilities would make libinput classify this as a pointer
        // or game controller instead of a keyboard.
        if !(0x100..=0x15f).contains(&code) && !(0x220..=0x223).contains(&code) && code < 0x2c0 {
            keys.insert(LinuxKey(code));
        }
    }
    let device = evdev::uinput::VirtualDevice::builder()?
        .name(VIRTUAL_NAME)
        .with_keys(&keys)?
        .build()?;
    Ok(KeyboardOutput {
        device,
        held: HashSet::new(),
    })
}

/// Read everything a device has pending and feed it to the engine.
fn pump_device(
    device: &mut Grabbed,
    inputs: &mut InputState,
    host: &mut dyn Host,
    virtual_device: &mut KeyboardOutput,
) -> io::Result<()> {
    let mut batch: Vec<(u16, i32)> = Vec::new();
    for event in device.device.fetch_events()? {
        if event.event_type() == EventType::KEY {
            batch.push((event.code(), event.value()));
        }
    }

    for (code, value) in batch {
        if !inputs.transition(&mut device.held, code, value) {
            continue;
        }
        let state = match value {
            0 => KeyState::Up,
            1 | 2 => KeyState::Down,
            _ => continue,
        };
        let key = match KeyCode::from_linux_key(code) {
            Some(k) => k,
            None => {
                // Unknown to our table: always pass it through untouched.
                emit(virtual_device, code, value)?;
                continue;
            }
        };
        let outcome = host.on_key(key, state);
        // Outputs first: a held-modifier mapping must precede the key that
        // triggered it (same ordering rule as the Windows backend).
        if !outcome.outputs.is_empty() {
            deliver(virtual_device, host, outcome.outputs)?;
        }
        if !outcome.swallow {
            emit(virtual_device, code, value)?;
        }
    }
    Ok(())
}

fn deliver(
    virtual_device: &mut KeyboardOutput,
    host: &mut dyn Host,
    outputs: Vec<Output>,
) -> io::Result<()> {
    for output in outputs {
        match output {
            Output::KeyDown(k) => emit_key(virtual_device, host, k, 1)?,
            Output::KeyUp(k) => emit_key(virtual_device, host, k, 0)?,
            Output::Command(c) => host.run_command(&c),
        }
    }
    Ok(())
}

fn emit_key(
    virtual_device: &mut KeyboardOutput,
    host: &mut dyn Host,
    key: KeyCode,
    value: i32,
) -> io::Result<()> {
    let code = key.linux_key();
    if code == keyforge_core::keycode::UNSUPPORTED {
        host.warn(&format!("{key} cannot be generated on Linux"));
        return Ok(());
    }
    emit(virtual_device, code, value)
}

fn emit(virtual_device: &mut KeyboardOutput, code: u16, value: i32) -> io::Result<()> {
    virtual_device.emit(code, value)
}

/// Re-scan /dev/input: grab new keyboards, drop the ones that disappeared.
fn rescan(devices: &mut Vec<Grabbed>, options: &Options, host: &mut dyn Host) {
    for (path, device) in evdev::enumerate() {
        if devices.iter().any(|d| d.path == path) {
            continue;
        }
        let name = device.name().unwrap_or("unknown").to_string();
        if !name_selected(&name, options) {
            continue;
        }
        if !is_keyboard(&device) {
            continue;
        }
        let mut device = device;
        if let Err(e) = device.set_nonblocking(true) {
            host.warn(&format!("cannot make {name} nonblocking: {e}"));
            continue;
        }
        match device.grab() {
            Ok(()) => {
                host.warn(&format!("grabbed input device {name} ({})", path.display()));
                devices.push(Grabbed {
                    path,
                    name,
                    device,
                    held: HashSet::new(),
                });
            }
            Err(e) => host.warn(&format!("cannot grab {name} ({}): {e}", path.display())),
        }
    }
    // Disconnection is handled through poll, with releases before removal.
}

fn is_keyboard(device: &Device) -> bool {
    // Do not swallow mouse/touchpad movement from hybrid event nodes.
    if device.supported_relative_axes().is_some() || device.supported_absolute_axes().is_some() {
        return false;
    }
    match device.supported_keys() {
        Some(keys) => {
            keys.contains(LinuxKey::KEY_A)
                && keys.contains(LinuxKey::KEY_Z)
                && keys.contains(LinuxKey::KEY_ENTER)
                && keys.contains(LinuxKey::KEY_SPACE)
        }
        None => false,
    }
}

/// Devices that would be grabbed, for `keyforge devices`.
pub fn list_devices(options: &Options) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for (path, device) in evdev::enumerate() {
        let name = device.name().unwrap_or("unknown").to_string();
        let keyboard = is_keyboard(&device) && name_selected(&name, options);
        out.push(format!(
            "{}  {:<40} {}",
            path.display(),
            name,
            if keyboard {
                "keyboard (would be grabbed)"
            } else {
                "skipped (virtual, filtered, or not a keyboard)"
            }
        ));
    }
    if out.is_empty() {
        out.push("no /dev/input/event* devices are readable by this user".to_string());
    }
    Ok(out)
}

/// Focused application on Linux.
///
/// Wayland gives no portable way to ask, so this uses `xdotool` when it is
/// installed and the session is X11; otherwise the filters simply see
/// "unknown".
pub fn frontmost_app(options: &Options) -> Option<String> {
    if !options.app_probe
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
    {
        return None;
    }
    let pid = std::process::Command::new("xdotool")
        .args(["getactivewindow", "getwindowpid"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<u32>().ok())?;
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.trim().to_string())
}

/// Is this process allowed to open input devices?
pub fn can_access_input() -> bool {
    let dir = match std::fs::read_dir("/dev/input") {
        Ok(d) => d,
        Err(_) => return false,
    };
    for entry in dir.flatten() {
        if entry.file_name().to_string_lossy().starts_with("event") {
            if std::fs::OpenOptions::new()
                .read(true)
                .open(entry.path())
                .is_ok()
            {
                return true;
            }
        }
    }
    false
}

fn name_selected(name: &str, options: &Options) -> bool {
    !name.contains(VIRTUAL_NAME)
        && (options.device_filter.is_empty()
            || options
                .device_filter
                .iter()
                .any(|f| name.to_ascii_lowercase().contains(&f.to_ascii_lowercase())))
}

fn release_device(
    device: &mut Grabbed,
    inputs: &mut InputState,
    host: &mut dyn Host,
    output: &mut KeyboardOutput,
) -> io::Result<()> {
    for code in device.held.clone() {
        if !inputs.transition(&mut device.held, code, 0) {
            continue;
        }
        if let Some(key) = KeyCode::from_linux_key(code) {
            let outcome = host.on_key(key, KeyState::Up);
            deliver(output, host, outcome.outputs)?;
            if !outcome.swallow {
                emit(output, code, 0)?;
            }
        } else {
            emit(output, code, 0)?;
        }
    }
    Ok(())
}

pub fn diagnostics() -> Vec<String> {
    let mut lines = vec![format!(
        "evdev input access: {}",
        if can_access_input() {
            "readable"
        } else {
            "unavailable (permissions or no devices)"
        }
    )];
    match build_virtual_device() {
        Ok(_) => lines.push("uinput: virtual keyboard creation OK (no keys injected)".into()),
        Err(e) => lines.push(format!("uinput: unavailable ({e})")),
    }
    lines.extend(list_devices(&Options::new()).unwrap_or_default());
    lines.push(format!(
        "foreground application: {}",
        frontmost_app(&Options::new())
            .unwrap_or_else(|| "unknown (requires X11 and xdotool; Wayland unsupported)".into())
    ));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multiple_keyboards_release_only_the_last_owner() {
        let mut inputs = InputState::default();
        let mut first = HashSet::new();
        let mut second = HashSet::new();
        assert!(inputs.transition(&mut first, 30, 1));
        assert!(!inputs.transition(&mut second, 30, 1));
        assert!(inputs.transition(&mut second, 30, 2));
        assert!(!inputs.transition(&mut first, 30, 0));
        assert!(inputs.transition(&mut second, 30, 0));
        assert!(!inputs.transition(&mut second, 30, 0));
        assert!(inputs.counts.is_empty());
    }
    #[test]
    fn virtual_device_and_name_filters() {
        let mut options = Options::new();
        assert!(!name_selected(VIRTUAL_NAME, &options));
        assert!(name_selected("USB Keyboard", &options));
        options.device_filter.push("usb".into());
        assert!(name_selected("USB Keyboard", &options));
        assert!(!name_selected("Laptop Keyboard", &options));
    }
    #[test]
    fn repeated_outputs_use_kernel_repeat_value() {
        let mut held = HashSet::new();
        assert_eq!(output_value(&held, 30, 1), 1);
        held.insert(30);
        assert_eq!(output_value(&held, 30, 1), 2);
        assert_eq!(output_value(&held, 30, 2), 2);
        assert_eq!(output_value(&held, 30, 0), 0);
    }
}

pub fn startup_warnings() -> Vec<String> {
    let mut warnings = Vec::new();
    if !evdev::enumerate()
        .any(|(_, d)| is_keyboard(&d) && name_selected(d.name().unwrap_or(""), &Options::new()))
    {
        warnings.push("no readable keyboard found under /dev/input; check device access and the keyforge group".into());
    }
    if let Err(e) = std::fs::OpenOptions::new().write(true).open("/dev/uinput") {
        warnings.push(format!(
            "cannot open /dev/uinput: {e}; load uinput and install the udev rules (see README)"
        ));
    }
    warnings
}
