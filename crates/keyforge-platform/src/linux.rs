//! Linux backend: grab keyboards through evdev, emit through a uinput device.
//!
//! **Status: not wired up yet** (see README). This file is deliberately left out
//! of the module tree in `lib.rs` so the crate builds without the Linux
//! dependencies; it needs a real-hardware pass before it is enabled.
//!
//! Grabbing (EVIOCGRAB) is what makes remapping lossless: the physical events
//! never reach the rest of the system, so the virtual device is the only
//! source of input. Anything the engine does not swallow is re-emitted.

use std::io;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use evdev::{AttributeSet, EventType, InputEvent, KeyCode as LinuxKey, RawDevice};
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
    device: RawDevice,
}

pub fn run(mut host: Box<dyn Host>, options: Options) -> Result<()> {
    STOP.store(false, Ordering::SeqCst);
    install_signal_handlers();

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

    let mut last_scan = Instant::now();
    let mut poll_fds: Vec<libc::pollfd> = Vec::with_capacity(devices.len());

    loop {
        if STOP.load(Ordering::SeqCst) || host.should_stop() {
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
                if pfd.revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP) == 0 {
                    continue;
                }
                let result = pump_device(&mut devices[i], host.as_mut(), &mut virtual_device);
                match result {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        host.warn(&format!(
                            "input device {} ({}) stopped: {e}",
                            devices[i].path.display(),
                            devices[i].name
                        ));
                        failed.push(devices[i].path.clone());
                    }
                }
            }
            if !failed.is_empty() {
                for path in failed {
                    if let Some(pos) = devices.iter().position(|d| d.path == path) {
                        let d = devices.remove(pos);
                        let _ = d.device.ungrab();
                    }
                }
                last_scan = Instant::now() - options.rescan_interval;
            }
        } else if ready < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::EINTR) {
                host.warn(&format!("poll failed: {err}"));
            }
        }

        let outputs = host.on_tick();
        if !outputs.is_empty() {
            deliver(&mut virtual_device, host.as_mut(), outputs)?;
        }
    }

    for d in devices {
        let _ = d.device.ungrab();
    }
    Ok(())
}

fn install_signal_handlers() {
    unsafe {
        libc::signal(libc::SIGINT, handle_signal as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handle_signal as libc::sighandler_t);
    }
}

fn build_virtual_device() -> io::Result<evdev::uinput::VirtualDevice> {
    let mut keys: AttributeSet<LinuxKey> = AttributeSet::new();
    for key in KeyCode::ALL {
        let code = key.linux_key();
        if code != keyforge_core::keycode::UNSUPPORTED {
            keys.insert(LinuxKey(code));
        }
    }
    evdev::uinput::VirtualDeviceBuilder::new()?
        .name(VIRTUAL_NAME)
        .with_keys(&keys)?
        .build()
}

/// Read everything a device has pending and feed it to the engine.
fn pump_device(
    device: &mut Grabbed,
    host: &mut dyn Host,
    virtual_device: &mut evdev::uinput::VirtualDevice,
) -> io::Result<()> {
    let mut batch: Vec<(u16, i32)> = Vec::new();
    for event in device.device.fetch_events()? {
        if event.event_type() == EventType::KEY {
            batch.push((event.code(), event.value()));
        }
    }

    for (code, value) in batch {
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
    virtual_device: &mut evdev::uinput::VirtualDevice,
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
    virtual_device: &mut evdev::uinput::VirtualDevice,
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

fn emit(
    virtual_device: &mut evdev::uinput::VirtualDevice,
    code: u16,
    value: i32,
) -> io::Result<()> {
    let event = InputEvent::new(EventType::KEY.0, code, value);
    virtual_device.emit(&[event])
}

/// Re-scan /dev/input: grab new keyboards, drop the ones that disappeared.
fn rescan(devices: &mut Vec<Grabbed>, options: &Options, host: &mut dyn Host) {
    let mut seen: Vec<PathBuf> = Vec::new();
    for (path, device) in evdev::enumerate() {
        seen.push(path.clone());
        if devices.iter().any(|d| d.path == path) {
            continue;
        }
        let name = device.name().unwrap_or("unknown").to_string();
        if name.contains(VIRTUAL_NAME) {
            continue;
        }
        if !options.device_filter.is_empty()
            && !options
                .device_filter
                .iter()
                .any(|f| name.to_ascii_lowercase().contains(&f.to_ascii_lowercase()))
        {
            continue;
        }
        if !is_keyboard(&device) {
            continue;
        }
        let mut device = device;
        match device.grab() {
            Ok(()) => {
                host.warn(&format!("grabbed input device {name} ({})", path.display()));
                devices.push(Grabbed {
                    path,
                    name,
                    device,
                });
            }
            Err(e) => host.warn(&format!(
                "cannot grab {name} ({}): {e}",
                path.display()
            )),
        }
    }
    devices.retain(|d| seen.contains(&d.path));
}

fn is_keyboard(device: &RawDevice) -> bool {
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
pub fn list_devices(_options: &Options) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for (path, device) in evdev::enumerate() {
        let name = device.name().unwrap_or("unknown").to_string();
        let keyboard = is_keyboard(&device);
        out.push(format!(
            "{}  {:<40} {}",
            path.display(),
            name,
            if keyboard {
                "keyboard (would be grabbed)"
            } else {
                "skipped (not a keyboard)"
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
    if !options.app_probe {
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
            if std::fs::OpenOptions::new().read(true).open(entry.path()).is_ok() {
                return true;
            }
        }
    }
    false
}
