//! `alarm-notify` - the Agent's alert notification dispatcher.
//!
//! A thin shell: argument classification, dispatch and exit-code mapping all live in
//! the library so integration tests can drive the same code path the daemon does.

use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let program_name = std::env::args()
        .next()
        .map(|p| {
            std::path::Path::new(&p)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or(p)
        })
        .unwrap_or_else(|| "alarm-notify".to_string());

    alarm_notify::run(&argv, &program_name)
}
