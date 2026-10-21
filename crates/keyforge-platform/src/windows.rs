//! Windows backend: `WH_KEYBOARD_LL` hook + `SendInput`.
//!
//! Design notes
//! ------------
//! * The hook must live on a thread that pumps messages, so the daemon's main
//!   thread becomes that thread and `MsgWaitForMultipleObjectsEx` replaces a
//!   plain `GetMessage` loop so timers (tap-hold, config reload) still run.
//! * `SendInput` is never called from inside the hook procedure: the hook only
//!   decides and queues, the message loop injects. This avoids the documented
//!   re-entrancy problems of low level hooks.
//! * Injected events carry a magic `dwExtraInfo` value and are passed straight
//!   through, so the hook can never remap its own output.

use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Once;

use keyforge_core::{KeyCode, KeyState, Output};

use crate::{Error, Host, Result};

mod tray;

// ---------------------------------------------------------------------------
// Win32 declarations
// ---------------------------------------------------------------------------

type Bool = i32;
type Hhook = *mut c_void;
type Hinstance = *mut c_void;
type Hwnd = *mut c_void;
type Lparam = isize;
type Lresult = isize;
type Wparam = usize;

const WH_KEYBOARD_LL: i32 = 13;
const WM_KEYDOWN: u32 = 0x0100;
const WM_KEYUP: u32 = 0x0101;
const WM_SYSKEYDOWN: u32 = 0x0104;
const WM_SYSKEYUP: u32 = 0x0105;
const WM_QUIT: u32 = 0x0012;
const WM_APP_KEYFORGE: u32 = 0x8000 + 7;
const PM_REMOVE: u32 = 0x0001;
const QS_ALLINPUT: u32 = 0x04FF;
const MWMO_INPUTAVAILABLE: u32 = 0x0004;
const INPUT_KEYBOARD: u32 = 1;
const KEYEVENTF_EXTENDEDKEY: u32 = 0x0001;
const KEYEVENTF_KEYUP: u32 = 0x0002;
const LLKHF_EXTENDED: u32 = 0x01;
const LLKHF_UP: u32 = 0x80;
const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
/// Marks our own synthetic input so the hook ignores it.
const MAGIC: usize = 0x4B46_4F52_4745; // "KFORGE"

#[repr(C)]
struct KbdllHookStruct {
    vk_code: u32,
    scan_code: u32,
    flags: u32,
    time: u32,
    dw_extra_info: usize,
}

#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

#[repr(C)]
struct Msg {
    hwnd: Hwnd,
    message: u32,
    w_param: Wparam,
    l_param: Lparam,
    time: u32,
    pt: Point,
    l_private: u32,
}

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

type HookProc = unsafe extern "system" fn(i32, Wparam, Lparam) -> Lresult;
type CtrlHandler = unsafe extern "system" fn(u32) -> Bool;

#[link(name = "user32")]
extern "system" {
    fn SetWindowsHookExW(idhook: i32, lpfn: Option<HookProc>, hmod: Hinstance, tid: u32) -> Hhook;
    fn UnhookWindowsHookEx(hhk: Hhook) -> Bool;
    fn CallNextHookEx(hhk: Hhook, ncode: i32, wparam: Wparam, lparam: Lparam) -> Lresult;
    fn PeekMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32, remove: u32) -> Bool;
    fn TranslateMessage(msg: *const Msg) -> Bool;
    fn DispatchMessageW(msg: *const Msg) -> Lresult;
    fn PostThreadMessageW(tid: u32, msg: u32, wparam: Wparam, lparam: Lparam) -> Bool;
    fn MsgWaitForMultipleObjectsEx(
        count: u32,
        handles: *const *mut c_void,
        millis: u32,
        wake_mask: u32,
        flags: u32,
    ) -> u32;
    fn SendInput(count: u32, inputs: *mut Input, size: i32) -> u32;
    fn GetForegroundWindow() -> Hwnd;
    fn GetWindowThreadProcessId(hwnd: Hwnd, pid: *mut u32) -> u32;
    fn SetConsoleCtrlHandler(handler: Option<CtrlHandler>, add: Bool) -> Bool;
}

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleW(name: *const u16) -> Hinstance;
    fn GetCurrentThreadId() -> u32;
    fn GetLastError() -> u32;
    fn GetCurrentProcess() -> *mut c_void;
    fn OpenProcess(access: u32, inherit: Bool, pid: u32) -> *mut c_void;
    fn CloseHandle(handle: *mut c_void) -> Bool;
    fn CreateEventW(
        attrs: *mut c_void,
        manual_reset: Bool,
        initial: Bool,
        name: *const u16,
    ) -> *mut c_void;
    fn OpenEventW(access: u32, inherit: Bool, name: *const u16) -> *mut c_void;
    fn SetEvent(handle: *mut c_void) -> Bool;
    fn QueryFullProcessImageNameW(
        process: *mut c_void,
        flags: u32,
        name: *mut u16,
        size: *mut u32,
    ) -> Bool;
}

