//! Canonical key codes plus per-platform translation tables.
//!
//! One table drives everything: the enum itself, config name parsing, and the
//! Windows virtual-key / Linux evdev / macOS virtual-keycode lookups. Adding a
//! key means adding a single row.

use std::fmt;

/// Marker used in the table for "this key does not exist on that platform".
pub const UNSUPPORTED: u16 = 0xFFFF;

macro_rules! key_table {
    ($( $variant:ident => $name:literal, $win:expr, $lin:expr, $mac:expr, $ext:expr; )*) => {
        /// A physical key, named identically on every platform.
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        #[allow(non_camel_case_types)]
        pub enum KeyCode {
            $($variant),*
        }

        impl KeyCode {
            /// Every known key, in table order.
            pub const ALL: &'static [KeyCode] = &[$(KeyCode::$variant),*];

            /// Canonical (config file) name.
            pub fn name(self) -> &'static str {
                match self {
                    $(KeyCode::$variant => $name),*
                }
            }

            /// Windows virtual-key code, or [`UNSUPPORTED`].
            pub fn windows_vk(self) -> u16 {
                match self {
                    $(KeyCode::$variant => $win),*
                }
            }

            /// Linux evdev `KEY_*` code, or [`UNSUPPORTED`].
            pub fn linux_key(self) -> u16 {
                match self {
                    $(KeyCode::$variant => $lin),*
                }
            }

            /// macOS virtual keycode (Carbon `kVK_*`), or [`UNSUPPORTED`].
            pub fn macos_keycode(self) -> u16 {
                match self {
                    $(KeyCode::$variant => $mac),*
                }
            }

            /// Windows: does this key need `KEYEVENTF_EXTENDEDKEY`?
            pub fn windows_extended(self) -> bool {
                match self {
                    $(KeyCode::$variant => $ext),*
                }
            }
        }

        fn from_normalized(n: &str) -> Option<KeyCode> {
            match n {
                $($name => Some(KeyCode::$variant),)*
                other => aliases(other),
            }
        }
    };
}

