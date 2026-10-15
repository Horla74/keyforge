//! CLI and service file tests.

use keyforge_daemon::cli::{parse, Args, HELP};
use keyforge_daemon::service;
use keyforge_core::LogLevel;

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn defaults_to_run_for_the_daemon_binary() {
    let parsed = parse(&args(&["--config", "x.toml"]), Some("run")).unwrap();
    assert_eq!(parsed.command, "run");
    assert_eq!(parsed.config.unwrap().to_str().unwrap(), "x.toml");
}

#[test]
fn first_positional_is_the_command() {
    let parsed = parse(&args(&["check", "-c", "cfg.toml", "-v"]), None).unwrap();
    assert_eq!(parsed.command, "check");
    assert_eq!(parsed.log_level, Some(LogLevel::Debug));
}

#[test]
fn service_actions_are_positional() {
    let parsed = parse(&args(&["service", "install", "--output", "out"]), None).unwrap();
    assert_eq!(parsed.command, "service");
    assert_eq!(parsed.positional, vec!["install".to_string()]);
    assert_eq!(parsed.output.unwrap().to_str().unwrap(), "out");
}

#[test]
fn unknown_options_and_missing_values_fail() {
    assert!(parse(&args(&["run", "--nope"]), None).is_err());
    assert!(parse(&args(&["run", "--config"]), None).is_err());
    assert!(parse(&args(&["--log-level", "loud"]), None).is_err());
}

#[test]
fn help_text_documents_every_command() {
    for command in ["run", "check", "init", "keys", "devices", "service", "stop", "doctor"] {
        assert!(HELP.contains(command), "help is missing `{command}`");
    }
}

#[test]
fn hot_reload_is_off_unless_asked_for() {
    // A live reload drops the state of keys held at that moment, so it is opt in.
    assert!(!parse(&args(&["run"]), None).unwrap().watch);
    assert!(!parse(&args(&["run", "--no-watch"]), None).unwrap().watch);
    assert!(parse(&args(&["run", "--watch"]), None).unwrap().watch);
}

#[test]
fn stop_is_a_command_with_a_force_flag() {
    let parsed = parse(&args(&["stop", "--force"]), None).unwrap();
    assert_eq!(parsed.command, "stop");
    assert!(parsed.force);
}

#[test]
fn service_file_is_generated_for_this_platform() {
    let (name, body) = service::render();
    assert!(!name.is_empty());
    assert!(body.contains("keyforged") || body.contains("keyforge"), "{body}");
    if cfg!(windows) {
        assert!(name.ends_with(".xml"));
        assert!(body.contains("LogonTrigger"), "Windows needs a logon task");
    }
    let _ = Args::default();
}
