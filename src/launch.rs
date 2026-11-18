// SPDX-License-Identifier: MPL-2.0

//! Opens the process monitor on the host (D5): the setting `process_monitor`, by default COSMIC's
//! System Monitor. The setting is a desktop id or a command line ([`Launcher::parse`]).
//!
//! A **desktop id** is launched by a fixed `sh` script, in a Flatpak through
//! `flatpak-spawn --host`, the way a launcher would:
//!
//! 1. `gio launch` with the entry's file, found in `$XDG_DATA_HOME` and `$XDG_DATA_DIRS`. That
//!    honours the entry exactly (its `Exec`, D-Bus activation, a user's override, a Flatpak
//!    install's export) and `gio` comes with `GLib`, which every COSMIC host has.
//! 2. Else `gtk-launch` with the desktop id, which does the same but ships with GTK 3's tools.
//! 3. Else, for COSMIC's System Monitor only, its command, `cosmic-monitor`, directly.
//!
//! The script's text is a constant and the desktop id and command reach it as positional
//! arguments, so nothing is interpolated into a shell.
//!
//! A **command line** is split into words by the shell's quoting rules (`shlex`) and run without
//! a shell: directly, or in a Flatpak as `flatpak-spawn --host <words…>`.

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

use crate::panel::DEFAULT_PROCESS_MONITOR;

/// The default process monitor's command, for hosts without `gio` or `gtk-launch`.
const DEFAULT_EXEC: &str = "cosmic-monitor";

/// Launches the desktop entry `$1`, or else runs the command `$2` if there is one, then exits at
/// once.
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
[ -n "$2" ] || exit 1
"$2" >/dev/null 2>&1 &
"#;

/// What the setting `process_monitor` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launcher {
    /// A desktop id, without `.desktop`.
    DesktopId(String),
    /// A command line, split into words.
    Command(Vec<String>),
}

impl Launcher {
    /// What `setting` names. Empty, it is the default, COSMIC's System Monitor.
    ///
    /// - With `.desktop` at the end, or in reverse-DNS form (`org.gnome.SystemMonitor`: two or
    ///   more dot-separated parts, each starting with a letter or `_`), a single word is a
    ///   desktop id. Ids use only ASCII letters, digits, `.`, `_` and `-`.
    /// - Anything else is a command line: `missioncenter`, `kitty -e btop`,
    ///   `foot -e sh -c 'htop; read'`.
    ///
    /// `None` for a command line with unbalanced quotes, or whose first word is empty or starts
    /// with `-`, which `flatpak-spawn` would take for one of its own options.
    #[must_use]
    pub fn parse(setting: &str) -> Option<Self> {
        let setting = setting.trim();
        if setting.is_empty() {
            return Some(Self::DesktopId(DEFAULT_PROCESS_MONITOR.to_owned()));
        }
        if let Some(id) = desktop_id(setting) {
            return Some(Self::DesktopId(id.to_owned()));
        }
        let words = shlex::split(setting)?;
        let program = words.first()?;
        (!program.is_empty() && !program.starts_with('-')).then_some(Self::Command(words))
    }
}

/// `word` as a desktop id, without `.desktop`, if it is one ([`Launcher::parse`]).
fn desktop_id(word: &str) -> Option<&str> {
    let (id, suffixed) = word
        .strip_suffix(".desktop")
        .map_or((word, false), |id| (id, true));
    let starts_well = |part: &str| {
        part.chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
    };
    let id_chars = id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    let reverse_dns = id.split('.').count() >= 2 && id.split('.').all(starts_well);
    let first_ok = id
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_');
    (id_chars && first_ok && (suffixed || reverse_dns)).then_some(id)
}

/// Opens the process monitor that `setting` names without waiting for it. The launch runs on its
/// own thread, which only waits for the launcher (or the command) to exit, so the UI never
/// blocks.
pub fn process_monitor(setting: &str) {
    let Some(launcher) = Launcher::parse(setting) else {
        eprintln!("can't open the process monitor: {setting:?} is no desktop id or command");
        return;
    };
    let mut command = command(&launcher, std::path::Path::new("/.flatpak-info").exists());
    let spawned = std::thread::Builder::new()
        .name("sysmon-launch".into())
        .spawn(move || match command.spawn() {
            // Reaps the launcher, or the command once it exits.
            Ok(mut child) => match child.wait() {
                Ok(status) if !status.success() => {
                    eprintln!("the process monitor's launcher failed: {status}");
                }
                _ => {}
            },
            Err(why) => eprintln!("can't open the process monitor: {why}"),
        });
    if let Err(why) = spawned {
        eprintln!("can't open the process monitor: {why}");
    }
}

