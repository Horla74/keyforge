//! Hand written argument parsing and the sub commands of both binaries.
//!
//! Keeping this dependency free keeps the build small and the CLI predictable.

use std::path::PathBuf;

use keyforge_core::{logging, paths, KeyCode, LogLevel, DEFAULT_CONFIG};

use crate::{platform, runtime, service};

pub const HELP: &str = r#"keyforge — cross platform keyboard remapper

USAGE
    keyforge <COMMAND> [OPTIONS]

COMMANDS
    run            run the remapper in the foreground (default for keyforged)
    stop           stop a running daemon cleanly (--force to kill it)
    check          parse and validate the configuration, then exit
    init           write an example configuration file
    keys           list every key name that can be used in the config
    devices        list input devices (Linux only for now)
    doctor         check the configuration, permissions and platform support
    service        print or install the autostart definition
                   (service print | service install [--output DIR])
    help           show this text
    version        show version information

OPTIONS
    -c, --config <PATH>     configuration file
                            (default: %APPDATA%\keyforge\config.toml on Windows,
                             ~/.config/keyforge/config.toml on Linux,
                             ~/Library/Application Support/keyforge on macOS)
        --log-level <LVL>   error | warn | info | debug | trace
        --log-file <PATH>   also append log lines to this file
    -v, --verbose           shorthand for --log-level debug
        --watch             reload the config file when it changes
                            (OFF by default: a live reload drops the state of
                             keys that are held at that moment)
        --no-app-probe      never probe the focused application
    -f, --force             overwrite an existing file (init, service install)
    -o, --output <DIR>      directory for generated files (service install)
    -h, --help              show this text
    -V, --version           show version information

EXAMPLES
    keyforge init                     write the example config
    keyforge check                    validate it
    keyforge run -v                   run with debug logging
    keyforge service print            show the logon task / unit file
"#;

#[derive(Debug, Clone)]
pub struct Args {
    pub command: String,
    pub config: Option<PathBuf>,
    pub log_level: Option<LogLevel>,
    pub log_file: Option<PathBuf>,
    pub watch: bool,
    pub app_probe: bool,
    pub force: bool,
    pub output: Option<PathBuf>,
    pub positional: Vec<String>,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            command: "run".to_string(),
            config: None,
            log_level: None,
            log_file: None,
            watch: false,
            app_probe: true,
            force: false,
            output: None,
            positional: Vec::new(),
        }
    }
}

pub fn parse(argv: &[String], default_command: Option<&str>) -> Result<Args, String> {
    let mut args = Args::default();
    if let Some(default) = default_command {
        args.command = default.to_string();
    }
    let mut command_seen = default_command.is_some();
    let mut idx = 0usize;

    while idx < argv.len() {
        let arg = argv[idx].clone();
        idx += 1;
        match arg.as_str() {
            "-h" | "--help" => args.command = "help".to_string(),
            "-V" | "--version" => args.command = "version".to_string(),
            "-c" | "--config" => {
                args.config = Some(PathBuf::from(take(argv, &mut idx, "--config")?))
            }
            "--log-level" => {
                let raw = take(argv, &mut idx, "--log-level")?;
                args.log_level = Some(parse_level(&raw)?);
            }
            "--log-file" => {
                args.log_file = Some(PathBuf::from(take(argv, &mut idx, "--log-file")?))
            }
            "-v" | "--verbose" => args.log_level = Some(LogLevel::Debug),
            "-vv" => args.log_level = Some(LogLevel::Trace),
            "-q" | "--quiet" => args.log_level = Some(LogLevel::Warn),
            "--no-watch" => args.watch = false,
            "--watch" => args.watch = true,
            "--no-app-probe" => args.app_probe = false,
            "-f" | "--force" => args.force = true,
            "-o" | "--output" => {
                args.output = Some(PathBuf::from(take(argv, &mut idx, "--output")?))
            }
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(format!("unknown option `{other}`"));
            }
            other => {
                if !command_seen {
                    args.command = other.to_string();
                    command_seen = true;
                } else {
                    args.positional.push(other.to_string());
                }
            }
        }
    }
    Ok(args)
}

