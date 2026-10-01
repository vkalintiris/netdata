use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;

use netdata_agent_log::Priority;
use netdata_agent_rrd::host::HostInfo;
use netdata_agent_rrd::mode::DbMode;

use super::*;

fn host() -> Host {
    Host::new(
        "5a1e0000-0000-4000-8000-0000000000cc",
        true,
        HostInfo {
            hostname: "child".into(),
            registry_hostname: "child".into(),
            os: "linux".into(),
            timezone: "UTC".into(),
            abbrev_timezone: "UTC".into(),
            utc_offset: 0,
            program_name: "netdata".into(),
            program_version: "v1".into(),
            update_every: 1,
            db_mode: DbMode::Ram,
            history_entries: 4096,
            health_enabled: false,
            system_info: Default::default(),
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            stream_send: None,
            cache_dir: None,
        },
    )
}

fn local() -> Local {
    Local { host_id: [7; 16], user_agent: "netdata/v1".into(), update_every: 1 }
}

/// An HTTP answer carrying `body` with its length.
fn json_answer(body: &str) -> &'static [u8] {
    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).leak().as_bytes()
}

/// A parent that answers its first connection (the probe) with `answer` and holds the next open.
fn parent(answer: &'static [u8]) -> (String, std::thread::JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let handle = std::thread::spawn(move || {
        let (mut probe, _) = listener.accept().unwrap();
        let mut request = vec![0u8; 4096];
        let n = probe.read(&mut request).unwrap();
        request.truncate(n);
        probe.write_all(answer).unwrap();
        drop(probe);
        if answer.starts_with(b"HTTP/1.1 404") {
            let _ = listener.accept();
        }
        request
    });
    (addr, handle)
}

/// One pass over `parents`: whether it connected, the records (priority and text), the host's reason.
fn pass(parents: &mut Parents) -> (bool, Vec<(Priority, String)>, Reason, SockError) {
    let cancel = AtomicBool::new(false);
    let th = Thread::new(&cancel);
    let mut sock = NdSock::default();
    let mut reason = Reason::NEVER;
    let h = host();
    let (ok, records) = netdata_agent_log::capture(|| {
        parents.connect_to_one(&mut sock, &h, &local(), 1, &mut reason, 19999, 5, &th)
    });
    let records = records.into_iter().map(|r| (r.priority, r.message.unwrap_or_default())).collect();
    (ok, records, reason, sock.error)
}

fn messages(records: &[(Priority, String)]) -> Vec<String> {
    records.iter().map(|(_, m)| m.clone()).collect()
}