#[link(name = "advapi32")]
extern "system" {
    fn OpenProcessToken(process: *mut c_void, access: u32, token: *mut *mut c_void) -> Bool;
    fn GetTokenInformation(
        token: *mut c_void,
        class: u32,
        info: *mut c_void,
        len: u32,
        returned: *mut u32,
    ) -> Bool;
    fn GetSidSubAuthorityCount(sid: *mut c_void) -> *mut u8;
    fn GetSidSubAuthority(sid: *mut c_void, index: u32) -> *mut u32;
}

// ---------------------------------------------------------------------------
// shared state
// ---------------------------------------------------------------------------

thread_local! {
    static HOST: RefCell<Option<Box<dyn Host>>> = const { RefCell::new(None) };
    /// `command` mappings are executed by the message loop, never in the hook.
    static COMMANDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

static STOP: AtomicBool = AtomicBool::new(false);
static THREAD_ID: AtomicU32 = AtomicU32::new(0);
/// Handle of the named stop event (0 when not created).
static STOP_EVENT: AtomicUsize = AtomicUsize::new(0);
static HANDLER: Once = Once::new();

/// Session local event all keyforge instances listen on.
const STOP_EVENT_NAME: &str = "Local\\keyforge-stop";
const EVENT_MODIFY_STATE: u32 = 0x0002;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_ALREADY_EXISTS: u32 = 183;
const WAIT_OBJECT_0: u32 = 0;

fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Create the stop event. Returns `(handle, already_existed)`.
///
/// `already_existed` is how we notice a second instance: two daemons would
/// fight over the same keys, which looks exactly like "the remapping is still
/// active after I quit".
fn create_stop_event() -> (*mut c_void, bool) {
    let name = wide(STOP_EVENT_NAME);
    let handle = unsafe { CreateEventW(null_mut(), 1, 0, name.as_ptr()) };
    if handle.is_null() {
        return (null_mut(), false);
    }
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    (handle, existed)
}

/// Is another instance currently listening for the stop signal?
pub fn instance_running() -> bool {
    let name = wide(STOP_EVENT_NAME);
    let handle = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
    if handle.is_null() {
        return false;
    }
    unsafe { CloseHandle(handle) };
    true
}

/// Ask the running instance(s) to stop gracefully.
pub fn signal_stop() -> std::result::Result<bool, String> {
    let name = wide(STOP_EVENT_NAME);
    let handle = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
    if handle.is_null() {
        let err = unsafe { GetLastError() };
        if err == ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        return Err(format!("OpenEventW failed (win32 error {err})"));
    }
    let ok = unsafe { SetEvent(handle) };
    unsafe { CloseHandle(handle) };
    if ok == 0 {
        return Err(format!(
            "SetEvent failed (win32 error {})",
            unsafe { GetLastError() }
        ));
    }
    Ok(true)
}

/// Ask the running backend to shut down.
pub fn request_stop() {
    STOP.store(true, Ordering::SeqCst);
    let event = STOP_EVENT.load(Ordering::SeqCst);
    if event != 0 {
        unsafe { SetEvent(event as *mut c_void) };
    }
    let tid = THREAD_ID.load(Ordering::SeqCst);
    if tid != 0 {
        unsafe { PostThreadMessageW(tid, WM_QUIT, 0, 0) };
    }
}

pub unsafe extern "system" fn ctrl_handler(kind: u32) -> Bool {
    // CTRL_C, CTRL_BREAK, CTRL_CLOSE, CTRL_LOGOFF, CTRL_SHUTDOWN
    if matches!(kind, 0 | 1 | 2 | 5 | 6) {
        request_stop();
        return 1;
    }
    0
}

unsafe extern "system" fn hook_proc(code: i32, wparam: Wparam, lparam: Lparam) -> Lresult {
    if code < 0 {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }
    let message = wparam as u32;
    if !matches!(
        message,
        WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP
    ) {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }
    let kb = &*(lparam as *const KbdllHookStruct);
    // Our own synthetic input: never remap it.
    if kb.dw_extra_info == MAGIC {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }
    let extended = kb.flags & LLKHF_EXTENDED != 0;
    let key = match KeyCode::from_windows_vk(kb.vk_code as u16, extended) {
        Some(k) => k,
        None => return CallNextHookEx(null_mut(), code, wparam, lparam),
    };
    let state = if kb.flags & LLKHF_UP != 0 {
        KeyState::Up
    } else {
        KeyState::Down
    };

    // `try_borrow_mut` keeps the hook safe if Windows re-enters it while the
    // message loop is working; in that case the event simply passes through.
    let decided = HOST.with(|slot| {
        let mut borrowed = match slot.try_borrow_mut() {
            Ok(b) => b,
            Err(_) => return None,
        };
        match borrowed.as_mut() {
            Some(host) => {
                let outcome = host.on_key(key, state);
                // Inject synchronously: a held-modifier mapping must reach the
                // system before the key that triggered it.
                deliver(host.as_mut(), outcome.outputs);
                Some(outcome.swallow)
            }
            None => None,
        }
    });

    match decided {
        Some(true) => 1,
        _ => CallNextHookEx(null_mut(), code, wparam, lparam),
    }
}

/// Inject one key transition. `false` on failure.
fn inject(key: KeyCode, down: bool) -> bool {
    let vk = key.windows_vk();
    if vk == keyforge_core::keycode::UNSUPPORTED {
        return false;
    }
    let mut flags = 0u32;
    if key.windows_extended() {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    let mut input = Input {
        kind: INPUT_KEYBOARD,
        u: InputUnion {
            ki: KeybdInput {
                w_vk: vk,
                w_scan: 0,
                dw_flags: flags,
                time: 0,
                dw_extra_info: MAGIC,
            },
        },
    };
    let sent = unsafe { SendInput(1, &mut input, std::mem::size_of::<Input>() as i32) };
    sent == 1
}

/// Run one round of output: keys are injected right here, commands are queued
/// for the message loop (shell commands must not run inside a hook).
fn deliver(host: &mut dyn Host, outputs: Vec<Output>) {
    let mut queued = false;
    for output in outputs {
        match output {
            Output::KeyDown(k) => {
                if !inject(k, true) {
                    host.warn(&format!("cannot inject {k} (down)"));
                }
            }
            Output::KeyUp(k) => {
                if !inject(k, false) {
                    host.warn(&format!("cannot inject {k} (up)"));
                }
            }
            Output::Command(c) => {
                COMMANDS.with(|q| q.borrow_mut().push(c));
                queued = true;
            }
        }
    }
    if queued {
        let tid = THREAD_ID.load(Ordering::SeqCst);
        if tid != 0 {
            unsafe { PostThreadMessageW(tid, WM_APP_KEYFORGE, 0, 0) };
        }
    }
}

fn run_queued_commands() {
    let commands = COMMANDS.with(|q| std::mem::take(&mut *q.borrow_mut()));
    if commands.is_empty() {
        return;
    }
    with_host(|host| {
        for c in &commands {
            host.run_command(c);
        }
    });
}

fn with_host<R>(f: impl FnOnce(&mut dyn Host) -> R) -> Option<R> {
    HOST.with(|slot| {
        let mut borrowed = slot.try_borrow_mut().ok()?;
        borrowed.as_mut().map(|host| f(host.as_mut()))
    })
}

fn timeout_ms() -> u32 {
    with_host(|host| host.next_timeout_ms()).unwrap_or(250).clamp(1, 500)
}

// ---------------------------------------------------------------------------
// entry point
// ---------------------------------------------------------------------------

pub fn run(host: Box<dyn Host>) -> Result<()> {
    HANDLER.call_once(|| unsafe {
        SetConsoleCtrlHandler(Some(ctrl_handler), 1);
    });
    STOP.store(false, Ordering::SeqCst);

    let (stop_event, existed) = create_stop_event();
    if !stop_event.is_null() {
        STOP_EVENT.store(stop_event as usize, Ordering::SeqCst);
    }
    if existed {
        keyforge_core::log_warn!(
            "another keyforge instance is already running in this session; \
             two daemons fight over the same keys. Stop it with `keyforge stop` first."
        );
    }

    let tid = unsafe { GetCurrentThreadId() };
    THREAD_ID.store(tid, Ordering::SeqCst);

    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), GetModuleHandleW(null()), 0) };
    if hook.is_null() {
        let error = unsafe { GetLastError() };
        release_stop_event();
        return Err(Error::new(format!(
            "SetWindowsHookExW(WH_KEYBOARD_LL) failed (win32 error {error}); \
             keyboard hooks are not available in this session"
        )));
    }

    HOST.with(|slot| *slot.borrow_mut() = Some(host));
    keyforge_core::log_info!("keyboard hook installed (thread {tid}); remapping is live");
    let result = match tray::Tray::new() {
        Ok(tray) => message_loop(&tray),
        Err(error) => Err(error),
    };

    // Give back every key we still hold, otherwise a mapping that happened to
    // be active while stopping would stay pressed in the system.
    let pending = with_host(|h| h.on_shutdown()).unwrap_or_default();
    if !pending.is_empty() {
        with_host(|h| deliver(h, pending));
    }

    HOST.with(|slot| *slot.borrow_mut() = None);
    unsafe { UnhookWindowsHookEx(hook) };
    release_stop_event();
    keyforge_core::log_info!("keyboard hook removed");
    result
}

