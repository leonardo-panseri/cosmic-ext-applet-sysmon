// SPDX-License-Identifier: MPL-2.0

//! A probe in a child process, over JSON lines on its stdin and stdout.

use std::io::{BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use sysmon_proto::{Reply, Request};

use super::{Probe, REPLY_TIMEOUT};

/// The wait before restarting a probe after its first transient failure; it doubles with each
/// failure in a row, up to [`MAX_BACKOFF`].
pub const FIRST_BACKOFF: Duration = Duration::from_secs(30);
/// The longest wait before a restart.
pub const MAX_BACKOFF: Duration = Duration::from_secs(600);
/// Transient failures in a row after which the probe is given up for good.
pub const MAX_TRANSIENT_FAILURES: u32 = 5;

/// A probe process: started on the first [`Probe::ask`], then kept.
///
/// Failures come in two kinds:
/// - **Permanent** (ADR 0002): it can't start, or fails the handshake (the host's loader or glibc
///   refuses it, another protocol version), or breaks the protocol. Logged once; every later
///   call returns `None` at once.
/// - **Transient**: after a good handshake, it misses [`REPLY_TIMEOUT`] (a scan under heavy load
///   or swap) or exits. The child is killed and reaped, never reused: the protocol has no request
///   ids, so its late reply could be read as the answer to the next request. A new one is started
///   on the first call after [`backoff`]; [`MAX_TRANSIENT_FAILURES`] in a row are permanent, and
///   a good reply resets the count.
#[derive(Debug)]
pub struct Spawned {
    command: Command,
    state: State,
    /// Transient failures since the last good reply.
    failures: u32,
    handshake_timeout: Duration,
    reply_timeout: Duration,
    first_backoff: Duration,
}

#[derive(Debug)]
enum State {
    /// No child: never started, or waiting until `retry_at` after a transient failure.
    Stopped {
        retry_at: Option<Instant>,
    },
    Running(Session),
    /// Given up for good; logged.
    Failed,
}

/// Why a request got no answer.
#[derive(Debug)]
enum Failure {
    /// The probe was slow or went away; a new one may do better.
    Transient(String),
    /// The probe breaks the protocol.
    Permanent(String),
}

/// A running probe.
#[derive(Debug)]
struct Session {
    child: Child,
    /// `None` only while dropping.
    stdin: Option<ChildStdin>,
    /// Lines from its stdout, read by a thread that blocks on the pipe (no wakeups while idle)
    /// and ends with it.
    lines: mpsc::Receiver<String>,
}

/// The wait before the restart that follows the `failures`-th transient failure in a row
/// (counting from 1): `first`, doubling each time, at most [`MAX_BACKOFF`].
#[must_use]
pub fn backoff(first: Duration, failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    first.saturating_mul(1 << doublings).min(MAX_BACKOFF)
}

impl Spawned {
    /// A probe that `command` will start; it must say hello within `handshake_timeout`.
    #[must_use]
    pub fn new(command: Command, handshake_timeout: Duration) -> Self {
        Self {
            command,
            state: State::Stopped { retry_at: None },
            failures: 0,
            handshake_timeout,
            reply_timeout: REPLY_TIMEOUT,
            first_backoff: FIRST_BACKOFF,
        }
    }

    #[cfg(test)]
    fn with_timeouts(mut self, reply_timeout: Duration, first_backoff: Duration) -> Self {
        self.reply_timeout = reply_timeout;
        self.first_backoff = first_backoff;
        self
    }

    /// Whether it has failed for good.
    #[cfg(test)]
    fn failed(&self) -> bool {
        matches!(self.state, State::Failed)
    }

    /// The running session, started if none runs and no backoff is pending.
    fn session(&mut self) -> Option<&mut Session> {
        if let State::Stopped { retry_at } = self.state {
            if retry_at.is_some_and(|at| Instant::now() < at) {
                return None;
            }
            self.state = match Session::start(&mut self.command, self.handshake_timeout) {
                Ok(session) => State::Running(session),
                Err(e) => {
                    let command = &self.command;
                    eprintln!("probe: {e}; no host processes or mounts (command: {command:?})");
                    State::Failed
                }
            };
        }
        match &mut self.state {
            State::Running(session) => Some(session),
            State::Stopped { .. } | State::Failed => None,
        }
    }
}

impl Probe for Spawned {
    fn ask(&mut self, request: Request) -> Option<Reply> {
        let timeout = self.reply_timeout;
        let result = self.session()?.ask(request, timeout);
        // Replacing the state drops a failed session, which kills and reaps its child.
        match result {
            Ok(reply) => {
                self.failures = 0;
                return Some(reply);
            }
            Err(Failure::Permanent(e)) => {
                eprintln!("probe: {e}; no more host processes or mounts");
                self.state = State::Failed;
            }
            Err(Failure::Transient(e)) => {
                self.failures += 1;
                if self.failures >= MAX_TRANSIENT_FAILURES {
                    eprintln!(
                        "probe: {e}; {} failures in a row, no more host processes or mounts",
                        self.failures
                    );
                    self.state = State::Failed;
                } else {
                    let wait = backoff(self.first_backoff, self.failures);
                    eprintln!("probe: {e}; starting a new one in {wait:?}");
                    self.state = State::Stopped {
                        retry_at: Some(Instant::now() + wait),
                    };
                }
            }
        }
        None
    }
}

impl Session {
    fn start(command: &mut Command, handshake_timeout: Duration) -> Result<Self, String> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("can't start the probe: {e}"))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            unreachable!("both pipes were asked for");
        };
        let (tx, lines) = mpsc::channel();
        let reader = std::thread::Builder::new()
            .name("sysmon-probe-io".into())
            .spawn(move || {
                let mut stdout = BufReader::new(stdout);
                let mut line = String::new();
                while let Ok(true) = sysmon_proto::read_line(&mut stdout, &mut line) {
                    if tx.send(std::mem::take(&mut line)).is_err() {
                        break;
                    }
                }
            });
        let session = Self {
            child,
            stdin: Some(stdin),
            lines,
        };
        if let Err(e) = reader {
            return Err(format!("can't start the probe's reader thread: {e}"));
        }
        match session.receive(handshake_timeout) {
            Ok(Reply::Hello { .. }) => Ok(session),
            Ok(other) => Err(format!("the probe didn't say hello first: {other:?}")),
            Err(e) => Err(format!("no handshake: {e}")),
        }
    }

    fn ask(&mut self, request: Request, timeout: Duration) -> Result<Reply, Failure> {
        let stdin = self.stdin.as_mut().expect("open until dropped");
        stdin
            .write_all(sysmon_proto::encode(&request).as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|e| Failure::Transient(format!("can't write to the probe: {e}")))?;
        let line = self.lines.recv_timeout(timeout).map_err(|e| {
            Failure::Transient(match e {
                mpsc::RecvTimeoutError::Timeout => {
                    format!("the probe didn't answer {request:?} in {timeout:?}")
                }
                mpsc::RecvTimeoutError::Disconnected => "the probe exited".to_owned(),
            })
        })?;
        match sysmon_proto::decode_reply(&line).map_err(|e| Failure::Permanent(e.to_string()))? {
            Reply::Error { message } => Err(Failure::Permanent(format!(
                "the probe refused {request:?}: {message}"
            ))),
            reply if answers(request, &reply) => Ok(reply),
            reply => Err(Failure::Permanent(format!(
                "the probe answered {request:?} with {reply:?}"
            ))),
        }
    }

    fn receive(&self, timeout: Duration) -> Result<Reply, String> {
        let line = self.lines.recv_timeout(timeout).map_err(|e| match e {
            mpsc::RecvTimeoutError::Timeout => format!("the probe didn't answer in {timeout:?}"),
            mpsc::RecvTimeoutError::Disconnected => "the probe exited".to_owned(),
        })?;
        sysmon_proto::decode_reply(&line).map_err(|e| e.to_string())
    }
}

