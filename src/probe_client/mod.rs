// SPDX-License-Identifier: MPL-2.0

//! The applet's side of the **probe** (plan §3.2): the [`Probe`] trait and its two adapters.
//!
//! - [`Spawned`]: a probe process speaking the JSON-lines protocol of `sysmon-proto` on its stdin
//!   and stdout. In a Flatpak, [`connect`] makes it run
//!   `flatpak-spawn --host --watch-bus <app-path>/bin/sysmon-probe`.
//! - [`InProcess`]: outside a Flatpak, the same `sysmon_probe::Host` called directly.
//!
//! Calls block, so they belong on the sampler thread, never in `update()`. A spawned probe is
//! started on the first call and kept until the applet exits (its stdin closes, and
//! `--watch-bus` kills it if the applet dies). If it can't start or fails the handshake (another
//! protocol version, a host glibc older than 2.34, NixOS…), that is logged once and every later
//! call returns `None` at once: the applet goes on with what the sandbox can see and never
//! retries in a loop (ADR 0002). Missing [`REPLY_TIMEOUT`] or exiting after a good handshake is
//! transient: [`Spawned`] starts a new probe after a backoff, and gives up after a few in a row.

mod spawned;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub use self::spawned::Spawned;
use sysmon_proto::{MAX_TOP, Mount, Reply, Request, TopProcesses};

/// How long the probe has to start and say hello: `flatpak-spawn` is a D-Bus round trip.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the probe has to answer a request. A process scan takes milliseconds, plus a 250 ms
/// baseline when the popup opens.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(3);

/// Something that answers probe [`Request`]s.
pub trait Probe: Send + std::fmt::Debug {
    /// The reply to `request`; `None` if the probe is unavailable. Blocks until the reply, at
    /// most about [`REPLY_TIMEOUT`] (plus [`HANDSHAKE_TIMEOUT`] on the first call).
    fn ask(&mut self, request: Request) -> Option<Reply>;

    /// The top [`MAX_TOP`] processes by CPU and by memory.
    fn processes(&mut self) -> Option<TopProcesses> {
        match self.ask(Request::Processes { top: MAX_TOP })? {
            Reply::Processes(top) => {
                debug_once(&LOGGED_PROCESSES, || format!("{top:?}"));
                Some(top)
            }
            _ => None,
        }
    }

    /// The host's real mounts, with their space.
    fn mounts(&mut self) -> Option<Vec<Mount>> {
        match self.ask(Request::Mounts)? {
            Reply::Mounts { mounts } => {
                debug_once(&LOGGED_MOUNTS, || format!("{mounts:?}"));
                Some(mounts)
            }
            _ => None,
        }
    }
}

/// The probe for this environment: [`Spawned`] on the host from a Flatpak, otherwise
/// [`InProcess`]. Nothing starts until the first call.
#[must_use]
pub fn connect() -> Box<dyn Probe> {
    match std::fs::read_to_string("/.flatpak-info") {
        Ok(info) => {
            let Some(app_path) = flatpak_app_path(&info) else {
                eprintln!("probe: no app-path in /.flatpak-info; no host processes or mounts");
                return Box::new(Unavailable);
            };
            Box::new(Spawned::new(host_command(&app_path), HANDSHAKE_TIMEOUT))
        }
        Err(_) => Box::new(InProcess::default()),
    }
}

/// `flatpak-spawn --host --watch-bus <app_path>/bin/sysmon-probe`.
fn host_command(app_path: &Path) -> Command {
    let mut command = Command::new("flatpak-spawn");
    command
        .arg("--host")
        .arg("--watch-bus")
        .arg(app_path.join("bin/sysmon-probe"));
    command
}

/// The `app-path` key of the `[Instance]` group of `/.flatpak-info`: where the app's `/app` is
/// on the host.
#[must_use]
pub fn flatpak_app_path(info: &str) -> Option<PathBuf> {
    let mut in_instance = false;
    for line in info.lines().map(str::trim) {
        if line.starts_with('[') {
            in_instance = line == "[Instance]";
        } else if in_instance
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "app-path"
            && !value.trim().is_empty()
        {
            return Some(PathBuf::from(value.trim()));
        }
    }
    None
}

/// Outside a Flatpak: the probe's logic, called on this thread.
#[derive(Debug, Default)]
pub struct InProcess(sysmon_probe::Host);

impl Probe for InProcess {
    fn ask(&mut self, request: Request) -> Option<Reply> {
        Some(self.0.answer(request))
    }
}

/// A probe that can't run here.
#[derive(Debug)]
pub struct Unavailable;

impl Probe for Unavailable {
    fn ask(&mut self, _request: Request) -> Option<Reply> {
        None
    }
}

static LOGGED_PROCESSES: AtomicBool = AtomicBool::new(false);
static LOGGED_MOUNTS: AtomicBool = AtomicBool::new(false);

/// Whether `SYSMON_DEBUG` is set: the first reply of each kind is logged, and the sampler asks
/// for the processes once at start, so a closed popup can be checked too.
#[must_use]
pub fn debug() -> bool {
    std::env::var_os("SYSMON_DEBUG").is_some()
}

/// Logs the first reply of a kind when [`debug`].
fn debug_once(logged: &AtomicBool, message: impl FnOnce() -> String) {
    if debug() && !logged.swap(true, Ordering::Relaxed) {
        eprintln!("probe: {}", message());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_path_comes_from_the_instance_group() {
        let info = "[Application]\nname=io.github.x\nruntime=runtime/org.freedesktop.Platform/x86_64/25.08\n\n\
                    [Instance]\ninstance-id=123\napp-path=/home/u/.local/share/flatpak/app/io.github.x/x86_64/master/abc/files\n\
                    runtime-path=/usr/lib/flatpak/runtime/files\n";
        assert_eq!(
            flatpak_app_path(info),
            Some(PathBuf::from(
                "/home/u/.local/share/flatpak/app/io.github.x/x86_64/master/abc/files"
            ))
        );
        assert_eq!(flatpak_app_path("[Application]\napp-path=/wrong\n"), None);
        assert_eq!(flatpak_app_path(""), None);
    }

    #[test]
    fn the_host_command_runs_the_installed_probe_and_dies_with_the_applet() {
        let command = host_command(Path::new("/x/files"));
        assert_eq!(command.get_program(), "flatpak-spawn");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["--host", "--watch-bus", "/x/files/bin/sysmon-probe"]);
    }

    #[test]
    fn in_process_answers_with_this_machine_s_data() {
        let mut probe = InProcess::default();
        assert!(probe.mounts().is_some());
        let top = probe.processes().unwrap();
        assert_eq!(top.by_memory.len(), usize::from(MAX_TOP));
    }
}