/// The command that opens `launcher`, through `flatpak-spawn --host` when `in_flatpak`.
fn command(launcher: &Launcher, in_flatpak: bool) -> Command {
    let mut argv: Vec<&str> = match launcher {
        Launcher::DesktopId(id) => {
            let fallback = if id == DEFAULT_PROCESS_MONITOR {
                DEFAULT_EXEC
            } else {
                ""
            };
            vec!["sh", "-c", SCRIPT, "sh", id, fallback]
        }
        Launcher::Command(words) => words.iter().map(String::as_str).collect(),
    };
    if in_flatpak {
        // No `--watch-bus`: the monitor outlives the applet.
        argv.splice(0..0, ["flatpak-spawn", "--host"]);
    }
    let mut command = Command::new(argv[0]);
    command
        .args(&argv[1..])
        // The applet's own runtime setting; the monitor runs outside a Flatpak with our
        // environment.
        .env_remove(crate::TOKIO_WORKER_THREADS)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Its own process group, so a signal to the applet's group doesn't reach the monitor.
        .process_group(0);
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

    /// What `setting` names, which must be something.
    fn parsed(setting: &str) -> Launcher {
        Launcher::parse(setting).unwrap()
    }

    fn id(id: &str) -> Launcher {
        Launcher::DesktopId(id.into())
    }

    fn words(words: &[&str]) -> Launcher {
        Launcher::Command(words.iter().map(ToString::to_string).collect())
    }

    #[test]
    fn reverse_dns_names_and_desktop_files_are_desktop_ids() {
        assert_eq!(
            parsed("com.system76.CosmicMonitor"),
            id("com.system76.CosmicMonitor")
        );
        assert_eq!(
            parsed(" io.missioncenter.MissionCenter.desktop "),
            id("io.missioncenter.MissionCenter")
        );
        assert_eq!(
            parsed("gnome-system-monitor.desktop"),
            id("gnome-system-monitor")
        );
        assert_eq!(
            parsed("org.kde.plasma-systemmonitor"),
            id("org.kde.plasma-systemmonitor")
        );
    }

    #[test]
    fn an_empty_setting_is_the_default() {
        assert_eq!(parsed(""), id(DEFAULT_PROCESS_MONITOR));
        assert_eq!(parsed("  "), id(DEFAULT_PROCESS_MONITOR));
    }

    #[test]
    fn anything_else_is_a_command_line() {
        assert_eq!(parsed("missioncenter"), words(&["missioncenter"]));
        assert_eq!(parsed("kitty -e btop"), words(&["kitty", "-e", "btop"]));
        assert_eq!(
            parsed("foot -e sh -c 'htop; read'"),
            words(&["foot", "-e", "sh", "-c", "htop; read"])
        );
        assert_eq!(
            parsed(r#"/opt/My\ Monitor/run "a b" $HOME"#),
            words(&["/opt/My Monitor/run", "a b", "$HOME"])
        );
        // Not reverse-DNS: a part starts with a digit, or a word is a path.
        assert_eq!(parsed("python3.12"), words(&["python3.12"]));
        assert_eq!(parsed("./monitor.sh"), words(&["./monitor.sh"]));
    }

    #[test]
    fn broken_quotes_and_options_name_nothing() {
        assert_eq!(Launcher::parse("kitty -e 'btop"), None);
        assert_eq!(Launcher::parse("--help"), None);
        assert_eq!(Launcher::parse("-x.desktop"), None);
        assert_eq!(Launcher::parse("'' btop"), None);
    }

    #[test]
    fn a_desktop_id_runs_the_fixed_script_on_the_host_in_a_flatpak() {
        let args = args(&command(
            &Launcher::DesktopId(DEFAULT_PROCESS_MONITOR.into()),
            true,
        ));
        assert_eq!(args[..4], ["flatpak-spawn", "--host", "sh", "-c"]);
        assert_eq!(args[4], SCRIPT);
        assert_eq!(args[5..], ["sh", DEFAULT_PROCESS_MONITOR, DEFAULT_EXEC]);
    }

    #[test]
    fn a_desktop_id_runs_the_same_script_directly_outside_one() {
        let args = args(&command(
            &Launcher::DesktopId("org.gnome.SystemMonitor".into()),
            false,
        ));
        assert_eq!(args[..2], ["sh", "-c"]);
        assert_eq!(args[2], SCRIPT);
        // Only COSMIC's System Monitor has a known command to fall back on.
        assert_eq!(args[3..], ["sh", "org.gnome.SystemMonitor", ""]);
    }

    #[test]
    fn a_command_line_runs_its_words_without_a_shell() {
        let launcher = Launcher::parse("kitty -e 'btop --utf-force'").unwrap();
        assert_eq!(
            args(&command(&launcher, false)),
            ["kitty", "-e", "btop --utf-force"]
        );
        assert_eq!(
            args(&command(&launcher, true)),
            ["flatpak-spawn", "--host", "kitty", "-e", "btop --utf-force"]
        );
    }

    #[test]
    fn keeps_the_applet_s_worker_count_to_itself() {
        let launchers = [
            Launcher::DesktopId(DEFAULT_PROCESS_MONITOR.into()),
            Launcher::Command(vec!["missioncenter".into()]),
        ];
        for launcher in &launchers {
            for in_flatpak in [true, false] {
                let command = command(launcher, in_flatpak);
                let removed = command
                    .get_envs()
                    .any(|(name, value)| name == crate::TOKIO_WORKER_THREADS && value.is_none());
                assert!(removed, "{launcher:?}, in_flatpak: {in_flatpak}");
            }
        }
    }

    /// A scratch directory for one test, removed when dropped.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("sysmon-{name}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        /// An executable script `name` with `body`.
        fn script(&self, name: &str, body: &str) -> std::path::PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_command_line_reaches_the_program_word_for_word() {
        // A "monitor" that records each argument on its own line.
        let dir = Scratch::new("command");
        let out = dir.0.join("out");
        let monitor = dir.script(
            "monitor",
            &format!("printf '%s\\n' \"$@\" > {}", out.display()),
        );
        let setting = format!("{} 'a b' '$HOME;' \"c\"", monitor.display());
        let launcher = Launcher::parse(&setting).unwrap();
        let status = command(&launcher, false).status().unwrap();
        assert!(status.success());
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "a b\n$HOME;\nc\n");
    }

    #[test]
    fn the_script_finds_the_entry_in_the_data_dirs() {
        // A fake data dir and a `gio` that records its arguments instead of launching.
        let dir = Scratch::new("launch");
        let apps = dir.0.join("share/applications");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(dir.0.join("bin")).unwrap();
        std::fs::write(apps.join("x.test.App.desktop"), "").unwrap();
        let out = dir.0.join("out");
        dir.script("bin/gio", &format!("echo \"$@\" > {}", out.display()));
        let status = Command::new("sh")
            .args(["-c", SCRIPT, "sh", "x.test.App", ""])
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", dir.0.join("bin").display()),
            )
            .env("XDG_DATA_HOME", dir.0.join("nowhere"))
            .env(
                "XDG_DATA_DIRS",
                format!("/nonexistent:{}", dir.0.join("share").display()),
            )
            .status()
            .unwrap();
        let recorded = std::fs::read_to_string(&out).unwrap();
        assert!(status.success());
        assert_eq!(
            recorded.trim(),
            format!("launch {}", apps.join("x.test.App.desktop").display())
        );
    }

    #[test]
    fn the_script_runs_the_command_without_a_launcher() {
        // No `gio` or `gtk-launch` on the PATH, and a "monitor" that leaves a mark.
        let dir = Scratch::new("fallback");
        let mark = dir.0.join("ran");
        let monitor = dir.script("monitor", &format!(": > {}", mark.display()));
        let status = Command::new("/bin/sh")
            .args(["-c", SCRIPT, "sh", "x.test.Missing"])
            .arg(&monitor)
            .env("PATH", &dir.0)
            .env("XDG_DATA_DIRS", &dir.0)
            .status()
            .unwrap();
        // The command runs in the background.
        let ran = (0..50).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            mark.exists()
        });
        assert!(status.success());
        assert!(ran);
    }

    #[test]
    fn the_script_fails_without_a_launcher_or_a_command() {
        let dir = Scratch::new("nothing");
        let status = Command::new("/bin/sh")
            .args(["-c", SCRIPT, "sh", "x.test.Missing", ""])
            .env("PATH", &dir.0)
            .env("XDG_DATA_DIRS", &dir.0)
            .status()
            .unwrap();
        assert!(!status.success());
    }

    /// Opens the real process monitor, as the panel's right click does outside a Flatpak, and
    /// closes it again. Run by hand: `cargo test -- --ignored opens_the_process_monitor`.
    #[test]
    #[ignore = "opens COSMIC's System Monitor on the desktop"]
    fn opens_the_process_monitor() {
        let running = || {
            Command::new("pgrep")
                .args(["-x", DEFAULT_EXEC])
                .output()
                .map_or_else(
                    |_| String::new(),
                    |o| String::from_utf8_lossy(&o.stdout).trim().to_owned(),
                )
        };
        assert_eq!(running(), "", "the process monitor is already open");
        process_monitor(DEFAULT_PROCESS_MONITOR);
        let mut pids = String::new();
        for _ in 0..50 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            pids = running();
            if !pids.is_empty() {
                break;
            }
        }
        assert!(!pids.is_empty(), "the process monitor didn't start");
        let _ = Command::new("pkill").args(["-x", DEFAULT_EXEC]).status();
    }
}