key_table! {
    // --- letters -----------------------------------------------------------
    A => "a", 0x41, 30, 0x00, false;
    B => "b", 0x42, 48, 0x0B, false;
    C => "c", 0x43, 46, 0x08, false;
    D => "d", 0x44, 32, 0x02, false;
    E => "e", 0x45, 18, 0x0E, false;
    F => "f", 0x46, 33, 0x03, false;
    G => "g", 0x47, 34, 0x05, false;
    H => "h", 0x48, 35, 0x04, false;
    I => "i", 0x49, 23, 0x22, false;
    J => "j", 0x4A, 36, 0x26, false;
    K => "k", 0x4B, 37, 0x28, false;
    L => "l", 0x4C, 38, 0x25, false;
    M => "m", 0x4D, 50, 0x2E, false;
    N => "n", 0x4E, 49, 0x2D, false;
    O => "o", 0x4F, 24, 0x1F, false;
    P => "p", 0x50, 25, 0x23, false;
    Q => "q", 0x51, 16, 0x0C, false;
    R => "r", 0x52, 19, 0x0F, false;
    S => "s", 0x53, 31, 0x01, false;
    T => "t", 0x54, 20, 0x11, false;
    U => "u", 0x55, 22, 0x20, false;
    V => "v", 0x56, 47, 0x09, false;
    W => "w", 0x57, 17, 0x0D, false;
    X => "x", 0x58, 45, 0x07, false;
    Y => "y", 0x59, 21, 0x10, false;
    Z => "z", 0x5A, 44, 0x06, false;

    // --- digits ------------------------------------------------------------
    Num0 => "0", 0x30, 11, 0x1D, false;
    Num1 => "1", 0x31, 2, 0x12, false;
    Num2 => "2", 0x32, 3, 0x13, false;
    Num3 => "3", 0x33, 4, 0x14, false;
    Num4 => "4", 0x34, 5, 0x15, false;
    Num5 => "5", 0x35, 6, 0x17, false;
    Num6 => "6", 0x36, 7, 0x16, false;
    Num7 => "7", 0x37, 8, 0x1A, false;
    Num8 => "8", 0x38, 9, 0x1C, false;
    Num9 => "9", 0x39, 10, 0x19, false;

    // --- editing / punctuation --------------------------------------------
    Escape => "escape", 0x1B, 1, 0x35, false;
    Minus => "minus", 0xBD, 12, 0x1B, false;
    Equal => "equal", 0xBB, 13, 0x18, false;
    Backspace => "backspace", 0x08, 14, 0x33, false;
    Tab => "tab", 0x09, 15, 0x30, false;
    LeftBracket => "leftbracket", 0xDB, 26, 0x21, false;
    RightBracket => "rightbracket", 0xDD, 27, 0x1E, false;
    Backslash => "backslash", 0xDC, 43, 0x2A, false;
    CapsLock => "capslock", 0x14, 58, 0x39, false;
    Semicolon => "semicolon", 0xBA, 39, 0x29, false;
    Apostrophe => "apostrophe", 0xDE, 40, 0x27, false;
    Grave => "grave", 0xC0, 41, 0x32, false;
    Comma => "comma", 0xBC, 51, 0x2B, false;
    Period => "period", 0xBE, 52, 0x2F, false;
    Slash => "slash", 0xBF, 53, 0x2C, false;
    Space => "space", 0x20, 57, 0x31, false;
    Enter => "enter", 0x0D, 28, 0x24, false;
    KpEnter => "kpenter", 0x0D, 96, 0x4C, true;

    // --- modifiers ---------------------------------------------------------
    LeftShift => "leftshift", 0xA0, 42, 0x38, false;
    RightShift => "rightshift", 0xA1, 54, 0x3C, false;
    LeftCtrl => "leftctrl", 0xA2, 29, 0x3B, false;
    RightCtrl => "rightctrl", 0xA3, 97, 0x3E, true;
    LeftAlt => "leftalt", 0xA4, 56, 0x3A, false;
    RightAlt => "rightalt", 0xA5, 100, 0x3D, true;
    LeftMeta => "leftmeta", 0x5B, 125, 0x37, true;
    RightMeta => "rightmeta", 0x5C, 126, 0x36, true;
    AppsMenu => "appsmenu", 0x5D, 127, UNSUPPORTED, true;
    Fn => "fn", UNSUPPORTED, UNSUPPORTED, 0x3F, false;

    // --- function row ------------------------------------------------------
    F1 => "f1", 0x70, 59, 0x7A, false;
    F2 => "f2", 0x71, 60, 0x78, false;
    F3 => "f3", 0x72, 61, 0x63, false;
    F4 => "f4", 0x73, 62, 0x76, false;
    F5 => "f5", 0x74, 63, 0x60, false;
    F6 => "f6", 0x75, 64, 0x61, false;
    F7 => "f7", 0x76, 65, 0x62, false;
    F8 => "f8", 0x77, 66, 0x64, false;
    F9 => "f9", 0x78, 67, 0x65, false;
    F10 => "f10", 0x79, 68, 0x6D, false;
    F11 => "f11", 0x7A, 87, 0x67, false;
    F12 => "f12", 0x7B, 88, 0x6F, false;
    F13 => "f13", 0x7C, 183, 0x69, false;
    F14 => "f14", 0x7D, 184, 0x6B, false;
    F15 => "f15", 0x7E, 185, 0x71, false;
    F16 => "f16", 0x7F, 186, 0x6A, false;
    F17 => "f17", 0x80, 187, 0x40, false;
    F18 => "f18", 0x81, 188, 0x4F, false;
    F19 => "f19", 0x82, 189, 0x50, false;
    F20 => "f20", 0x83, 190, 0x5A, false;
    F21 => "f21", 0x84, 191, UNSUPPORTED, false;
    F22 => "f22", 0x85, 192, UNSUPPORTED, false;
    F23 => "f23", 0x86, 193, UNSUPPORTED, false;
    F24 => "f24", 0x87, 194, UNSUPPORTED, false;

    // --- navigation --------------------------------------------------------
    PrintScreen => "printscreen", 0x2C, 99, UNSUPPORTED, true;
    ScrollLock => "scrolllock", 0x91, 70, UNSUPPORTED, true;
    Pause => "pause", 0x13, 119, UNSUPPORTED, false;
    Insert => "insert", 0x2D, 110, UNSUPPORTED, true;
    Delete => "delete", 0x2E, 111, 0x75, true;
    Home => "home", 0x24, 102, 0x73, true;
    End => "end", 0x23, 107, 0x77, true;
    PageUp => "pageup", 0x21, 104, 0x74, true;
    PageDown => "pagedown", 0x22, 109, 0x79, true;
    ArrowUp => "up", 0x26, 103, 0x7E, true;
    ArrowDown => "down", 0x28, 108, 0x7D, true;
    ArrowLeft => "left", 0x25, 105, 0x7B, true;
    ArrowRight => "right", 0x27, 106, 0x7C, true;
    Help => "help", 0x2F, 138, 0x72, false;

    // --- keypad ------------------------------------------------------------
    NumLock => "numlock", 0x90, 69, UNSUPPORTED, true;
    KpDivide => "kpdivide", 0x6F, 98, 0x4B, true;
    KpMultiply => "kpmultiply", 0x6A, 55, 0x43, false;
    KpMinus => "kpminus", 0x6D, 74, 0x4E, false;
    KpPlus => "kpplus", 0x6B, 78, 0x45, false;
    KpDecimal => "kpdecimal", 0x6E, 83, 0x41, false;
    Kp0 => "kp0", 0x60, 82, 0x52, false;
    Kp1 => "kp1", 0x61, 79, 0x53, false;
    Kp2 => "kp2", 0x62, 80, 0x54, false;
    Kp3 => "kp3", 0x63, 81, 0x55, false;
    Kp4 => "kp4", 0x64, 75, 0x56, false;
    Kp5 => "kp5", 0x65, 76, 0x57, false;
    Kp6 => "kp6", 0x66, 77, 0x58, false;
    Kp7 => "kp7", 0x67, 71, 0x59, false;
    Kp8 => "kp8", 0x68, 72, 0x5B, false;
    Kp9 => "kp9", 0x69, 73, 0x5C, false;

    // --- media / system ----------------------------------------------------
    Mute => "mute", 0xAD, 113, UNSUPPORTED, true;
    VolumeDown => "volumedown", 0xAE, 114, UNSUPPORTED, true;
    VolumeUp => "volumeup", 0xAF, 115, UNSUPPORTED, true;
    MediaNext => "medianext", 0xB0, 167, UNSUPPORTED, true;
    MediaPrev => "mediaprev", 0xB1, 165, UNSUPPORTED, true;
    MediaStop => "mediastop", 0xB2, 166, UNSUPPORTED, true;
    MediaPlayPause => "mediaplaypause", 0xB3, 164, UNSUPPORTED, true;
    BrowserBack => "browserback", 0xA6, 158, UNSUPPORTED, true;
    BrowserForward => "browserforward", 0xA7, 159, UNSUPPORTED, true;
    BrowserRefresh => "browserrefresh", 0xA8, 173, UNSUPPORTED, true;
    BrowserStop => "browserstop", 0xA9, 128, UNSUPPORTED, true;
    BrowserSearch => "browsersearch", 0xAA, 217, UNSUPPORTED, true;
    BrowserFavorites => "browserfavorites", 0xAB, 156, UNSUPPORTED, true;
    BrowserHome => "browserhome", 0xAC, 172, UNSUPPORTED, true;
    MediaSelect => "mediaselect", 0xB5, 226, UNSUPPORTED, true;
    Mail => "mail", 0xB4, 155, UNSUPPORTED, true;
    Calculator => "calculator", 0xB6, 140, UNSUPPORTED, true;
    Sleep => "sleep", 0x5F, 142, UNSUPPORTED, true;
    Power => "power", UNSUPPORTED, 116, UNSUPPORTED, true;
    BrightnessDown => "brightnessdown", UNSUPPORTED, 224, UNSUPPORTED, true;
    BrightnessUp => "brightnessup", UNSUPPORTED, 225, UNSUPPORTED, true;
    KbdIllumDown => "kbdillumdown", UNSUPPORTED, 229, UNSUPPORTED, true;
    KbdIllumUp => "kbdillumup", UNSUPPORTED, 230, UNSUPPORTED, true;
    MicMute => "micmute", UNSUPPORTED, 248, UNSUPPORTED, true;
}

