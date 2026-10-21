//! Windows binary packaging and interactive tray lifecycle regression checks.
#![cfg(windows)]

use std::ffi::c_void;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

#[test]
fn daemon_uses_gui_subsystem_while_cli_keeps_console() {
    for (binary, expected) in [
        (env!("CARGO_BIN_EXE_keyforged"), 2u16),
        (env!("CARGO_BIN_EXE_keyforge"), 3u16),
    ] {
        let bytes = std::fs::read(binary).unwrap();
        let pe = u32::from_le_bytes(bytes[0x3c..0x40].try_into().unwrap()) as usize;
        assert_eq!(&bytes[pe..pe + 4], b"PE\0\0");
        let offset = pe + 24 + 68; // OptionalHeader.Subsystem (PE32 and PE32+)
        assert_eq!(
            u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap()),
            expected
        );
    }
}

#[link(name = "user32")]
extern "system" {
    fn FindWindowW(class: *const u16, title: *const u16) -> *mut c_void;
    fn GetWindowThreadProcessId(window: *mut c_void, pid: *mut u32) -> u32;
    fn PostThreadMessageW(thread: u32, message: u32, wp: usize, lp: isize) -> i32;
}

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "tray check timed out"
        );
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn log_contains(log: &Path, text: &str) -> bool {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .contains(text)
}

#[test]
#[ignore = "starts a tray icon and keyboard hook; needs an interactive desktop with no daemon running"]
fn tray_commands_reload_pause_resume_and_exit() {
    assert!(
        !keyforge_platform::instance_running(),
        "stop the existing daemon before this test"
    );
    let dir = std::env::temp_dir().join(format!("keyforge-tray-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = dir.join("config.toml");
    let log = dir.join("daemon.log");
    std::fs::write(&config, "[profiles.default]\n").unwrap();
    let mut child = Running(
        Command::new(env!("CARGO_BIN_EXE_keyforged"))
            .arg("--config")
            .arg(&config)
            .arg("--log-file")
            .arg(&log)
            .spawn()
            .unwrap(),
    );
    let class: Vec<u16> = "KeyforgeTrayWindow".encode_utf16().chain(Some(0)).collect();
    let mut thread = 0;
    wait_for(|| {
        let window = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
        let mut pid = 0;
        if !window.is_null() {
            thread = unsafe { GetWindowThreadProcessId(window, &mut pid) };
        }
        pid == child.0.id() && log_contains(&log, "notification icon installed")
    });
    let command = |id| assert_ne!(unsafe { PostThreadMessageW(thread, 0x8000 + 9, id, 0) }, 0);
    command(2);
    wait_for(|| log_contains(&log, "remapping paused"));
    std::fs::write(
        &config,
        "[profiles.default]\n[[profiles.default.maps]]\nkey = 'f13'\nremap = 'f14'\n",
    )
    .unwrap();
    command(1); // watch is off; explicit reload must still apply
    wait_for(|| log_contains(&log, "1 profiles, 1 mappings"));
    std::fs::write(&config, "invalid = [").unwrap();
    command(1);
    wait_for(|| {
        log_contains(
            &log,
            "config reload failed, keeping the previous configuration",
        )
    });
    command(2);
    wait_for(|| log_contains(&log, "remapping resumed"));
    command(3);
    wait_for(|| child.0.try_wait().unwrap().is_some());
    assert!(child.0.wait().unwrap().success());
    assert!(log_contains(&log, "keyboard hook removed"));
    assert!(log_contains(&log, "keyforge stopped"));
    std::fs::remove_file(&config).unwrap();
    std::fs::remove_file(&log).unwrap();
    std::fs::remove_dir(&dir).unwrap();
}