fn release_stop_event() {
    let handle = STOP_EVENT.swap(0, Ordering::SeqCst);
    if handle != 0 {
        unsafe { CloseHandle(handle as *mut c_void) };
    }
}

fn message_loop(tray: &tray::Tray) -> Result<()> {
    let mut msg: Msg = unsafe { std::mem::zeroed() };
    loop {
        if STOP.load(Ordering::SeqCst) {
            return Ok(());
        }
        if with_host(|host| host.should_stop()).unwrap_or(true) {
            return Ok(());
        }

        // Wait for input, the stop event, or the next timer deadline.
        let event = STOP_EVENT.load(Ordering::SeqCst) as *mut c_void;
        let handles = [event];
        let (count, ptr) = if event.is_null() {
            (0u32, null())
        } else {
            (1u32, handles.as_ptr())
        };
        let rc = unsafe {
            MsgWaitForMultipleObjectsEx(count, ptr, timeout_ms(), QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        };
        // Index 0 of the wait array is the stop event (messages come after it).
        if !event.is_null() && rc == WAIT_OBJECT_0 {
            keyforge_core::log_info!("stop requested by another keyforge process");
            return Ok(());
        }

        loop {
            let has = unsafe { PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) };
            if has == 0 {
                break;
            }
            if msg.message == WM_QUIT {
                return Ok(());
            }
            if msg.message == WM_APP_KEYFORGE {
                run_queued_commands();
                continue;
            }
            if msg.message == tray::WM_TRAY_COMMAND {
                tray::handle_command(msg.w_param);
                tray.update();
                continue;
            }
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        let tick = with_host(|host| host.on_tick()).unwrap_or_default();
        if !tick.is_empty() {
            with_host(|host| deliver(host, tick));
        }
        run_queued_commands();
    }
}

/// File name of the process owning the foreground window, e.g. `code.exe`.
pub fn frontmost_app() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(process);
        if ok == 0 || size == 0 {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..size as usize]);
        Some(
            full.rsplit(['\\', '/'])
                .next()
                .unwrap_or(full.as_str())
                .to_string(),
        )
    }
}