/// Whether `reply` is an answer to `request`.
fn answers(request: Request, reply: &Reply) -> bool {
    matches!(
        (request, reply),
        (Request::Processes { .. }, Reply::Processes(_))
            | (Request::Mounts, Reply::Mounts { .. })
            | (Request::Gpus { .. }, Reply::Unsupported)
            | (Request::Release, Reply::Released)
    )
}

impl Drop for Session {
    fn drop(&mut self) {
        // The end of its stdin makes a probe exit. The child is killed too rather than waited
        // for, in case it hangs; killing `flatpak-spawn` closes its bus connection, so
        // `--watch-bus` ends the host process as well.
        drop(self.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const HELLO: &str = r#"{"v":1,"reply":"hello","probe":"test"}"#;
    const RELEASED: &str = r#"{"v":1,"reply":"released"}"#;
    const SHORT: Duration = Duration::from_millis(500);

    /// A probe played by `sh -c script`, with short timeouts and backoff.
    fn sh_with_backoff(script: &str, first_backoff: Duration) -> Spawned {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        Spawned::new(command, SHORT).with_timeouts(SHORT, first_backoff)
    }

    fn sh(script: &str) -> Spawned {
        sh_with_backoff(script, Duration::ZERO)
    }

    /// Says hello, then answers every line with `reply`.
    fn answering(reply: &str) -> String {
        format!("echo '{HELLO}'; while read -r line; do echo '{reply}'; done")
    }

    /// A counter file, removed when dropped.
    struct Counter(PathBuf);

    impl Drop for Counter {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// A script that counts its runs in a fresh file, then runs `per_run` (a `case` on `$n`).
    /// Returns the script and the counter.
    fn counting(test: &str, per_run: &str) -> (String, Counter) {
        let file =
            std::env::temp_dir().join(format!("sysmon-spawned-{test}-{}", std::process::id()));
        let _ = std::fs::remove_file(&file);
        let path = file.display();
        let script = format!(
            "n=$(cat '{path}' 2>/dev/null || echo 0); n=$((n + 1)); echo $n > '{path}'; {per_run}"
        );
        (script, Counter(file))
    }

    fn runs(counter: &Counter) -> u32 {
        std::fs::read_to_string(&counter.0).map_or(0, |t| t.trim().parse().unwrap())
    }

    #[test]
    fn asks_after_the_handshake_and_keeps_the_process() {
        let (script, file) = counting("keeps", &answering(RELEASED));
        let mut probe = sh(&script);
        assert_eq!(probe.ask(Request::Release), Some(Reply::Released));
        assert_eq!(probe.ask(Request::Release), Some(Reply::Released));
        assert_eq!(runs(&file), 1);
        assert!(!probe.failed());
    }

    #[test]
    fn typed_calls_unwrap_their_reply() {
        let mut probe = sh(&answering(r#"{"v":1,"reply":"mounts","mounts":[]}"#));
        assert_eq!(probe.mounts(), Some(vec![]));
    }

    #[test]
    fn backoff_doubles_up_to_ten_minutes() {
        let secs = |n| backoff(FIRST_BACKOFF, n).as_secs();
        assert_eq!([secs(1), secs(2), secs(3), secs(4)], [30, 60, 120, 240]);
        assert_eq!(secs(6), 600);
        assert_eq!(secs(100), 600);
    }

    #[test]
    fn a_timeout_starts_a_new_probe_after_the_backoff() {
        // The first probe says hello, then never answers; the second one answers.
        let (script, file) = counting(
            "timeout",
            &format!(
                "case $n in 1) echo '{HELLO}'; cat >/dev/null;; *) {};; esac",
                answering(RELEASED)
            ),
        );
        let backoff = Duration::from_millis(300);
        let mut probe = sh_with_backoff(&script, backoff);
        assert_eq!(probe.ask(Request::Release), None);
        assert!(!probe.failed());
        // Within the backoff: no new process, no wait.
        let start = Instant::now();
        assert_eq!(probe.ask(Request::Release), None);
        assert!(start.elapsed() < backoff);
        assert_eq!(runs(&file), 1);
        std::thread::sleep(backoff);
        assert_eq!(probe.ask(Request::Release), Some(Reply::Released));
        assert_eq!(runs(&file), 2);
    }

    #[test]
    fn an_exit_after_the_handshake_is_transient_and_a_good_reply_resets_the_count() {
        // Each probe answers once, then exits on the next request.
        let (script, file) = counting(
            "exit",
            &format!("echo '{HELLO}'; read -r line; echo '{RELEASED}'; read -r line; exit 0"),
        );
        let mut probe = sh(&script);
        for _ in 0..12 {
            probe.ask(Request::Release);
        }
        assert!(!probe.failed());
        assert_eq!(runs(&file), 6);
    }

    #[test]
    fn five_transient_failures_in_a_row_are_permanent() {
        // Each probe says hello and exits at the first request.
        let (script, file) = counting("five", &format!("echo '{HELLO}'; read -r line; exit 0"));
        let mut probe = sh(&script);
        for _ in 0..4 {
            assert_eq!(probe.ask(Request::Release), None);
            assert!(!probe.failed());
        }
        assert_eq!(probe.ask(Request::Release), None);
        assert!(probe.failed());
        assert_eq!(probe.ask(Request::Release), None);
        assert_eq!(runs(&file), 5);
    }

    #[test]
    fn a_failed_handshake_is_permanent_even_on_a_restart() {
        // The first probe times out (transient); the second is of another version.
        let (script, file) = counting(
            "handshake",
            &format!(
                "case $n in 1) echo '{HELLO}'; cat >/dev/null;; \
                 *) echo '{{\"v\":2,\"reply\":\"hello\",\"probe\":\"new\"}}'; cat >/dev/null;; esac"
            ),
        );
        let mut probe = sh(&script);
        assert_eq!(probe.ask(Request::Release), None);
        assert!(!probe.failed());
        assert_eq!(probe.ask(Request::Release), None);
        assert!(probe.failed());
        assert_eq!(probe.ask(Request::Release), None);
        assert_eq!(runs(&file), 2);
    }

    #[test]
    fn a_probe_of_another_version_is_given_up() {
        let mut probe = sh(r#"echo '{"v":2,"reply":"hello","probe":"new"}'; cat >/dev/null"#);
        assert_eq!(probe.ask(Request::Release), None);
        assert!(probe.failed());
    }

    #[test]
    fn a_probe_that_can_t_start_is_given_up() {
        let mut probe = Spawned::new(Command::new("/nonexistent/sysmon-probe"), SHORT);
        assert_eq!(probe.ask(Request::Release), None);
        assert!(probe.failed());
        // As the host's loader does when glibc is too old: a message, then exit.
        let (script, file) = counting("glibc", "echo 'version GLIBC_2.99 not found' >&2; exit 1");
        let mut probe = sh(&script);
        assert_eq!(probe.ask(Request::Release), None);
        assert!(probe.failed());
        assert_eq!(probe.ask(Request::Release), None);
        assert_eq!(runs(&file), 1);
    }

    #[test]
    fn a_probe_that_never_says_hello_is_given_up() {
        let mut probe = sh("cat >/dev/null");
        assert_eq!(probe.ask(Request::Release), None);
        assert!(probe.failed());
    }

    #[test]
    fn a_wrong_or_refused_answer_is_permanent() {
        let mut probe = sh(&answering(RELEASED));
        assert_eq!(probe.ask(Request::Mounts), None);
        assert!(probe.failed());
        let mut probe = sh(&answering(r#"{"v":1,"reply":"error","message":"no"}"#));
        assert_eq!(probe.ask(Request::Release), None);
        assert!(probe.failed());
    }
}
