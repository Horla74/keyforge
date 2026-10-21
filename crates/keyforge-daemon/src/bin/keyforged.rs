//! `keyforged` — the daemon binary.
//!
//! It behaves like `keyforge run …`; it exists so that autostart entries and
//! process supervisors can start the daemon directly.

#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let code = keyforge_daemon::cli::main(argv, "keyforged", Some("run"));
    std::process::exit(code);
}
