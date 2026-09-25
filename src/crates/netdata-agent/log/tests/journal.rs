//! Records sent to a journal socket found under the host prefix, the way the direct-socket search finds
//! `<prefix>/run/systemd/journal.netdata/socket`. A record too large for a datagram arrives as a sealed memfd.

use std::os::unix::net::UnixDatagram;

use netdata_agent_log::{
    Source, initialize, netdata_log_info, set_default_log_dir, set_host_prefix, set_program_name,
    set_user_settings,
};

fn fields(datagram: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(datagram)
        .lines()
        .filter(|l| {
            !["TID=", "THREAD_TAG=", "INVOCATION_ID=", "CODE_"]
                .iter()
                .any(|p| l.starts_with(p))
        })
        .map(str::to_string)
        .collect()
}

#[test]
fn records_go_to_the_journal_socket_with_their_fields() {
    let dir = std::env::temp_dir().join(format!("ndlog-journal-{}", std::process::id()));
    let socket_dir = dir.join("run/systemd/journal.netdata");
    std::fs::create_dir_all(&socket_dir).unwrap();
    let journald = UnixDatagram::bind(socket_dir.join("socket")).unwrap();

    set_program_name("netdata");
    set_host_prefix(&dir.to_string_lossy());
    set_default_log_dir(&dir.to_string_lossy());
    for source in [Source::Collector, Source::Access, Source::Health] {
        set_user_settings(source, "none");
    }
    set_user_settings(Source::Daemon, "journal");
    initialize();

    netdata_log_info!("to the journal");
    let mut buf = vec![0u8; 65536];
    let n = journald.recv(&mut buf).unwrap();
    assert_eq!(
        fields(&buf[..n]),
        [
            "SYSLOG_IDENTIFIER=netdata",
            "ND_LOG_SOURCE=daemon",
            "PRIORITY=6",
            "MESSAGE=to the journal",
        ]
    );

    // larger than any datagram the socket accepts
    let big = "x".repeat(4 * 1024 * 1024);
    netdata_log_info!("{big}");
    let mut cmsg = nix::cmsg_space!([std::os::fd::RawFd; 1]);
    let mut iov = [std::io::IoSliceMut::new(&mut buf)];
    let msg = nix::sys::socket::recvmsg::<()>(
        std::os::fd::AsRawFd::as_raw_fd(&journald),
        &mut iov,
        Some(&mut cmsg),
        nix::sys::socket::MsgFlags::empty(),
    )
    .unwrap();
    let fd = msg
        .cmsgs()
        .unwrap()
        .find_map(|c| match c {
            nix::sys::socket::ControlMessageOwned::ScmRights(fds) => fds.first().copied(),
            _ => None,
        })
        .expect("a memfd");
    let payload = std::fs::read(format!("/proc/self/fd/{fd}")).unwrap();
    let fields = fields(&payload);
    assert_eq!(
        fields.last().map(String::len),
        Some("MESSAGE=".len() + big.len())
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