#[test]
fn a_probe_answered_with_an_empty_404_keeps_the_parent_and_connects() {
    let (addr, stub) = parent(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
    let mut parents = Parents::new([(addr.as_str(), false)].into_iter());
    let (ok, records, reason, error) = pass(&mut parents);
    assert!(ok);
    assert_eq!((reason, error), (Reason::SP_CONNECTED, SockError::None));
    let fd = records.last().unwrap().1.rsplit("fd ").next().unwrap().to_string();
    assert_eq!(
        messages(&records),
        [
            format!("STREAM PARENTS 'child': fetching stream info from '{addr}'..."),
            format!("STREAM PARENTS 'child': stream info response from '{addr}' has invalid Content-Length"),
            format!("STREAM PARENTS 'child': only 1 parent is available: '{addr}'"),
            format!("STREAM PARENTS 'child': connecting to '{addr}' (default port: 19999, parent 1 of 1)..."),
            format!("STREAM PARENTS 'child': connected to '{addr}' (default port: 19999, fd {fd}"),
        ]
    );
    assert_eq!(parents.current().map(|d| (d.attempts, d.reason)), Some((1, Reason::PARENT_INTERNAL_ERROR)));
    let request = String::from_utf8(stub.join().unwrap()).unwrap();
    assert_eq!(
        request,
        format!(
            "GET /api/v3/stream_info?machine_guid=5a1e0000-0000-4000-8000-0000000000cc HTTP/1.1\r\nHost: {addr}\r\n\
             User-Agent: netdata/v1\r\nAccept: */*\r\nAccept-Encoding: identity\r\nTE: identity\r\nPragma: \
             no-cache\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n"
        )
    );
}

#[test]
fn a_c_parents_404_is_reported_and_its_members_kept() {
    let (addr, _stub) = parent(json_answer(
        r#"{"version":1,"status":404,"host_id":"11111111-2222-3333-4444-555555555555","nodes":3,"receivers":0,"nonce":7}"#,
    ));
    let mut parents = Parents::new([(addr.as_str(), false)].into_iter());
    let (_, records, ..) = pass(&mut parents);
    assert_eq!(
        records[1],
        (
            Priority::Warning,
            format!(
                "STREAM PARENTS 'child': failed to extract fields from JSON stream info response from '{addr}': status \
                 reported (404) is not OK (200) - JSON data: {{\"version\":1,\"status\":404,\"host_id\":\
                 \"11111111-2222-3333-4444-555555555555\",\"nodes\":3,\"receivers\":0,\"nonce\":7}}"
            )
        )
    );
    let d = &parents.list[0];
    assert_eq!((d.remote.nodes, d.remote.nonce, d.remote.status), (3, 7, 404));
}

#[test]
fn a_parent_reporting_our_host_as_its_localhost_is_banned_for_good() {
    let (addr, _stub) = parent(json_answer(
        r#"{"version":1,"status":200,"host_id":"11111111-2222-3333-4444-555555555555","nodes":1,"receivers":0,"nonce":1,"db_status":"online","db_liveness":"live","ingest_type":"localhost","ingest_status":"online","first_time_s":1,"last_time_s":2}"#,
    ));
    let mut parents = Parents::new([(addr.as_str(), false)].into_iter());
    let (ok, records, reason, error) = pass(&mut parents);
    assert!(!ok);
    assert_eq!((reason, error), (Reason::SP_NO_DESTINATION, SockError::NoDestinationAvailable));
    assert_eq!(
        &records[2..],
        [
            (
                Priority::Warning,
                format!(
                    "STREAM PARENTS 'child': destination '{addr}' is banned permanently because it is the origin \
                     server, but it is not in the stream path before us!"
                )
            ),
            (
                Priority::Debug,
                "STREAM PARENTS 'child': no parents available (0 skipped but useful, 1 skipped not useful, 0 potential)"
                    .to_string()
            ),
        ]
    );
    assert!(parents.list[0].banned_permanently);
}

#[test]
fn a_refused_probe_postpones_and_blocks_its_parent() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let addr = format!("127.0.0.1:{port}");
    let mut parents = Parents::new([(addr.as_str(), false)].into_iter());
    let before = now_realtime_ut();
    let (ok, records, _, error) = pass(&mut parents);
    assert!(!ok);
    // the connection's own socket never tried, so the connector logs nothing more
    assert_eq!(error, SockError::NoDestinationAvailable);
    assert_eq!(
        messages(&records)[1..],
        [
            format!("Failed to connect to '127.0.0.1', port '{port}'"),
            format!("STREAM PARENTS 'child': failed to connect for stream info to '{addr}': connection refused"),
            format!("STREAM PARENTS 'child': only 1 parent is available: '{addr}'"),
        ]
    );
    let d = &parents.list[0];
    assert_eq!(d.reason, Reason::SP_CONNECTION_REFUSED);
    assert!((before + 30_000_000..before + 61_000_000).contains(&d.postpone_until_ut));
    assert!(is_blocked(&addr));
    // the next pass counts it potential without a record
    let (_, records, reason, _) = pass(&mut parents);
    assert_eq!(
        messages(&records),
        ["STREAM PARENTS 'child': no parents available (0 skipped but useful, 0 skipped not useful, 1 potential)"]
    );
    assert_eq!(reason, Reason::SP_CONNECTION_REFUSED);
}

#[test]
fn parents_rank_by_newest_data_and_shuffle_within_120_seconds() {
    let mut parents = Parents::new([("a", false), ("b", false), ("c", false)].into_iter());
    for (d, last) in parents.list.iter_mut().zip([100, 1000, 950]) {
        d.remote.db_last_time_s = last;
    }
    let cancel = AtomicBool::new(false);
    let th = Thread::new(&cancel);
    let mut array = vec![0, 1, 2];
    parents.rank(&mut array, "child", &th);
    let mut first_two = [array[0], array[1]];
    first_two.sort();
    assert_eq!((first_two, array[2]), ([1, 2], 0));
    assert!(parents.list[1].selection.random && !parents.list[0].selection.random);
    assert_eq!(parents.list[0].selection.batch, 2);
}

#[test]
fn delays_fall_within_c_s_bounds() {
    for (min, max, low, high) in [(5, 60, 5, 60), (0, 0, 5, 5), (30, 10, 30, 30), (3600, 7200, 3600, 7200)] {
        let now = now_realtime_ut();
        let at = randomize_wait_ut(min, max);
        let after = now_realtime_ut();
        assert!(at >= now + low * 1_000_000, "{min} {max}");
        assert!(at <= after + (high.max(low + 1)) * 1_000_000, "{min} {max}");
    }
}

