use keyforge_core::{config::LogLevel, log_info, logging};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn disabled_logging_creates_nothing_and_can_be_toggled() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("keyforge-logging-{}-{unique}", std::process::id()));
    let path = dir.join("test.log");
    logging::init_configured(LogLevel::Info, Some(&path), false, 1, 1, false).unwrap();
    log_info!("disabled at startup");
    logging::maintenance().unwrap();
    assert!(!dir.exists());
    assert!(!logging::level_enabled(LogLevel::Error));

    logging::reconfigure(true, 1, 1).unwrap();
    log_info!("first enabled record");
    let contents = fs::read_to_string(&path).unwrap();
    assert!(contents.contains("first enabled record"));
    assert!(!contents.contains("disabled at startup"));

    logging::reconfigure(false, 1, 1).unwrap();
    let expired = dir.join("test.log.archive-1-0.log");
    fs::write(&expired, "old archive").unwrap();
    log_info!("disabled after reload");
    logging::maintenance().unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), contents);
    assert!(expired.exists());

    logging::reconfigure(true, 1, 1).unwrap();
    log_info!("second enabled record");
    assert!(fs::read_to_string(&path)
        .unwrap()
        .contains("second enabled record"));
    assert!(!expired.exists());
    logging::reconfigure(false, 1, 1).unwrap();
    fs::remove_file(path).unwrap();
    fs::remove_dir(dir).unwrap();
}