/// Alternative spellings accepted in the config file.
fn aliases(n: &str) -> Option<KeyCode> {
    let k = match n {
        "esc" => KeyCode::Escape,
        "return" | "ret" => KeyCode::Enter,
        "numenter" | "kpreturn" | "kp_enter" => KeyCode::KpEnter,
        "back" | "bspace" => KeyCode::Backspace,
        "caps" => KeyCode::CapsLock,
        "ctrl" | "control" | "ctl" => KeyCode::LeftCtrl,
        "rightcontrol" => KeyCode::RightCtrl,
        "alt" | "opt" | "option" => KeyCode::LeftAlt,
        "altgr" | "rightoption" => KeyCode::RightAlt,
        "shift" => KeyCode::LeftShift,
        "meta" | "cmd" | "command" | "win" | "windows" | "super" => KeyCode::LeftMeta,
        "rightcmd" | "rightcommand" | "rightwin" | "rightsuper" => KeyCode::RightMeta,
        "menu" | "contextmenu" | "application" => KeyCode::AppsMenu,
        "pgup" | "page_up" => KeyCode::PageUp,
        "pgdn" | "pagedn" | "page_down" => KeyCode::PageDown,
        "del" => KeyCode::Delete,
        "ins" => KeyCode::Insert,
        "prtsc" | "print" | "sysrq" => KeyCode::PrintScreen,
        "scrlk" | "scroll" => KeyCode::ScrollLock,
        "num" => KeyCode::NumLock,
        "lbracket" | "leftbrace" => KeyCode::LeftBracket,
        "rbracket" | "rightbrace" => KeyCode::RightBracket,
        "quote" => KeyCode::Apostrophe,
        "backtick" | "tilde" => KeyCode::Grave,
        "kp_plus" | "kpplus" => KeyCode::KpPlus,
        "kp_minus" => KeyCode::KpMinus,
        "kp_star" | "kpasterisk" => KeyCode::KpMultiply,
        "kp_slash" => KeyCode::KpDivide,
        "kp_dot" | "kpperiod" => KeyCode::KpDecimal,
        "playpause" | "play" => KeyCode::MediaPlayPause,
        "nexttrack" | "nextsong" => KeyCode::MediaNext,
        "prevtrack" | "previoussong" => KeyCode::MediaPrev,
        "stop" => KeyCode::MediaStop,
        "volup" => KeyCode::VolumeUp,
        "voldown" => KeyCode::VolumeDown,
        _ => return None,
    };
    Some(k)
}

