//! Service / autostart file generation.
//!
//! Nothing is written outside the target directory without being asked: the
//! `print` action only emits text, and `install` writes the file next to the
//! user's config (or into `--output`).

use std::path::{Path, PathBuf};

use keyforge_core::paths;

pub const SYSTEMD_UNIT: &str = "keyforge.service";
pub const LAUNCHD_PLIST: &str = "com.keyforge.daemon.plist";
pub const WINDOWS_TASK: &str = "keyforge-task.xml";

/// Path of the daemon binary, as an absolute path when possible.
fn daemon_path() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| {
            let candidate = p.with_file_name(if cfg!(windows) {
                "keyforged.exe"
            } else {
                "keyforged"
            });
            if candidate.exists() {
                Some(candidate)
            } else {
                Some(p)
            }
        })
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "keyforged".to_string())
}

pub fn config_path() -> PathBuf {
    paths::default_config_path()
}

/// Content of the autostart definition for the current platform.
pub fn render() -> (String, String) {
    let bin = daemon_path();
    let cfg = config_path().display().to_string();

    if cfg!(windows) {
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-16"?>
<!-- Save as UTF-16 and register with:
     schtasks /Create /TN keyforge /XML "<path to this file>" /F
     schtasks /Run    /TN keyforge
     schtasks /Delete /TN keyforge /F        (to remove)

     Note: keyforge must run inside the interactive user session to see
     keystrokes, so a Windows *service* is not suitable; a logon task is. -->
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>keyforge keyboard remapper</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>3</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{bin}</Command>
      <Arguments>run --config "{cfg}"</Arguments>
    </Exec>
  </Actions>
</Task>
"#
        );
        (WINDOWS_TASK.to_string(), xml)
    } else if cfg!(target_os = "macos") {
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Install: cp com.keyforge.daemon.plist ~/Library/LaunchAgents/
              launchctl load -w ~/Library/LaunchAgents/com.keyforge.daemon.plist -->
<plist version="1.0">
<dict>
    <key>Label</key><string>com.keyforge.daemon</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>run</string>
        <string>--config</string>
        <string>{cfg}</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
</dict>
</plist>
"#
        );
        (LAUNCHD_PLIST.to_string(), plist)
    } else {
        let unit = format!(
            r#"# Install: mkdir -p ~/.config/systemd/user
#          cp keyforge.service ~/.config/systemd/user/
#          systemctl --user daemon-reload
#          systemctl --user enable --now keyforge
#
# Reading /dev/input/* and writing to /dev/uinput needs privileges: see
# packaging/linux/60-keyforge.rules, or run the daemon as root for testing.
[Unit]
Description=keyforge keyboard remapper
After=graphical-session.target

[Service]
Type=simple
ExecStart={bin} run --config "{cfg}"
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
"#
        );
        (SYSTEMD_UNIT.to_string(), unit)
    }
}

/// Write the autostart file into `dir`; returns the created path.
pub fn install(dir: Option<&Path>) -> std::io::Result<PathBuf> {
    let (name, body) = render();
    let dir = dir.map(Path::to_path_buf).unwrap_or_else(paths::config_dir);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(name);
    std::fs::write(&path, body)?;
    Ok(path)
}

/// Human readable "what to do next" text.
pub fn instructions(path: &Path) -> String {
    if cfg!(windows) {
        format!(
            "Scheduled task written to {}\n\nRegister it with:\n  \
             schtasks /Create /TN keyforge /XML \"{}\" /F\n  schtasks /Run /TN keyforge\n\n\
             Remove it with:\n  schtasks /Delete /TN keyforge /F\n\n\
             Windows requires keyforge to run in your interactive session, so a logon\n\
             task (not a Windows service) is the correct autostart mechanism.",
            path.display(),
            path.display()
        )
    } else if cfg!(target_os = "macos") {
        format!(
            "LaunchAgent written to {}\n\nInstall it with:\n  \
             cp \"{}\" ~/Library/LaunchAgents/\n  \
             launchctl load -w ~/Library/LaunchAgents/{}\n",
            path.display(),
            path.display(),
            LAUNCHD_PLIST
        )
    } else {
        format!(
            "systemd user unit written to {}\n\nInstall it with:\n  \
             mkdir -p ~/.config/systemd/user\n  \
             cp \"{}\" ~/.config/systemd/user/\n  \
             systemctl --user daemon-reload\n  systemctl --user enable --now keyforge\n",
            path.display(),
            path.display()
        )
    }
}