// ---------------------------------------------------------------------------
// diagnostics (`keyforge doctor`)
// ---------------------------------------------------------------------------

/// Integrity RID of the current process: `0x1000` = Low, `0x2000` = Medium,
/// `0x3000` = High, ...
///
/// This matters a lot: a low integrity process may install a low level hook,
/// but UIPI only lets it observe input that is going to processes at the same
/// or a lower integrity level — so remapping would look like it only works in
/// the terminal keyforge was launched from.
pub fn integrity_rid() -> Option<u32> {
    unsafe {
        let mut token: *mut c_void = null_mut();
        if OpenProcessToken(GetCurrentProcess(), 0x0008 /* TOKEN_QUERY */, &mut token) == 0 {
            return None;
        }
        let rid = token_integrity_rid(token);
        CloseHandle(token);
        rid
    }
}

/// Integrity RID of an already opened token.
unsafe fn token_integrity_rid(token: *mut c_void) -> Option<u32> {
    let mut buf = [0u8; 256];
    let mut returned = 0u32;
    // TokenIntegrityLevel = 25
    let ok = GetTokenInformation(
        token,
        25,
        buf.as_mut_ptr() as *mut c_void,
        buf.len() as u32,
        &mut returned,
    );
    if ok == 0 {
        return None;
    }
    // TOKEN_MANDATORY_LABEL { SID_AND_ATTRIBUTES { PSID Sid; DWORD Attributes } }
    let sid = *(buf.as_ptr() as *const *mut c_void);
    if sid.is_null() {
        return None;
    }
    let count = *GetSidSubAuthorityCount(sid);
    if count == 0 {
        return None;
    }
    Some(*GetSidSubAuthority(sid, (count - 1) as u32))
}

