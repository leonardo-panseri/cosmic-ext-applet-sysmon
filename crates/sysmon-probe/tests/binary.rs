// SPDX-License-Identifier: MPL-2.0

//! The `sysmon-probe` binary as the applet runs it: requests on stdin, replies on stdout.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use sysmon_proto::{Reply, Request, decode_reply, encode};

#[test]
fn answers_on_stdout_and_exits_at_the_end_of_stdin() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sysmon-probe"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut reply = || {
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        decode_reply(&line).unwrap()
    };
    assert!(matches!(reply(), Reply::Hello { .. }));
    stdin
        .write_all(encode(&Request::Mounts).as_bytes())
        .unwrap();
    assert!(matches!(reply(), Reply::Mounts { .. }));
    stdin
        .write_all(encode(&Request::Processes { top: 5 }).as_bytes())
        .unwrap();
    let Reply::Processes(top) = reply() else {
        panic!("not a processes reply")
    };
    assert_eq!(top.by_memory.len(), 5);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn takes_no_arguments() {
    let status = Command::new(env!("CARGO_BIN_EXE_sysmon-probe"))
        .arg("/etc/shadow")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
}
