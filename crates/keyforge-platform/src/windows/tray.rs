//! Notification-area controls, owned by the keyboard hook's message thread.
use super::*;

pub(super) const WM_TRAY_COMMAND: u32 = 0x8000 + 9;
const WM_TRAY: u32 = 0x8000 + 8;
const RELOAD: usize = 1;
const PAUSE: usize = 2;
const EXIT: usize = 3;
const OPEN_FOLDER: usize = 4;

thread_local! {
    static ICONS: RefCell<Option<Icons>> = const { RefCell::new(None) };
}

struct Icons {
    running: *mut c_void,
    paused: *mut c_void,
}

impl Drop for Icons {
    fn drop(&mut self) {
        unsafe {
            DestroyIcon(self.running);
            DestroyIcon(self.paused);
        }
    }
}

fn load_icon(bytes: &[u8]) -> Result<*mut c_void> {
    // These embedded ICOs contain PNG frames. Select the 32px frame.
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    for entry in bytes[6..6 + count * 16].chunks_exact(16) {
        if entry[0] != 32 {
            continue;
        }
        let len = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
        let mut frame = bytes[offset..offset + len].to_vec();
        let icon = unsafe {
            CreateIconFromResourceEx(frame.as_mut_ptr(), len as u32, 1, 0x00030000, 32, 32, 0)
        };
        if !icon.is_null() {
            return Ok(icon);
        }
    }
    Err(Error::new("cannot load the embedded tray icon"))
}

#[repr(C)]
struct WindowClass {
    style: u32,
    proc: Option<unsafe extern "system" fn(Hwnd, u32, Wparam, Lparam) -> Lresult>,
    class_extra: i32,
    window_extra: i32,
    instance: Hinstance,
    icon: *mut c_void,
    cursor: *mut c_void,
    background: *mut c_void,
    menu_name: *const u16,
    class_name: *const u16,
}

#[repr(C)]
struct NotifyIconData {
    size: u32,
    window: Hwnd,
    id: u32,
    flags: u32,
    callback: u32,
    icon: *mut c_void,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid: [u8; 16],
    balloon_icon: *mut c_void,
}

#[link(name = "user32")]
extern "system" {
    fn RegisterClassW(class: *const WindowClass) -> u16;
    fn CreateWindowExW(
        ex: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Hwnd,
        menu: *mut c_void,
        instance: Hinstance,
        param: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(window: Hwnd, msg: u32, wp: Wparam, lp: Lparam) -> Lresult;
    fn DestroyWindow(window: Hwnd) -> Bool;
    fn CreateIconFromResourceEx(
        bits: *mut u8,
        size: u32,
        icon: Bool,
        version: u32,
        width: i32,
        height: i32,
        flags: u32,
    ) -> *mut c_void;
    fn DestroyIcon(icon: *mut c_void) -> Bool;
    fn CreatePopupMenu() -> *mut c_void;
    fn AppendMenuW(menu: *mut c_void, flags: u32, id: usize, text: *const u16) -> Bool;
    fn DestroyMenu(menu: *mut c_void) -> Bool;
    fn GetCursorPos(point: *mut Point) -> Bool;
    fn SetForegroundWindow(window: Hwnd) -> Bool;
    fn TrackPopupMenu(
        menu: *mut c_void,
        flags: u32,
        x: i32,
        y: i32,
        reserved: i32,
        window: Hwnd,
        rect: *const c_void,
    ) -> u32;
    fn PostMessageW(window: Hwnd, msg: u32, wp: Wparam, lp: Lparam) -> Bool;
    fn RegisterWindowMessageW(name: *const u16) -> u32;
}

#[link(name = "shell32")]
extern "system" {
    fn Shell_NotifyIconW(message: u32, data: *const NotifyIconData) -> Bool;
    fn ShellExecuteW(
        window: Hwnd,
        operation: *const u16,
        file: *const u16,
        parameters: *const u16,
        directory: *const u16,
        show: i32,
    ) -> *mut c_void;
}

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

pub(super) struct Tray(Hwnd);

impl Tray {
    pub(super) fn new() -> Result<Self> {
        let mut icons = Icons {
            running: load_icon(include_bytes!("../../assets/running.ico"))?,
            paused: null_mut(),
        };
        icons.paused = load_icon(include_bytes!("../../assets/paused.ico"))?;
        unsafe {
            let name = wide("KeyforgeTrayWindow");
            let instance = GetModuleHandleW(null());
            let class = WindowClass {
                style: 0,
                proc: Some(window_proc),
                class_extra: 0,
                window_extra: 0,
                instance,
                icon: null_mut(),
                cursor: null_mut(),
                background: null_mut(),
                menu_name: null(),
                class_name: name.as_ptr(),
            };
            if RegisterClassW(&class) == 0 && GetLastError() != 1410 {
                return Err(Error::new("cannot register the tray window class"));
            }
            TASKBAR_CREATED.store(
                RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
                Ordering::SeqCst,
            );
            // A hidden top-level window receives Explorer's restart broadcast.
            let window = CreateWindowExW(
                0x80,
                name.as_ptr(),
                name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance,
                null_mut(),
            );
            if window.is_null() {
                return Err(Error::new("cannot create the tray window"));
            }
            let tray = Self(window);
            ICONS.with(|slot| *slot.borrow_mut() = Some(icons));
            if !notify(window, 0) {
                return Err(Error::new("cannot add the keyforge notification icon"));
            }
            keyforge_core::log_info!("notification icon installed");
            Ok(tray)
        }
    }

    pub(super) fn update(&self) {
        if !notify(self.0, 1) {
            keyforge_core::log_warn!("cannot update the keyforge notification icon");
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        notify(self.0, 2);
        unsafe {
            DestroyWindow(self.0);
        }
        ICONS.with(|slot| slot.borrow_mut().take());
    }
}

fn notify(window: Hwnd, message: u32) -> bool {
    let paused = with_host(|h| h.is_paused()).unwrap_or(false);
    let mut data: NotifyIconData = unsafe { std::mem::zeroed() };
    data.size = std::mem::size_of::<NotifyIconData>() as u32;
    data.window = window;
    data.id = 1;
    data.flags = 1 | 2 | 4; // NIF_MESSAGE | NIF_ICON | NIF_TIP
    data.callback = WM_TRAY;
    data.icon = ICONS.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|icons| if paused { icons.paused } else { icons.running })
            .unwrap_or(null_mut())
    });
    let tip = wide(if paused {
        "Keyforge - paused"
    } else {
        "Keyforge - running"
    });
    data.tip[..tip.len()].copy_from_slice(&tip);
    unsafe { Shell_NotifyIconW(message, &data) != 0 }
}