/// Integrity level of the process owning the foreground window.
///
/// This is the decisive check for "remapping only works in some windows":
/// a hook installed by a lower integrity process never receives the keystrokes
/// that are going to a higher integrity window.
pub fn frontmost_integrity_rid() -> Option<u32> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut token: *mut c_void = null_mut();
        let opened = OpenProcessToken(process, 0x0008 /* TOKEN_QUERY */, &mut token);
        CloseHandle(process);
        if opened == 0 {
            return None;
        }
        let rid = token_integrity_rid(token);
        CloseHandle(token);
        rid
    }
}

pub fn integrity_name(rid: u32) -> &'static str {
    match rid {
        0x0000 => "Untrusted",
        0x1000 => "Low",
        0x2000 => "Medium",
        0x2100 => "Medium Plus",
        0x3000 => "High",
        0x4000 => "System",
        _ => "Unknown",
    }
}

/// Integrity level of the current process, for `keyforge doctor`.
pub fn integrity_level() -> String {
    match integrity_rid() {
        Some(rid) => format!("{} (0x{rid:x})", integrity_name(rid)),
        None => "unknown (cannot read the process token)".to_string(),
    }
}

/// Install and immediately remove the hook: is the hook mechanism available?
pub fn probe_install_hook() -> std::result::Result<(), String> {
    let hook = unsafe {
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(probe_hook_proc),
            GetModuleHandleW(null()),
            0,
        )
    };
    if hook.is_null() {
        return Err(format!(
            "SetWindowsHookExW failed (win32 error {})",
            unsafe { GetLastError() }
        ));
    }
    unsafe { UnhookWindowsHookEx(hook) };
    Ok(())
}

