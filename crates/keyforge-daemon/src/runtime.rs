//! The daemon: hosts the engine, reloads the config and executes commands.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use keyforge_core::engine::{Engine, KeyState, Outcome, Output};
use keyforge_core::{
    log_debug, log_error, log_info, log_warn, CompiledConfig, Config, KeyCode, Report,
};
use keyforge_platform::{self as platform, Host, Options as PlatformOptions};

/// Everything `keyforge run` needs.
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub config_path: PathBuf,
    /// Reload the config file when it changes.
    pub watch: bool,
    /// Probe the focused application for `only_in` / `except_in`.
    pub app_probe: bool,
}

impl RunOptions {
    pub fn new(config_path: PathBuf) -> RunOptions {
        RunOptions {
            config_path,
            watch: true,
            app_probe: true,
        }
    }
}

/// Read + compile a config file.
pub fn load_compiled(path: &std::path::Path) -> Result<CompiledConfig, Report> {
    let config = keyforge_core::config::load_config_file(path)?;
    CompiledConfig::compile(&config)
}

/// Same, but starting from a [`Config`] value.
pub fn compile(config: &Config) -> Result<CompiledConfig, Report> {
    CompiledConfig::compile(config)
}

struct DaemonHost {
    engine: Engine,
    config_path: PathBuf,
    watch: bool,
    mtime: Option<SystemTime>,
    last_cfg_check: Instant,
    app_probe: bool,
    last_app_poll: Instant,
    app_filters: bool,
    children: Vec<Child>,
    stop: Arc<AtomicBool>,
    paused: bool,
}

impl DaemonHost {
    fn new(engine: Engine, options: RunOptions, stop: Arc<AtomicBool>) -> DaemonHost {
        let mtime = std::fs::metadata(&options.config_path)
            .and_then(|m| m.modified())
            .ok();
        let app_filters = engine.config().has_app_filters();
        DaemonHost {
            engine,
            config_path: options.config_path,
            watch: options.watch,
            mtime,
            last_cfg_check: Instant::now(),
            app_probe: options.app_probe,
            last_app_poll: Instant::now(),
            app_filters,
            children: Vec::new(),
            stop,
            paused: false,
        }
    }

    /// Poll the focused application; only needed when filters are used.
    fn poll_front_app(&mut self) {
        if !self.app_probe || !self.app_filters {
            return;
        }
        if self.last_app_poll.elapsed() < Duration::from_millis(200) {
            return;
        }
        self.last_app_poll = Instant::now();
        let options = PlatformOptions {
            app_probe: true,
            ..PlatformOptions::default()
        };
        let app = platform::frontmost_app(&options);
        if app != self.engine.front_app().map(str::to_string) {
            log_debug!("focused application: {:?}", app);
            self.engine.set_front_app(app);
        }
    }

    /// Hot reload when the file changed.
    fn reload_if_changed(&mut self) -> Vec<Output> {
        if !self.watch {
            return Vec::new();
        }
        if self.last_cfg_check.elapsed() < Duration::from_millis(700) {
            return Vec::new();
        }
        self.last_cfg_check = Instant::now();

        let mtime = std::fs::metadata(&self.config_path)
            .and_then(|m| m.modified())
            .ok();
        if mtime.is_none() || mtime == self.mtime {
            return Vec::new();
        }
        self.mtime = mtime;

        self.reload_config()
    }

    fn reload(&mut self) -> Vec<Output> {
        match load_compiled(&self.config_path) {
            Ok(cfg) => {
                keyforge_core::logging::set_rotation(
                    cfg.general.log_max_size_mb,
                    cfg.general.log_retention_days,
                );
                log_info!(
                    "reloading {} ({} profiles, {} mappings); keys held right now \
                     are released — prefer restarting while typing",
                    self.config_path.display(),
                    cfg.profiles.len(),
                    cfg.map_count()
                );
                self.app_filters = cfg.has_app_filters();
                self.mtime = std::fs::metadata(&self.config_path)
                    .and_then(|m| m.modified())
                    .ok();
                self.last_app_poll = Instant::now() - Duration::from_millis(200);
                self.poll_front_app();
                // replace_config releases keys that are still held, so those
                // releases have to reach the OS as well.
                self.engine.replace_config(cfg)
            }
            Err(report) => {
                log_error!(
                    "config reload failed, keeping the previous configuration:\n{}",
                    report.render()
                );
                Vec::new()
            }
        }
    }

    fn reap_children(&mut self) {
        self.children.retain_mut(|child| match child.try_wait() {
            Ok(Some(status)) => {
                log_debug!("command finished with {status}");
                false
            }
            Ok(None) => true,
            Err(e) => {
                log_warn!("cannot poll a spawned command: {e}");
                false
            }
        });
    }