fn take(argv: &[String], idx: &mut usize, name: &str) -> Result<String, String> {
    let value = argv
        .get(*idx)
        .cloned()
        .ok_or_else(|| format!("{name} needs a value"))?;
    *idx += 1;
    Ok(value)
}

fn parse_level(raw: &str) -> Result<LogLevel, String> {
    match raw.to_ascii_lowercase().as_str() {
        "error" => Ok(LogLevel::Error),
        "warn" | "warning" => Ok(LogLevel::Warn),
        "info" => Ok(LogLevel::Info),
        "debug" => Ok(LogLevel::Debug),
        "trace" => Ok(LogLevel::Trace),
        other => Err(format!(
            "unknown log level `{other}` (error|warn|info|debug|trace)"
        )),
    }
}

/// Entry point shared by `keyforge` and `keyforged`.
pub fn main(argv: Vec<String>, program: &str, default_command: Option<&str>) -> i32 {
    let args = match parse(&argv, default_command) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{program}: {e}\n\nRun `{program} --help` for usage.");
            show_daemon_error(program, &e);
            return 2;
        }
    };
    match run(&args, program) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("{program}: {message}");
            show_daemon_error(program, &message);
            1
        }
    }
}

fn show_daemon_error(program: &str, message: &str) {
    #[cfg(windows)]
    if program == "keyforged" {
        #[link(name = "user32")]
        extern "system" {
            fn MessageBoxW(
                window: *mut std::ffi::c_void,
                text: *const u16,
                title: *const u16,
                flags: u32,
            ) -> i32;
        }
        let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
        let title: Vec<u16> = "Keyforge - startup failed"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        unsafe {
            MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), 0x10);
        }
    }
    #[cfg(not(windows))]
    let _ = (program, message);
}

pub fn run(args: &Args, program: &str) -> Result<i32, String> {
    match args.command.as_str() {
        "help" => {
            print!("{HELP}");
            Ok(0)
        }
        "version" => {
            println!(
                "keyforge {} ({}, backend: {})",
                keyforge_core::VERSION,
                std::env::consts::OS,
                if platform::backend_available() {
                    "available"
                } else {
                    "not implemented on this platform yet"
                }
            );
            Ok(0)
        }
        "keys" => {
            print_keys();
            Ok(0)
        }
        "init" => cmd_init(args),
        "check" => cmd_check(args),
        "doctor" => cmd_doctor(args),
        "devices" => cmd_devices(args),
        "stop" => cmd_stop(args),
        "service" => cmd_service(args),
        "run" => cmd_run(args, cfg!(windows) && program == "keyforged"),
        other => Err(format!("unknown command `{other}`; try `{program} --help`")),
    }
}

fn cmd_init(args: &Args) -> Result<i32, String> {
    let path = paths::resolve_config_path(args.config.as_deref());
    if path.exists() && !args.force {
        return Err(format!(
            "{} already exists (use --force to overwrite)",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, DEFAULT_CONFIG)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    // Fail fast if the shipped template is not actually valid.
    runtime::load_compiled(&path).map_err(|r| format!("template is invalid:\n{}", r.render()))?;
    println!("wrote {}", path.display());
    println!("\nNext steps:");
    println!(
        "  1. edit the file, then run `keyforge check --config \"{}\"`",
        path.display()
    );
    println!(
        "  2. start it with `keyforge run --config \"{}\"`",
        path.display()
    );
    if let Some(hint) = platform::permission_hint() {
        println!("\n{hint}");
    }
    Ok(0)
}

fn cmd_check(args: &Args) -> Result<i32, String> {
    let path = paths::resolve_config_path(args.config.as_deref());
    if !path.exists() {
        return Err(format!(
            "{} does not exist; create one with `keyforge init`",
            path.display()
        ));
    }
    match runtime::load_compiled(&path) {
        Ok(compiled) => {
            println!("{}: OK", path.display());
            println!(
                "  {} profiles, {} mappings, combo_mode={:?}, tap_hold_ms={}",
                compiled.profiles.len(),
                compiled.map_count(),
                compiled.general.combo_mode,
                compiled.general.tap_hold_ms
            );
            for profile in &compiled.profiles {
                println!(
                    "  [{}] {}{}",
                    profile.name,
                    if profile.name == compiled.general.default_profile {
                        "base"
                    } else {
                        "layer"
                    },
                    profile
                        .description
                        .as_ref()
                        .map(|d| format!(" — {d}"))
                        .unwrap_or_default()
                );
                for map in &profile.maps {
                    println!(
                        "      {:<24} {}{}",
                        map.trigger_label(),
                        map.action.describe(),
                        if map.only_in.is_empty() && map.except_in.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "   [only_in={:?} except_in={:?}]",
                                map.only_in, map.except_in
                            )
                        }
                    );
                }
            }
            for note in compiled.lint() {
                println!("\nnote: {note}");
            }
            Ok(0)
        }
        Err(report) => {
            let mut text = report.render();
            if text.is_empty() {
                text.push_str("invalid configuration\n");
            }
            eprint!("{text}");
            Ok(1)
        }
    }
}