static PROBE_SEEN: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn probe_hook_proc(code: i32, wparam: Wparam, lparam: Lparam) -> Lresult {
    if code >= 0 {
        let kb = &*(lparam as *const KbdllHookStruct);
        if kb.dw_extra_info != MAGIC && kb.vk_code == 0x7C {
            PROBE_SEEN.store(true, Ordering::SeqCst);
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

/// Install the hook, inject one F13 (a genuine looking event) and see whether
/// it comes back through the hook. This is the same code path the daemon uses.
pub fn probe_hook_roundtrip() -> std::result::Result<bool, String> {
    PROBE_SEEN.store(false, Ordering::SeqCst);
    let hook = unsafe {
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(probe_hook_proc),
            GetModuleHandleW(null()),
            0,
        )
    };
    if hook.is_null() {
        return Err(format!(
            "SetWindowsHookExW failed (win32 error {})",
            unsafe { GetLastError() }
        ));
    }

    // Inject from another thread shortly after we start pumping messages.
    let injector = std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(150));
        let mut down = Input {
            kind: INPUT_KEYBOARD,
            u: InputUnion {
                ki: KeybdInput {
                    w_vk: 0x7C,
                    w_scan: 0,
                    dw_flags: 0,
                    time: 0,
                    dw_extra_info: 0,
                },
            },
        };
        let mut up = Input {
            kind: INPUT_KEYBOARD,
            u: InputUnion {
                ki: KeybdInput {
                    w_vk: 0x7C,
                    w_scan: 0,
                    dw_flags: KEYEVENTF_KEYUP,
                    time: 0,
                    dw_extra_info: 0,
                },
            },
        };
        unsafe {
            SendInput(1, &mut down, std::mem::size_of::<Input>() as i32);
            std::thread::sleep(std::time::Duration::from_millis(30));
            SendInput(1, &mut up, std::mem::size_of::<Input>() as i32);
        }
    });

    let mut msg: Msg = unsafe { std::mem::zeroed() };
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_millis(1500) {
        unsafe {
            MsgWaitForMultipleObjectsEx(0, null(), 50, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        }
        loop {
            let has = unsafe { PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) };
            if has == 0 {
                break;
            }
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if PROBE_SEEN.load(Ordering::SeqCst) {
            break;
        }
    }
    let _ = injector.join();
    unsafe { UnhookWindowsHookEx(hook) };
    Ok(PROBE_SEEN.load(Ordering::SeqCst))
}

/// Can `SendInput` place a key event into the session input queue?
pub fn probe_send_input() -> std::result::Result<bool, String> {
    let mut input = Input {
        kind: INPUT_KEYBOARD,
        u: InputUnion {
            ki: KeybdInput {
                w_vk: 0x7C,
                w_scan: 0,
                dw_flags: 0,
                time: 0,
                dw_extra_info: MAGIC,
            },
        },
    };
    let sent = unsafe { SendInput(1, &mut input, std::mem::size_of::<Input>() as i32) };
    if sent == 1 {
        let mut up = Input {
            kind: INPUT_KEYBOARD,
            u: InputUnion {
                ki: KeybdInput {
                    w_vk: 0x7C,
                    w_scan: 0,
                    dw_flags: KEYEVENTF_KEYUP,
                    time: 0,
                    dw_extra_info: MAGIC,
                },
            },
        };
        unsafe { SendInput(1, &mut up, std::mem::size_of::<Input>() as i32) };
        Ok(true)
    } else {
        Err(format!(
            "SendInput accepted 0 of 1 events (win32 error {})",
            unsafe { GetLastError() }
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_struct_has_the_documented_size() {
        // 40 bytes on x64, 28 on x86: a wrong size makes SendInput a no-op.
        let expected = if cfg!(target_pointer_width = "64") { 40 } else { 28 };
        assert_eq!(std::mem::size_of::<Input>(), expected);
        assert_eq!(std::mem::size_of::<KbdllHookStruct>(), if cfg!(target_pointer_width = "64") { 24 } else { 20 });
    }

    #[test]
    fn keycode_translation_matches_win32_constants() {
        assert_eq!(KeyCode::LeftCtrl.windows_vk(), 0xA2);
        assert_eq!(KeyCode::Enter.windows_vk(), 0x0D);
        assert!(KeyCode::KpEnter.windows_extended());
        assert_eq!(KeyCode::VolumeUp.windows_vk(), 0xAF);
    }
}