unsafe extern "system" fn window_proc(window: Hwnd, msg: u32, wp: Wparam, lp: Lparam) -> Lresult {
    let restart = TASKBAR_CREATED.load(Ordering::SeqCst);
    if restart != 0 && msg == restart {
        if !notify(window, 0) {
            keyforge_core::log_warn!("cannot restore the tray icon after Explorer restarted");
        }
        return 0;
    }
    if msg == WM_TRAY && matches!(lp as u32, 0x0205 | 0x007B) {
        // right up / context menu
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return 0;
        }
        let paused = with_host(|h| h.is_paused()).unwrap_or(false);
        let config = with_host(|h| h.config_path().map(|p| p.to_path_buf())).flatten();
        let label = config
            .as_ref()
            .map(|p| format!("Config: {}", p.display()))
            .unwrap_or_else(|| "Config: unavailable".into())
            .replace('&', "&&");
        AppendMenuW(menu, 2, 0, wide(&label).as_ptr()); // disabled informational row
        AppendMenuW(
            menu,
            if config.is_some() { 0 } else { 2 },
            OPEN_FOLDER,
            wide("Open config folder").as_ptr(),
        );
        AppendMenuW(menu, 0x800, 0, null());
        AppendMenuW(menu, 0, RELOAD, wide("Reload config").as_ptr());
        AppendMenuW(
            menu,
            0,
            PAUSE,
            wide(if paused { "Resume" } else { "Pause" }).as_ptr(),
        );
        AppendMenuW(menu, 0x800, 0, null()); // separator
        AppendMenuW(menu, 0, EXIT, wide("Exit").as_ptr());
        let mut point = Point { x: 0, y: 0 };
        GetCursorPos(&mut point);
        SetForegroundWindow(window);
        // Return the choice so host work runs outside this nested menu loop.
        let choice = TrackPopupMenu(menu, 0x0100 | 0x0002, point.x, point.y, 0, window, null());
        DestroyMenu(menu);
        PostMessageW(window, 0, 0, 0); // allow dismissal on the next invocation
        if choice != 0 {
            PostThreadMessageW(GetCurrentThreadId(), WM_TRAY_COMMAND, choice as usize, 0);
        }
        return 0;
    }
    DefWindowProcW(window, msg, wp, lp)
}

pub(super) fn handle_command(command: usize) {
    if command == OPEN_FOLDER {
        let config = with_host(|h| h.config_path().map(|p| p.to_path_buf())).flatten();
        if let Some(folder) = config.as_deref().and_then(|p| p.parent()) {
            use std::os::windows::ffi::OsStrExt;
            let folder: Vec<u16> = folder.as_os_str().encode_wide().chain(Some(0)).collect();
            let result = unsafe {
                ShellExecuteW(
                    null_mut(),
                    wide("open").as_ptr(),
                    folder.as_ptr(),
                    null(),
                    null(),
                    1,
                )
            };
            if result as isize <= 32 {
                keyforge_core::log_warn!(
                    "cannot open the configuration folder (shell error {})",
                    result as isize
                );
            }
        }
        return;
    }
    if command == EXIT {
        request_stop();
        return;
    }
    with_host(|host| {
        let outputs = match command {
            RELOAD => host.reload_config(),
            PAUSE => host.toggle_pause(),
            _ => Vec::new(),
        };
        deliver(host, outputs);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_structs_match_windows_layout() {
        assert_eq!(
            std::mem::size_of::<NotifyIconData>(),
            if cfg!(target_pointer_width = "64") {
                976
            } else {
                956
            }
        );
        assert_eq!(
            std::mem::size_of::<WindowClass>(),
            if cfg!(target_pointer_width = "64") {
                72
            } else {
                40
            }
        );
    }
}
