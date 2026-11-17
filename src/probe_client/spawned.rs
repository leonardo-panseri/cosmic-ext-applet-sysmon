// SPDX-License-Identifier: MPL-2.0

//! A probe in a child process, over JSON lines on its stdin and stdout.

use std::io::{BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use sysmon_proto::{Reply, Request};

use super::{Probe, REPLY_TIMEOUT};

/// A probe process: started on the first [`Probe::ask`], then kept.
#[derive(Debug)]
pub struct Spawned {
    state: State,
    handshake_timeout: Duration,
    reply_timeout: Duration,
}

#[derive(Debug)]
enum State {
    NotStarted(Command),
    Running(Session),
    /// It failed once and was logged; it stays unavailable.
    Failed,
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

impl Spawned {
    /// A probe that `command` will start; it must say hello within `handshake_timeout`.
    #[must_use]
    pub fn new(command: Command, handshake_timeout: Duration) -> Self {
        Self {
            state: State::NotStarted(command),
            handshake_timeout,
            reply_timeout: REPLY_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_reply_timeout(mut self, timeout: Duration) -> Self {
        self.reply_timeout = timeout;
        self
    }

    /// Whether it has failed for good.
    #[cfg(test)]
    fn failed(&self) -> bool {
        matches!(self.state, State::Failed)
    }

    fn session(&mut self) -> Option<&mut Session> {
        if let State::NotStarted(command) = &mut self.state {
            self.state = match Session::start(command, self.handshake_timeout) {
                Ok(session) => State::Running(session),
                Err(e) => {
                    eprintln!("probe: {e}; no host processes or mounts (command: {command:?})");
                    State::Failed
                }
            };
        }
        match &mut self.state {
            State::Running(session) => Some(session),
            State::NotStarted(_) | State::Failed => None,
        }
    }
}

impl Probe for Spawned {
    fn ask(&mut self, request: Request) -> Option<Reply> {
        let timeout = self.reply_timeout;
        let result = self.session()?.ask(request, timeout);
        match result {
            Ok(reply) => Some(reply),
            Err(e) => {
                eprintln!("probe: {e}; no more host processes or mounts");
                // Dropping the session closes the probe's stdin and reaps it.
                self.state = State::Failed;
                None
            }
        }
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

    fn ask(&mut self, request: Request, timeout: Duration) -> Result<Reply, String> {
        let stdin = self.stdin.as_mut().expect("open until dropped");
        stdin
            .write_all(sysmon_proto::encode(&request).as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|e| format!("can't write to the probe: {e}"))?;
        match self.receive(timeout)? {
            Reply::Error { message } => Err(format!("the probe refused {request:?}: {message}")),
            reply if answers(request, &reply) => Ok(reply),
            reply => Err(format!("the probe answered {request:?} with {reply:?}")),
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
    use super::*;

    const HELLO: &str = r#"{"v":1,"reply":"hello","probe":"test"}"#;
    const RELEASED: &str = r#"{"v":1,"reply":"released"}"#;
    const SHORT: Duration = Duration::from_millis(500);

    /// A probe played by `sh -c script`.
    fn sh(script: &str) -> Spawned {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        Spawned::new(command, SHORT).with_reply_timeout(SHORT)
    }

    /// Says hello, then answers every line with `reply`.
    fn answering(reply: &str) -> Spawned {
        sh(&format!(
            "echo '{HELLO}'; while read -r line; do echo '{reply}'; done"
        ))
    }

    #[test]
    fn asks_after_the_handshake_and_keeps_the_process() {
        let mut probe = answering(RELEASED);
        assert_eq!(probe.ask(Request::Release), Some(Reply::Released));
        assert_eq!(probe.ask(Request::Release), Some(Reply::Released));
        assert!(!probe.failed());
    }

    #[test]
    fn typed_calls_unwrap_their_reply() {
        let mut probe = answering(r#"{"v":1,"reply":"mounts","mounts":[]}"#);
        assert_eq!(probe.mounts(), Some(vec![]));
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
        let mut probe = sh("echo 'version GLIBC_2.99 not found' >&2; exit 1");
        assert_eq!(probe.ask(Request::Release), None);
        assert!(probe.failed());
    }

    #[test]
    fn a_silent_probe_times_out_and_is_given_up() {
        let mut probe = sh("cat >/dev/null");
        assert_eq!(probe.ask(Request::Release), None);
        assert!(probe.failed());
        let mut probe = sh(&format!("echo '{HELLO}'; cat >/dev/null"));
        assert_eq!(probe.ask(Request::Release), None);
        assert!(probe.failed());
        // Given up for good: no new process, no wait.
        let start = std::time::Instant::now();
        assert_eq!(probe.ask(Request::Release), None);
        assert!(start.elapsed() < SHORT);
    }

    #[test]
    fn a_wrong_or_refused_answer_gives_the_probe_up() {
        let mut probe = answering(RELEASED);
        assert_eq!(probe.ask(Request::Mounts), None);
        assert!(probe.failed());
        let mut probe = answering(r#"{"v":1,"reply":"error","message":"no"}"#);
        assert_eq!(probe.ask(Request::Release), None);
        assert!(probe.failed());
    }
}