/// Lowercase and drop every separator: `"Page-Up"` -> `"pageup"`.
pub fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

impl KeyCode {
    /// Parse a canonical name or an alias.
    pub fn parse(name: &str) -> Result<KeyCode, String> {
        let n = normalize(name);
        if n.is_empty() {
            return Err("empty key name".to_string());
        }
        from_normalized(&n).ok_or_else(|| format!("unknown key `{name}`"))
    }

    /// Windows virtual key -> canonical key. `extended` disambiguates the
    /// keypad Enter key (same VK as Return, `LLKHF_EXTENDED` flag set).
    pub fn from_windows_vk(vk: u16, extended: bool) -> Option<KeyCode> {
        if vk == 0x0D && extended {
            return Some(KeyCode::KpEnter);
        }
        // Generic modifier VKs are not in the table (the table uses the
        // left/right specific codes); map them defensively.
        match vk {
            0x10 => return Some(KeyCode::LeftShift),
            0x11 => return Some(KeyCode::LeftCtrl),
            0x12 => return Some(KeyCode::LeftAlt),
            _ => {}
        }
        KeyCode::ALL
            .iter()
            .copied()
            .find(|k| k.windows_vk() == vk && !(vk == 0x0D && *k == KeyCode::KpEnter))
    }

    /// Linux evdev `KEY_*` -> canonical key.
    pub fn from_linux_key(code: u16) -> Option<KeyCode> {
        KeyCode::ALL.iter().copied().find(|k| k.linux_key() == code)
    }

    /// macOS virtual keycode -> canonical key.
    pub fn from_macos_keycode(code: u16) -> Option<KeyCode> {
        KeyCode::ALL
            .iter()
            .copied()
            .find(|k| k.macos_keycode() == code)
    }

    pub fn is_modifier(self) -> bool {
        matches!(
            self,
            KeyCode::LeftShift
                | KeyCode::RightShift
                | KeyCode::LeftCtrl
                | KeyCode::RightCtrl
                | KeyCode::LeftAlt
                | KeyCode::RightAlt
                | KeyCode::LeftMeta
                | KeyCode::RightMeta
                | KeyCode::Fn
        )
    }
}

