//! End to end test for the Windows backend.
//!
//! It starts the real `keyforged` binary with a temporary configuration, waits
//! until the low level hook is installed, injects the F13 key through
//! `SendInput` (an ordinary key event, not one of keyforge's own) and checks
//! that the daemon reports the F13 -> F14 remapping.
//!
//! Set `KEYFORGE_SKIP_E2E=1` to skip it (for example on a headless machine).

#![cfg(windows)]

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const VK_F13: u16 = 0x7C;
const VK_F15: u16 = 0x7E;
const INPUT_KEYBOARD: u32 = 1;
const KEYEVENTF_KEYUP: u32 = 0x0002;

#[repr(C)]
#[derive(Clone, Copy)]
struct KeybdInput {
    w_vk: u16,
    w_scan: u16,
    dw_flags: u32,
    time: u32,
    dw_extra_info: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct MouseInput {
    dx: i32,
    dy: i32,
    mouse_data: u32,
    dw_flags: u32,
    time: u32,
    dw_extra_info: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct HardwareInput {
    u_msg: u32,
    w_param_l: u16,
    w_param_h: u16,
}

#[repr(C)]
union InputUnion {
    ki: KeybdInput,
    mi: MouseInput,
    hi: HardwareInput,
}

#[repr(C)]
struct Input {
    kind: u32,
    u: InputUnion,
}

#[link(name = "user32")]
extern "system" {
    fn SendInput(count: u32, inputs: *mut Input, size: i32) -> u32;
}

/// Press and release a virtual key without keyforge's magic marker, so the
/// hook treats it as real user input.
fn tap_key(vk: u16) {
    for (flags, what) in [(0u32, "down"), (KEYEVENTF_KEYUP, "up")] {
        let mut input = Input {
            kind: INPUT_KEYBOARD,
            u: InputUnion {
                ki: KeybdInput {
                    w_vk: vk,
                    w_scan: 0,
                    dw_flags: flags,
                    time: 0,
                    // 0 = looks like a genuine key press to the hook
                    dw_extra_info: 0,
                },
            },
        };
        let sent = unsafe { SendInput(1, &mut input, std::mem::size_of::<Input>() as i32) };
        assert_eq!(sent, 1, "SendInput failed for {what} of vk {vk:#x}");
        std::thread::sleep(Duration::from_millis(30));
    }
}

fn write_config(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("keyforge-e2e.toml");
    std::fs::write(
        &path,
        r#"
[general]
default_profile = "default"
log_level = "debug"

[profiles.default]
[[profiles.default.maps]]
key = "f13"
remap = "f14"

[[profiles.default.maps]]
keys = ["ctrl", "f13"]
command = "echo keyforge-e2e"
"#,
    )
    .unwrap();
    path
}

#[test]
#[ignore = "needs a normal interactive session (integrity level Medium or higher); \
            run with `cargo test -p keyforge-daemon --test windows_e2e -- --ignored`"]
fn remaps_a_real_key_event() {
    if std::env::var("KEYFORGE_SKIP_E2E").is_ok() {
        eprintln!("KEYFORGE_SKIP_E2E set, skipping");
        return;
    }
    let bin = env!("CARGO_BIN_EXE_keyforged");
    let dir = std::env::temp_dir().join(format!("keyforge-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = write_config(&dir);

    let mut child = Command::new(bin)
        .arg("run")
        .arg("--config")
        .arg(&config)
        .arg("--log-level")
        .arg("debug")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cannot start keyforged");

    let stdout = child.stdout.take().expect("stdout");
    let stderr = child.stderr.take().expect("stderr");
    let (tx, rx) = mpsc::channel::<String>();
    // The logger writes to stderr; stdout is drained too so the child can
    // never block on a full pipe.
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
    }
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });

    let mut log: Vec<String> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut ready = false;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                if line.contains("keyboard hook installed") {
                    ready = true;
                    log.push(line);
                    break;
                }
                log.push(line);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    if !ready {
        let _ = child.kill();
        panic!(
            "keyforged never installed its keyboard hook; log so far:\n{}",
            log.join("\n")
        );
    }

    // Let the message loop settle, then inject a real F13 press.
    std::thread::sleep(Duration::from_millis(300));
    tap_key(VK_F13);
    std::thread::sleep(Duration::from_millis(200));
    // F15 is not mapped anywhere: it must simply pass through.
    tap_key(VK_F15);

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut fired = false;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                log.push(line.clone());
                if line.contains("f13") && line.contains("remap f14") {
                    fired = true;
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(
        fired,
        "keyforge did not remap the injected F13 key; log:\n{}",
        log.join("\n")
    );
    assert!(
        log.iter().any(|l| l.contains("+f14")),
        "the replacement key F14 was not injected; log:\n{}",
        log.join("\n")
    );
    // Regression guard for "pressing a types aa": a key that no mapping
    // matches must be passed through untouched and never injected again.
    assert!(
        log.iter().any(|l| l.contains("f15 down") && l.trim_end().ends_with("[]")),
        "an unmapped key must be passed through without injecting a copy; log:\n{}",
        log.join("\n")
    );
}
