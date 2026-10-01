//! The daemon binary is its own spawn server (D12, D140): with the marker set it runs the server before its options
//! (a late dispatch would print the version for `-v`), and a server created from it starts children.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn the_marker_runs_the_spawn_server_first() {
    let started = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_netdata"))
        .arg("-v")
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

/// The whole bootstrap through the daemon binary itself (its allocator, curl's constructor): a server started from it
/// runs a child and answers its exit. Unnamed, its socket is `/tmp/netdata-spawn-<pid>-<id>.sock`.
#[test]
fn a_server_from_the_daemon_binary_runs_children() {
    use std::os::fd::AsFd;
    let start = netdata_agent_spawn::client::Start { exe: env!("CARGO_BIN_EXE_netdata").into(), run_dir: || None };
    let server = netdata_agent_spawn::client::Server::create(None, false, &start).expect("created");
    let stderr = std::io::stderr();
    let child = server.exec(stderr.as_fd(), std::io::stdin().as_fd(), &["/bin/sh", "-c", "exit 3"]).expect("started");
    assert_eq!(child.wait(), 3 << 8);
    let path = server.path().to_path_buf();
    server.destroy();
    assert!(!path.exists());
}
