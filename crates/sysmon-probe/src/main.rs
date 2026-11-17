// SPDX-License-Identifier: MPL-2.0

//! `sysmon-probe`: answers the applet's requests on stdin/stdout and exits at the end of stdin.
//! It takes no arguments (see the library).

use std::io;
use std::process::ExitCode;

fn main() -> ExitCode {
    if std::env::args_os().len() > 1 {
        eprintln!("sysmon-probe takes no arguments; it answers requests on stdin");
        return ExitCode::FAILURE;
    }
    match sysmon_probe::serve(
        io::stdin().lock(),
        io::stdout().lock(),
        &mut sysmon_probe::Host::new(),
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("sysmon-probe: {e}");
            ExitCode::FAILURE
        }
    }
}