    fn describe_outputs(outputs: &[Output]) -> String {
        outputs
            .iter()
            .map(|o| match o {
                Output::KeyDown(k) => format!("+{k}"),
                Output::KeyUp(k) => format!("-{k}"),
                Output::Command(c) => format!("!{c}"),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl Host for DaemonHost {
    fn reset_input(&mut self) -> Vec<Output> {
        let outputs = self.engine.release_all();
        self.engine.set_front_app(None);
        self.last_app_poll = Instant::now() - Duration::from_millis(200);
        self.poll_front_app();
        log_info!("input state reset after session/power transition");
        outputs
    }
    fn config_path(&self) -> Option<&std::path::Path> {
        Some(&self.config_path)
    }
    fn reload_config(&mut self) -> Vec<Output> {
        self.reload()
    }

    fn toggle_pause(&mut self) -> Vec<Output> {
        self.paused = !self.paused;
        log_info!(
            "remapping {}",
            if self.paused { "paused" } else { "resumed" }
        );
        self.engine.release_all()
    }

    fn is_paused(&self) -> bool {
        self.paused
    }

    fn on_key(&mut self, key: KeyCode, state: KeyState) -> Outcome {
        if self.paused {
            return Outcome::default();
        }
        let outcome = self.engine.on_key(key, state);
        if let Some(what) = self.engine.take_last_activation() {
            log_info!(
                "mapping fired: {what}  [{}]",
                DaemonHost::describe_outputs(&outcome.outputs)
            );
        } else {
            log_debug!(
                "{key} {} -> swallow={} [{}]",
                match state {
                    KeyState::Down => "down",
                    KeyState::Up => "up",
                },
                outcome.swallow,
                DaemonHost::describe_outputs(&outcome.outputs)
            );
        }
        outcome
    }

    fn on_tick(&mut self) -> Vec<Output> {
        let mut outputs = self.reload_if_changed();
        if !self.paused {
            outputs.extend(self.engine.on_tick(Instant::now()));
        }
        self.reap_children();
        self.poll_front_app();
        outputs
    }

    fn run_command(&mut self, command: &str) {
        log_info!("running command: {command}");
        let mut process = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(command);
            c
        } else {
            let mut c = Command::new("sh");
            c.arg("-c").arg(command);
            c
        };
        process
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            process.creation_flags(CREATE_NO_WINDOW);
        }
        match process.spawn() {
            Ok(child) => self.children.push(child),
            Err(e) => log_error!("cannot run `{command}`: {e}"),
        }
    }

    fn should_stop(&mut self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn on_shutdown(&mut self) -> Vec<Output> {
        let released = self.engine.release_all();
        if !released.is_empty() {
            log_info!(
                "releasing keys that were still held: {}",
                DaemonHost::describe_outputs(&released)
            );
        }
        log_info!("keyforge is shutting down");
        released
    }

    fn next_timeout_ms(&self) -> u32 {
        let now = Instant::now();
        let mut timeout = 500u64;
        if let Some(until) = self.engine.time_until_deadline(now) {
            timeout = timeout.min(until.as_millis().max(1) as u64);
        }
        if self.watch {
            timeout = timeout.min(700);
        }
        if self.app_probe && self.app_filters {
            timeout = timeout.min(200);
        }
        timeout.clamp(1, 500) as u32
    }

    fn warn(&mut self, message: &str) {
        log_warn!("{message}");
    }
}

/// Run the daemon until Ctrl+C (or the platform fails).
pub fn run(mut options: RunOptions) -> Result<(), String> {
    if options.config_path.is_relative() {
        options.config_path = std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(&options.config_path);
    }
    let compiled = load_compiled(&options.config_path).map_err(|report| {
        format!(
            "cannot use {}:\n{}",
            options.config_path.display(),
            report.render()
        )
    })?;

    log_info!(
        "keyforge {} on {} — config {} ({} profiles, {} mappings, combo_mode={:?})",
        keyforge_core::VERSION,
        platform::platform_name(),
        options.config_path.display(),
        compiled.profiles.len(),
        compiled.map_count(),
        compiled.general.combo_mode
    );
    for profile in &compiled.profiles {
        log_debug!(
            "profile `{}`: {} mappings{}",
            profile.name,
            profile.maps.len(),
            profile
                .description
                .as_ref()
                .map(|d| format!(" ({d})"))
                .unwrap_or_default()
        );
    }
    for warning in platform::startup_warnings() {
        log_warn!("{warning}");
    }
    if options.watch {
        log_info!("config hot reload is ON: saving the file applies changes immediately");
    } else {
        log_info!(
            "config hot reload is OFF (default): use Reload config in the tray or restart keyforge \
             (`--watch` enables live reload)"
        );
    }

    let stop = Arc::new(AtomicBool::new(false));
    let mut engine = Engine::new(compiled);
    // A grabbing backend (Linux) needs pass-through keys re-injected; a hook
    // based backend (Windows) must not, or every key would be typed twice.
    engine.set_reinject_passthrough(platform::REINJECTS_PASSTHROUGH);
    let host = DaemonHost::new(engine, options, stop);
    let platform_options = PlatformOptions {
        app_probe: true,
        ..PlatformOptions::default()
    };

    let result = platform::run(Box::new(host), platform_options).map_err(|e| e.to_string());
    log_info!("keyforge stopped");
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> DaemonHost {
        let config = keyforge_core::config::parse_toml(
            r#"
[profiles.default]
[[profiles.default.maps]]
key = "f13"
remap = "f14"
"#,
        )
        .unwrap();
        let mut options = RunOptions::new(PathBuf::from("unused.toml"));
        options.watch = false;
        DaemonHost::new(
            Engine::new(compile(&config).unwrap()),
            options,
            Arc::new(AtomicBool::new(false)),
        )
    }

    #[test]
    fn desktop_recovery_releases_keys_and_preserves_pause() {
        let mut host = host();
        host.on_key(KeyCode::F13, KeyState::Down);
        // The corresponding physical key-up was lost on the lock desktop.
        assert_eq!(host.reset_input(), vec![Output::KeyUp(KeyCode::F14)]);
        assert_eq!(
            host.on_key(KeyCode::F13, KeyState::Down).outputs,
            vec![Output::KeyDown(KeyCode::F14)]
        );
        host.toggle_pause();
        assert!(host.reset_input().is_empty());
        assert!(host.is_paused());
        assert!(!host.on_key(KeyCode::F13, KeyState::Down).swallow);
    }

    #[test]
    fn pause_releases_injected_keys_and_passes_input_until_resumed() {
        let mut host = host();
        assert_eq!(
            host.on_key(KeyCode::F13, KeyState::Down).outputs,
            vec![Output::KeyDown(KeyCode::F14)]
        );
        assert_eq!(host.toggle_pause(), vec![Output::KeyUp(KeyCode::F14)]);
        assert!(host.is_paused());
        for state in [KeyState::Up, KeyState::Down, KeyState::Up] {
            let result = host.on_key(KeyCode::F13, state);
            assert!(!result.swallow);
            assert!(result.outputs.is_empty());
        }
        assert!(host.on_tick().is_empty());
        assert!(host.toggle_pause().is_empty());
        assert!(!host.is_paused());
        assert_eq!(
            host.on_key(KeyCode::F13, KeyState::Down).outputs,
            vec![Output::KeyDown(KeyCode::F14)]
        );
        assert_eq!(host.on_shutdown(), vec![Output::KeyUp(KeyCode::F14)]);
    }

    #[test]
    fn manual_reload_works_without_watch_and_rejects_invalid_config() {
        let mut host = host();
        let dir = std::env::temp_dir().join(format!("keyforge-reload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        host.config_path = dir.join("config.toml");
        std::fs::write(
            &host.config_path,
            r#"
[profiles.default]
[[profiles.default.maps]]
key = "f13"
remap = "f15"
only_in = ["editor"]
"#,
        )
        .unwrap();
        host.app_probe = false;
        host.on_key(KeyCode::F13, KeyState::Down);
        assert_eq!(host.reload_config(), vec![Output::KeyUp(KeyCode::F14)]);
        assert!(!host.app_probe, "reload must respect --no-app-probe");
        host.engine.set_front_app(Some("editor.exe".into()));
        host.on_key(KeyCode::F13, KeyState::Up);
        assert_eq!(
            host.on_key(KeyCode::F13, KeyState::Down).outputs,
            vec![Output::KeyDown(KeyCode::F15)]
        );
        std::fs::write(&host.config_path, "invalid = [").unwrap();
        assert!(host.reload_config().is_empty());
        // A failed reload keeps both the configuration and held-key state.
        assert_eq!(
            host.on_key(KeyCode::F13, KeyState::Up).outputs,
            vec![Output::KeyUp(KeyCode::F15)]
        );
        assert_eq!(
            host.on_key(KeyCode::F13, KeyState::Down).outputs,
            vec![Output::KeyDown(KeyCode::F15)]
        );
        host.toggle_pause();
        std::fs::write(&host.config_path, "[profiles.default]\n").unwrap();
        assert!(host.reload_config().is_empty());
        assert!(
            host.is_paused(),
            "reloading must not resume a paused daemon"
        );
        std::fs::remove_file(&host.config_path).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }
}
