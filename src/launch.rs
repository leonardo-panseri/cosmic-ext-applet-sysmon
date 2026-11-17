// SPDX-License-Identifier: MPL-2.0

//! Opens the process monitor, COSMIC's System Monitor, on the host (D5).
//!
//! It runs a fixed `sh` script there, in a Flatpak through `flatpak-spawn --host`, that launches
//! the desktop entry `com.system76.CosmicMonitor` the way a launcher would:
//!
//! 1. `gio launch` with the entry's file, found in `$XDG_DATA_HOME` and `$XDG_DATA_DIRS`. That
//!    honours the entry exactly (its `Exec`, D-Bus activation, a user's override, a Flatpak
//!    install's export) and `gio` comes with `GLib`, which every COSMIC host has.
//! 2. Else `gtk-launch` with the desktop id, which does the same but ships with GTK 3's tools.
//! 3. Else the entry's command, `cosmic-monitor`, directly.
//!
//! The entry is not D-Bus activatable, and libcosmic's desktop helpers need its `desktop` feature
//! and its dependencies; this needs neither. The script's text is a constant and the desktop id
//! and command reach it as positional arguments, so nothing is interpolated into a shell.

use std::process::{Command, Stdio};

/// The process monitor's desktop id.
pub const PROCESS_MONITOR: &str = "com.system76.CosmicMonitor";
/// Its command, for hosts without `gio` or `gtk-launch`.
const PROCESS_MONITOR_EXEC: &str = "cosmic-monitor";

/// Launches the desktop entry `$1`, or runs the command `$2`, then exits at once.
const SCRIPT: &str = r#"id=$1
IFS=:
for dir in "${XDG_DATA_HOME:-$HOME/.local/share}" ${XDG_DATA_DIRS:-/usr/local/share:/usr/share}; do
    file=$dir/applications/$id.desktop
    if [ -f "$file" ] && command -v gio >/dev/null 2>&1; then
        exec gio launch "$file"
    fi
done
if command -v gtk-launch >/dev/null 2>&1; then
    exec gtk-launch "$id"
fi
"$2" >/dev/null 2>&1 &
"#;

/// Opens the process monitor without waiting for it. The launch runs on its own thread, which
/// only waits for the launcher to exit, so the UI never blocks.
pub fn process_monitor() {
    let mut command = command(std::path::Path::new("/.flatpak-info").exists());
    let spawned = std::thread::Builder::new()
        .name("sysmon-launch".into())
        .spawn(move || match command.spawn() {
            // Reaps the launcher; the monitor itself is not our child.
            Ok(mut child) => {
                let _ = child.wait();
            }
            Err(why) => eprintln!("can't open the process monitor: {why}"),
        });
    if let Err(why) = spawned {
        eprintln!("can't open the process monitor: {why}");
    }
}

/// The command that launches the process monitor, through `flatpak-spawn --host` when
/// `in_flatpak`.
fn command(in_flatpak: bool) -> Command {
    let mut command = if in_flatpak {
        let mut command = Command::new("flatpak-spawn");
        // No `--watch-bus`: the monitor outlives the applet.
        command.args(["--host", "sh"]);
        command
    } else {
        Command::new("sh")
    };
    command
        .args(["-c", SCRIPT, "sh", PROCESS_MONITOR, PROCESS_MONITOR_EXEC])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn args(command: &Command) -> Vec<String> {
        std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn runs_the_fixed_script_on_the_host_in_a_flatpak() {
        let args = args(&command(true));
        assert_eq!(args[..4], ["flatpak-spawn", "--host", "sh", "-c"]);
        assert_eq!(args[4], SCRIPT);
        assert_eq!(args[5..], ["sh", PROCESS_MONITOR, PROCESS_MONITOR_EXEC]);
    }

    #[test]
    fn runs_the_same_script_directly_outside_one() {
        let args = args(&command(false));
        assert_eq!(args[..2], ["sh", "-c"]);
        assert_eq!(args[2], SCRIPT);
        assert_eq!(args[3..], ["sh", PROCESS_MONITOR, PROCESS_MONITOR_EXEC]);
    }

    #[test]
    fn the_script_finds_the_entry_in_the_data_dirs() {
        // A fake data dir and a `gio` that records its arguments instead of launching.
        let dir = std::env::temp_dir().join(format!("sysmon-launch-{}", std::process::id()));
        let apps = dir.join("share/applications");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(apps.join("x.test.App.desktop"), "").unwrap();
        let out = dir.join("out");
        let gio = bin.join("gio");
        std::fs::write(
            &gio,
            format!("#!/bin/sh\necho \"$@\" > {}\n", out.display()),
        )
        .unwrap();
        std::fs::set_permissions(&gio, std::fs::Permissions::from_mode(0o755)).unwrap();
        let status = Command::new("sh")
            .args(["-c", SCRIPT, "sh", "x.test.App", "false"])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("XDG_DATA_HOME", dir.join("nowhere"))
            .env(
                "XDG_DATA_DIRS",
                format!("/nonexistent:{}", dir.join("share").display()),
            )
            .status()
            .unwrap();
        let recorded = std::fs::read_to_string(&out).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(status.success());
        assert_eq!(
            recorded.trim(),
            format!(
                "launch {}",
                dir.join("share/applications/x.test.App.desktop").display()
            )
        );
    }

    #[test]
    fn the_script_runs_the_command_without_a_launcher() {
        // No `gio` or `gtk-launch` on the PATH, and a "monitor" that leaves a mark.
        let dir = std::env::temp_dir().join(format!("sysmon-fallback-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mark = dir.join("ran");
        let monitor = dir.join("monitor");
        std::fs::write(&monitor, format!("#!/bin/sh\n: > {}\n", mark.display())).unwrap();
        std::fs::set_permissions(&monitor, std::fs::Permissions::from_mode(0o755)).unwrap();
        let status = Command::new("/bin/sh")
            .args(["-c", SCRIPT, "sh", "x.test.Missing"])
            .arg(&monitor)
            .env("PATH", &dir)
            .env("XDG_DATA_DIRS", &dir)
            .status()
            .unwrap();
        // The command runs in the background.
        let ran = (0..50).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            mark.exists()
        });
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(status.success());
        assert!(ran);
    }

    /// Opens the real process monitor, as the panel's right click does outside a Flatpak, and
    /// closes it again. Run by hand: `cargo test -- --ignored opens_the_process_monitor`.
    #[test]
    #[ignore = "opens COSMIC's System Monitor on the desktop"]
    fn opens_the_process_monitor() {
        let running = || {
            Command::new("pgrep")
                .args(["-x", PROCESS_MONITOR_EXEC])
                .output()
                .map_or_else(
                    |_| String::new(),
                    |o| String::from_utf8_lossy(&o.stdout).trim().to_owned(),
                )
        };
        assert_eq!(running(), "", "the process monitor is already open");
        process_monitor();
        let mut pids = String::new();
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            pids = running();
            if !pids.is_empty() {
                break;
            }
        }
        assert!(!pids.is_empty(), "the process monitor didn't start");
        let _ = Command::new("pkill")
            .args(["-x", PROCESS_MONITOR_EXEC])
            .status();
    }
}