#[test]
fn a_reset_postpones_every_parent_alike_and_lifts_only_the_session_bans() {
    let mut parents = Parents::new([("a", false), ("b", false)].into_iter());
    parents.list[0].banned_for_this_session = true;
    parents.list[1].banned_permanently = true;
    parents.list[1].reason = Reason::SP_CONNECTION_REFUSED;
    // C's window is [max(5, d / 2), d + 5) seconds: 15 gives [7, 20), 2 gives [5, 7); 200 draws reach both ends'
    // quarters
    for (delay, low, high) in [(15, 7, 20), (2, 5, 7)] {
        let (mut first, mut last) = (u64::MAX, 0);
        for _ in 0..200 {
            let before = now_realtime_ut();
            parents.reset(Reason::NEVER, delay);
            let after = now_realtime_ut();
            let until = parents.list[0].postpone_until_ut;
            assert!(until >= before + low * 1_000_000 && until < after + high * 1_000_000, "{delay}");
            let states: Vec<_> = parents
                .list
                .iter()
                .map(|d| (d.postpone_until_ut, d.banned_for_this_session, d.banned_permanently, d.reason))
                .collect();
            assert_eq!(states, [(until, false, false, Reason::NEVER), (until, false, true, Reason::NEVER)]);
            first = first.min(until - before);
            last = last.max(until - before);
        }
        let quarter = (high - low) * 250_000;
        assert!(first < low * 1_000_000 + quarter && last > high * 1_000_000 - quarter, "{delay}: {first}..{last}");
    }
}

/// `stream_parent_nd_sock_error_to_reason()`: each socket error's reason, postponement window (a timeout's longer for a
/// parent with 10 nodes or more), session ban and block of the parent for every node, as C's table.
#[test]
fn socket_errors_postpone_and_block_as_c() {
    use SockError as E;
    let cases = [
        (E::ConnectionRefused, 0, Reason::SP_CONNECTION_REFUSED, (30, 60), false, Some(30)),
        (E::CannotResolveHostname, 0, Reason::SP_CANT_RESOLVE_HOSTNAME, (30, 60), false, Some(30)),
        (E::NoHostInDefinition, 0, Reason::SP_NO_HOST_IN_DESTINATION, (30, 60), true, Some(30)),
        (E::Timeout, 9, Reason::SP_CONNECT_TIMEOUT, (300, 600), false, Some(300)),
        (E::Timeout, 10, Reason::SP_CONNECT_TIMEOUT, (300, 900), false, Some(300)),
        (E::SslInvalidCertificate, 0, Reason::CONNECT_INVALID_CERTIFICATE, (300, 600), false, Some(300)),
        (E::SslCantEstablishSslConnection, 0, Reason::CONNECT_SSL_ERROR, (60, 180), false, Some(60)),
        (E::SslFailedToOpen, 0, Reason::CONNECT_SSL_ERROR, (60, 180), false, Some(60)),
        (E::PollError, 0, Reason::PARENT_INTERNAL_ERROR, (30, 60), false, None),
        (E::FailedToCreateSocket, 0, Reason::PARENT_INTERNAL_ERROR, (30, 60), false, None),
        (E::UnknownError, 0, Reason::PARENT_INTERNAL_ERROR, (30, 60), false, None),
        (E::ThreadCancelled, 0, Reason::PARENT_INTERNAL_ERROR, (30, 60), false, None),
        (E::NoDestinationAvailable, 0, Reason::PARENT_INTERNAL_ERROR, (30, 60), false, None),
    ];
    for (i, (error, nodes, reason, (low, high), banned, block)) in cases.into_iter().enumerate() {
        let destination = format!("socket-error-{i}");
        let mut d = Parent::new(&destination, false);
        d.remote.nodes = nodes;
        let before = now_realtime_ut();
        d.sock_error_to_reason(error);
        let after = now_realtime_ut();
        let blocked = BLOCKED
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|b| b.get(&destination))
            .map(|&(_, duration)| duration.as_secs());
        assert_eq!((d.reason, d.banned_for_this_session, blocked), (reason, banned, block), "{error:?}");
        assert!(
            d.postpone_until_ut >= before + low * 1_000_000 && d.postpone_until_ut <= after + high * 1_000_000,
            "{error:?}"
        );
    }
}

#[test]
fn the_path_before_us_is_the_agent_itself_or_a_closer_hop() {
    let entry = |id: u8, hops| PathEntry { host_id: [id; 16], hops, ..PathEntry::default() };
    let path = [entry(1, 0), entry(2, 1)];
    assert!(is_in_stream_path_before_us(&[9; 16], &path, &[9; 16], 1));
    assert!(is_in_stream_path_before_us(&[9; 16], &path, &[1; 16], 1));
    assert!(!is_in_stream_path_before_us(&[9; 16], &path, &[2; 16], 1));
    assert!(!is_in_stream_path_before_us(&[0; 16], &path, &[0; 16], 1));
}