impl fmt::Display for KeyCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The modifier families that a generic name such as `ctrl` stands for.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum GenericMod {
    Ctrl,
    Shift,
    Alt,
    Meta,
}

impl GenericMod {
    pub fn name(self) -> &'static str {
        match self {
            GenericMod::Ctrl => "ctrl",
            GenericMod::Shift => "shift",
            GenericMod::Alt => "alt",
            GenericMod::Meta => "meta",
        }
    }

    /// The concrete keys this family covers.
    pub fn keys(self) -> &'static [KeyCode] {
        match self {
            GenericMod::Ctrl => &[KeyCode::LeftCtrl, KeyCode::RightCtrl],
            GenericMod::Shift => &[KeyCode::LeftShift, KeyCode::RightShift],
            GenericMod::Alt => &[KeyCode::LeftAlt, KeyCode::RightAlt],
            GenericMod::Meta => &[KeyCode::LeftMeta, KeyCode::RightMeta],
        }
    }

    pub fn covers(self, k: KeyCode) -> bool {
        self.keys().contains(&k)
    }
}

/// One element of a trigger. Generic modifiers match either side, which is what
/// users expect from `keys = ["ctrl", "h"]`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum KeyMatcher {
    Key(KeyCode),
    Mod(GenericMod),
}

impl KeyMatcher {
    pub fn parse(name: &str) -> Result<KeyMatcher, String> {
        let n = normalize(name);
        let generic = match n.as_str() {
            "ctrl" | "control" | "ctl" => Some(GenericMod::Ctrl),
            "shift" => Some(GenericMod::Shift),
            "alt" | "opt" | "option" => Some(GenericMod::Alt),
            "meta" | "cmd" | "command" | "win" | "windows" | "super" => Some(GenericMod::Meta),
            _ => None,
        };
        match generic {
            Some(m) => Ok(KeyMatcher::Mod(m)),
            None => KeyCode::parse(name).map(KeyMatcher::Key),
        }
    }

    pub fn matches(self, k: KeyCode) -> bool {
        match self {
            KeyMatcher::Key(x) => x == k,
            KeyMatcher::Mod(m) => m.covers(k),
        }
    }

    /// Stable string form used in logs and in the "already bound" check.
    pub fn label(self) -> String {
        match self {
            KeyMatcher::Key(k) => k.name().to_string(),
            KeyMatcher::Mod(m) => m.name().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_row_round_trips() {
        for k in KeyCode::ALL {
            assert_eq!(KeyCode::parse(k.name()).unwrap(), *k, "{}", k.name());
        }
        // Generic names resolve to the left variant when a concrete key is needed.
        assert_eq!(KeyCode::parse("Ctrl").unwrap(), KeyCode::LeftCtrl);
        assert_eq!(KeyCode::parse("Page-Up").unwrap(), KeyCode::PageUp);
        assert_eq!(KeyCode::parse("esc").unwrap(), KeyCode::Escape);
    }

    #[test]
    fn generic_matchers_cover_both_sides() {
        let m = KeyMatcher::parse("ctrl").unwrap();
        assert!(m.matches(KeyCode::RightCtrl));
        assert!(m.matches(KeyCode::LeftCtrl));
        assert!(!m.matches(KeyCode::LeftAlt));
        let m = KeyMatcher::parse("rightctrl").unwrap();
        assert!(!m.matches(KeyCode::LeftCtrl));
    }

    #[test]
    fn platform_lookups_are_consistent() {
        assert_eq!(
            KeyCode::from_windows_vk(0x0D, true),
            Some(KeyCode::KpEnter)
        );
        assert_eq!(KeyCode::from_windows_vk(0x0D, false), Some(KeyCode::Enter));
        assert_eq!(
            KeyCode::from_linux_key(30),
            Some(KeyCode::A),
            "evdev KEY_A is 30"
        );
        assert_eq!(KeyCode::from_macos_keycode(0x00), Some(KeyCode::A));
        // Only Enter and the keypad Enter may share the Return virtual key.
        for k in KeyCode::ALL {
            if k.windows_vk() == 0x0D {
                assert!(
                    *k == KeyCode::KpEnter || *k == KeyCode::Enter,
                    "{} must not alias VK_RETURN",
                    k.name()
                );
            }
        }
    }
}
