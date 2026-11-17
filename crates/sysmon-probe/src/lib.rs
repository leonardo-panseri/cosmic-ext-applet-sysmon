// SPDX-License-Identifier: MPL-2.0

//! The host **probe** (plan §3.2): what the Flatpak sandbox can't see, read on the host.
//!
//! [`Host::answer`] is the whole behaviour: one [`Request`] in, one bounded [`Reply`] out. The
//! `sysmon-probe` binary wraps it in [`serve`] (JSON lines on stdin and stdout); outside a
//! Flatpak the applet calls [`Host::answer`] in-process. Neither takes paths, commands or other
//! input that would reach the file system: requests are a closed enum, so the probe is read-only
//! by construction.
//!
//! The probe does nothing between requests: no timers, no threads. Blocked on stdin, it makes no
//! wakeups.

pub mod mounts;
pub mod procs;

use std::io::{self, BufRead, Write};
use std::path::Path;

use sysmon_proto::{MAX_TOP, Reply, Request};

/// Answers requests about the system it runs on.
#[derive(Debug)]
pub struct Host {
    scanner: procs::Scanner,
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

impl Host {
    #[must_use]
    pub fn new() -> Self {
        Self {
            scanner: procs::Scanner::new(Path::new("/proc")),
        }
    }

    /// Answers `request`. `Processes` may take up to [`procs::BASELINE`] longer when no recent
    /// scan exists to measure CPU against.
    pub fn answer(&mut self, request: Request) -> Reply {
        match request {
            Request::Processes { top } => {
                Reply::Processes(self.scanner.top(usize::from(top.min(MAX_TOP))))
            }
            Request::Mounts => Reply::Mounts {
                mounts: mounts::read(Path::new("/proc/self/mounts")),
            },
            // NVML comes in P4; this probe must not touch any GPU.
            Request::Gpus { .. } => Reply::Unsupported,
            Request::Release => Reply::Released,
        }
        .bounded()
    }
}

/// Writes [`Reply::Hello`], then answers each request line on `input` with one line on `output`,
/// until `input` ends. A line that isn't a request of this version gets [`Reply::Error`].
///
/// # Errors
/// If reading or writing fails, or a line is longer than the protocol allows.
pub fn serve(mut input: impl BufRead, mut output: impl Write, host: &mut Host) -> io::Result<()> {
    sysmon_proto::write_message(
        &mut output,
        &Reply::Hello {
            probe: env!("CARGO_PKG_VERSION").into(),
        },
    )?;
    let mut line = String::new();
    while sysmon_proto::read_line(&mut input, &mut line)? {
        let reply = match sysmon_proto::decode::<Request>(&line) {
            Ok(request) => host.answer(request),
            Err(e) => Reply::Error {
                message: e.to_string(),
            }
            .bounded(),
        };
        sysmon_proto::write_message(&mut output, &reply)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sysmon_proto::{decode_reply, encode};

    fn session(input: &str) -> Vec<Reply> {
        let mut output = Vec::new();
        serve(io::Cursor::new(input), &mut output, &mut Host::new()).unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| decode_reply(l).unwrap())
            .collect()
    }

    #[test]
    fn says_hello_first_and_stops_at_the_end_of_input() {
        assert_eq!(
            session(""),
            [Reply::Hello {
                probe: env!("CARGO_PKG_VERSION").into()
            }]
        );
    }

    #[test]
    fn answers_each_request_in_order() {
        let input = [
            encode(&Request::Release),
            encode(&Request::Gpus { nvml: true }),
            encode(&Request::Release),
        ]
        .concat();
        assert_eq!(
            session(&input)[1..],
            [Reply::Released, Reply::Unsupported, Reply::Released]
        );
    }

    #[test]
    fn a_bad_line_gets_an_error_and_the_session_goes_on() {
        let input = format!(
            "{{\"req\":\"exec\",\"v\":1}}\n{{\"req\":\"release\",\"v\":2}}\n{}",
            encode(&Request::Release)
        );
        let replies = session(&input);
        assert!(matches!(replies[1], Reply::Error { .. }));
        assert!(matches!(&replies[2], Reply::Error { message } if message.contains("version 2")));
        assert_eq!(replies[3], Reply::Released);
    }

    #[test]
    fn an_overlong_line_ends_the_session() {
        let input = "x".repeat(sysmon_proto::MAX_LINE_BYTES + 1);
        let mut output = Vec::new();
        assert!(serve(io::Cursor::new(input), &mut output, &mut Host::new()).is_err());
    }

    #[test]
    fn processes_are_capped_at_five_per_list() {
        let Reply::Processes(top) = Host::new().answer(Request::Processes { top: 200 }) else {
            panic!("not a processes reply");
        };
        assert_eq!(top.by_cpu.len(), 5);
        assert_eq!(top.by_memory.len(), 5);
    }
}