fn cmd_doctor(args: &Args) -> Result<i32, String> {
    println!(
        "keyforge {} on {}",
        keyforge_core::VERSION,
        std::env::consts::OS
    );
    println!("platform backend: {}", platform::platform_name());
    println!(
        "backend compiled in: {}",
        if platform::backend_available() {
            "yes"
        } else {
            "no (Windows only for now)"
        }
    );

    let path = paths::resolve_config_path(args.config.as_deref());
    println!("\nconfiguration: {}", path.display());
    if path.exists() {
        match runtime::load_compiled(&path) {
            Ok(compiled) => {
                println!(
                    "  OK — {} profiles, {} mappings, {} app filters",
                    compiled.profiles.len(),
                    compiled.map_count(),
                    if compiled.has_app_filters() {
                        "with"
                    } else {
                        "without"
                    }
                );
                for note in compiled.lint() {
                    println!("  note: {note}");
                }
            }
            Err(report) => {
                println!("  INVALID");
                print!("{}", report.render());
            }
        }
    } else {
        println!("  missing — create it with `keyforge init`");
    }

    println!("\nplatform checks:");
    println!(
        "  executable: {}",
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "<unknown>".to_string())
    );
    for line in platform::diagnostics() {
        println!("  {line}");
    }

    let warnings = platform::startup_warnings();
    if warnings.is_empty() {
        println!("\nno problems detected");
    } else {
        println!("\nproblems:");
        for w in warnings {
            for (i, line) in w.lines().enumerate() {
                if i == 0 {
                    println!("  ! {line}");
                } else {
                    println!("    {line}");
                }
            }
        }
    }

    if platform::integrity_below_medium() {
        println!(
            "\nthe executable is running below medium integrity, which normally means the file \
             inherited a low mandatory label from the folder it was built in\n(agent/sandbox \
             workspaces do that). Copy the binaries to a normal folder and run them from there —\n\
             in a normal PowerShell window:"
        );
        for line in platform::relocation_commands() {
            println!("    {line}");
        }
        println!(
            "then compare: the new location must report `process integrity level: Medium (0x2000)`\n\
             and `hook round trip: OK`. Copying the binaries is enough — the config stays in place."
        );
    }

    let (name, _) = service::render();
    let autostart = keyforge_core::paths::config_dir().join(name);
    println!(
        "\nautostart definition: {}",
        if autostart.exists() {
            format!("{} (written)", autostart.display())
        } else {
            format!(
                "not written yet — `keyforge service install` creates {}",
                autostart.display()
            )
        }
    );
    println!(
        "\nlog file: {}",
        keyforge_core::paths::default_log_path().display()
    );
    Ok(0)
}

