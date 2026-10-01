//! The daemon binary is its own spawn server (D12, D140): with the marker set it must run the server before anything
//! else. Without a bootstrap on its stdin the server gives up at once, so a binary that dispatches late would be
//! seen starting a whole daemon instead.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn the_marker_runs_the_spawn_server_first() {
    let started = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_netdata"))
        .env("NETDATA_SPAWN_SERVER", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the binary did not exit as a spawn server without its bootstrap");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = child.wait_with_output().unwrap();
    assert_eq!(status.code(), Some(1));
    // nothing of the daemon ran: no record, nothing printed
    assert_eq!((output.stdout.len(), output.stderr.len()), (0, 0), "{}", String::from_utf8_lossy(&output.stderr));
}