fn cmd_stop(args: &Args) -> Result<i32, String> {
    let running = platform::instance_running();
    if running {
        match platform::signal_stop() {
            Ok(true) => println!("stop signal sent to the running keyforge daemon"),
            Ok(false) => println!("no keyforge daemon is listening (already gone)"),
            Err(e) => return Err(e.to_string()),
        }
    } else {
        println!("no keyforge daemon is running in this session");
    }

    // Wait for it to disappear: exiting gracefully is what releases held keys.
    if running {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if !platform::instance_running() {
                println!("stopped cleanly");
                return Ok(0);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        if !args.force {
            eprintln!(
                "the daemon did not stop within 3s.\n\
                 If it is an old or wedged process, run `keyforge stop --force`\n\
                 (a forced kill can leave a modifier stuck — tap that key once afterwards)."
            );
            return Ok(1);
        }
        println!("forcing termination");
        for image in ["keyforged.exe", "keyforge.exe"] {
            let _ = std::process::Command::new("taskkill")
                .args(["/IM", image, "/F"])
                .output();
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        if platform::instance_running() {
            eprintln!(
                "something is still holding the stop event; check Task Manager for\n\
                 keyforged.exe and end it manually, then tap any stuck modifier key once."
            );
            return Ok(1);
        }
        println!("stopped (forced)");
    }
    Ok(0)
}

fn cmd_devices(args: &Args) -> Result<i32, String> {
    let options = platform::Options {
        app_probe: args.app_probe,
        ..platform::Options::new()
    };
    match platform::list_devices(&options) {
        Ok(lines) => {
            for line in lines {
                println!("{line}");
            }
            Ok(0)
        }
        Err(e) => Err(e.to_string()),
    }
}

fn cmd_service(args: &Args) -> Result<i32, String> {
    let action = args
        .positional
        .first()
        .map(String::as_str)
        .unwrap_or("print");
    match action {
        "print" => {
            let (name, body) = service::render();
            println!("# {name}");
            print!("{body}");
            Ok(0)
        }
        "install" => {
            let path = service::install(args.output.as_deref())
                .map_err(|e| format!("cannot write the autostart file: {e}"))?;
            println!("{}", service::instructions(&path));
            Ok(0)
        }
        other => Err(format!(
            "unknown service action `{other}` (print | install)"
        )),
    }
}

fn cmd_run(args: &Args, background: bool) -> Result<i32, String> {
    let path = paths::resolve_config_path(args.config.as_deref());
    if !path.exists() {
        return Err(format!(
            "{} does not exist; create one with `keyforge init`",
            path.display()
        ));
    }

    // Read the config once for logging settings; runtime::run reads it again
    // through the validated path.
    let raw = keyforge_core::config::load_config_file(&path);
    let (level, log_file) = match &raw {
        Ok(config) => (
            args.log_level.unwrap_or(config.general.log_level),
            args.log_file.clone().or_else(|| {
                config
                    .general
                    .log_file
                    .clone()
                    .map(|p| paths::expand_user(&p))
            }),
        ),
        Err(_) => (
            args.log_level.unwrap_or(LogLevel::Info),
            args.log_file.clone(),
        ),
    };
    let log_file = log_file.or_else(|| background.then(paths::default_log_path));
    let general = raw
        .as_ref()
        .map(|config| config.general.clone())
        .unwrap_or_default();
    logging::init_configured(
        level,
        log_file.as_deref(),
        true,
        general.log_max_size_mb,
        general.log_retention_days,
        general.log_enabled,
    )
    .map_err(|e| format!("cannot open the log file: {e}"))?;

    if !platform::backend_available() {
        return Err(format!(
            "the {} backend is not implemented yet in this build (Windows only for now)",
            platform::platform_name()
        ));
    }
    if let Some(hint) = platform::permission_hint() {
        eprintln!("{hint}");
    }

    let options = runtime::RunOptions {
        config_path: path,
        watch: args.watch,
        app_probe: args.app_probe,
    };
    runtime::run(options)?;
    Ok(0)
}

fn print_keys() {
    println!("Key names for `keys`, `key`, `remap`, `tap` and `sequence`:\n");
    let names: Vec<&str> = KeyCode::ALL.iter().map(|k| k.name()).collect();
    let mut line = String::from("  ");
    for name in names {
        if line.len() + name.len() + 1 > 78 {
            println!("{line}");
            line = String::from("  ");
        }
        line.push_str(name);
        line.push(' ');
    }
    println!("{line}");
    println!(
        "\nGeneric modifier names (match either side in a trigger): ctrl, shift, alt, meta\n\
         Aliases: esc, return, caps, cmd, win, super, option, pgup, pgdn, del, ins, prtsc,\n\
         \x20        quote, backtick, kp_plus, kp_minus, kp_star, kp_slash, kp_dot, volup, voldown\n\
         Chords: \"ctrl+shift+t\" may be used for remap / tap / sequence."
    );
}
