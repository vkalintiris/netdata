//! A host's parents (`src/streaming/stream-parents.c`): the destinations it may stream to, the `stream_info` probe
//! of each, their ranking and the connection to the first that answers, with C's postponements, bans and the
//! blocks shared by every host. Map: `knowledge/map-m7-commit3-connector.md` §1, §3, §4.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use netdata_agent_ingest::jsonc::{self, Presence};
use netdata_agent_log::{Field, Priority, Value, push};
use netdata_agent_rrd::clock::now_realtime_ut;
use netdata_agent_rrd::host::Host;
use netdata_agent_rrd::pulse::host_status;
use netdata_agent_rrd::status::{DbLiveness, DbStatus, IngestStatus, IngestType};
use netdata_agent_rrd::stream_path::PathEntry;
use netdata_agent_text::c::c_str;
use netdata_agent_tls::SslContext;

use crate::connect_to::{NdSock, SockError, Thread, effective_service, log_errno};
use crate::random::{os_random, os_random32};
use crate::reason::Reason;

/// `SENDER_MIN_RECONNECT_DELAY`.
pub const MIN_RECONNECT_DELAY_S: i64 = 5;
/// `TIME_TO_CONSIDER_PARENTS_SIMILAR`.
const SIMILAR_S: i64 = 120;
/// `HTTP_HEADER_SIZE`: the probe's request and response buffer.
pub(crate) const HTTP_HEADER_SIZE: usize = 8192;
/// The probe's connect, send and receive timeouts.
const PROBE_TIMEOUT_S: i64 = 5;
const USEC_PER_SEC: u64 = 1_000_000;

/// `randomize_wait_ut()`: a wall-clock time in `[min, max)` seconds from now, at microsecond resolution; `min` is at
/// least 5 s and `max` at least `min` (then exactly `min`).
pub fn randomize_wait_ut(min_s: i64, max_s: i64) -> u64 {
    let min_s = min_s.max(MIN_RECONNECT_DELAY_S);
    let max_s = max_s.max(min_s);
    let (min_ut, max_ut) = (min_s as u64 * USEC_PER_SEC, max_s as u64 * USEC_PER_SEC);
    now_realtime_ut() + min_ut + os_random(max_ut - min_ut)
}

/// What a parent's `stream_info` reported (`d->remote`); zero-initialized as C's.
#[derive(Debug, Clone, Copy, Default)]
pub struct Remote {
    pub host_id: [u8; 16],
    pub status: i32,
    pub nonce: u32,
    pub nodes: u64,
    pub receivers: u64,
    pub db_status: Option<DbStatus>,
    pub db_liveness: Option<DbLiveness>,
    pub ingest_type: Option<IngestType>,
    pub ingest_status: Option<IngestStatus>,
    pub db_first_time_s: i64,
    pub db_last_time_s: i64,
}

impl Remote {
    /// The enums as C holds them: zero is the first value of each.
    fn ingest_type(&self) -> IngestType {
        self.ingest_type.unwrap_or(IngestType::Localhost)
    }

    fn ingest_status(&self) -> IngestStatus {
        self.ingest_status.unwrap_or(IngestStatus::Archived)
    }
}

/// `d->selection`: where the last pass placed the parent.
#[derive(Debug, Clone, Copy, Default)]
pub struct Selection {
    pub batch: usize,
    pub order: usize,
    pub random: bool,
    pub info: bool,
    pub skipped: bool,
}

/// `STREAM_PARENT`.
#[derive(Debug, Clone)]
pub struct Parent {
    pub destination: String,
    pub ssl: bool,
    pub banned_permanently: bool,
    pub banned_for_this_session: bool,
    pub banned_temporarily_erroneous: bool,
    pub reason: Reason,
    pub attempts: u32,
    pub since_ut: u64,
    pub postpone_until_ut: u64,
    pub remote: Remote,
    pub selection: Selection,
}

impl Parent {
    fn new(destination: &str, ssl: bool) -> Self {
        Parent {
            destination: destination.to_string(),
            ssl,
            banned_permanently: false,
            banned_for_this_session: false,
            banned_temporarily_erroneous: false,
            reason: Reason::NEVER,
            attempts: 0,
            since_ut: now_realtime_ut(),
            postpone_until_ut: 0,
            remote: Remote::default(),
            selection: Selection::default(),
        }
    }

    /// `stream_parent_set_reconnect_delay()`.
    fn set_reconnect_delay(&mut self, reason: Reason, secs: i64) {
        self.reason = reason;
        self.postpone_until_ut = randomize_wait_ut(5, secs);
    }

    /// `stream_parent_nd_sock_error_to_reason()`.
    fn sock_error_to_reason(&mut self, error: SockError) {
        let (reason, min, max, block) = match error {
            SockError::ConnectionRefused => (Reason::SP_CONNECTION_REFUSED, 30, 60, Some(30)),
            SockError::CannotResolveHostname => (Reason::SP_CANT_RESOLVE_HOSTNAME, 30, 60, Some(30)),
            SockError::NoHostInDefinition => {
                self.banned_for_this_session = true;
                (Reason::SP_NO_HOST_IN_DESTINATION, 30, 60, Some(30))
            }
            SockError::Timeout => {
                (Reason::SP_CONNECT_TIMEOUT, 300, if self.remote.nodes < 10 { 600 } else { 900 }, Some(300))
            }
            SockError::SslInvalidCertificate => (Reason::CONNECT_INVALID_CERTIFICATE, 300, 600, Some(300)),
            SockError::SslCantEstablishSslConnection | SockError::SslFailedToOpen => {
                (Reason::CONNECT_SSL_ERROR, 60, 180, Some(60))
            }
            _ => (Reason::PARENT_INTERNAL_ERROR, 30, 60, None),
        };
        self.reason = reason;
        self.postpone_until_ut = randomize_wait_ut(min, max);
        if let Some(secs) = block {
            block_for_all_nodes(&self.destination, secs);
        }
    }
}

/// `blocked_parents_set`: destinations that failed at the socket level, skipped by every host for a while (on the
/// monotonic clock).
static BLOCKED: Mutex<Option<HashMap<String, (Instant, Duration)>>> = Mutex::new(None);

/// `block_parent_for_all_nodes()`.
fn block_for_all_nodes(destination: &str, secs: u64) {
    let mut blocked = BLOCKED.lock().unwrap_or_else(PoisonError::into_inner);
    blocked
        .get_or_insert_with(HashMap::new)
        .insert(destination.to_string(), (Instant::now(), Duration::from_secs(secs)));
}

/// `is_a_blocked_parent()`.
fn is_blocked(destination: &str) -> bool {
    let blocked = BLOCKED.lock().unwrap_or_else(PoisonError::into_inner);
    blocked
        .as_ref()
        .and_then(|b| b.get(destination))
        .is_some_and(|&(since, duration)| since.elapsed() < duration)
}

/// `rrdhost_is_host_in_stream_path_before_us()`: the remote agent is this agent, or an agent of the host's path
/// closer to its origin than `our_hops`.
pub fn is_in_stream_path_before_us(localhost_id: &[u8; 16], path: &[PathEntry], remote: &[u8; 16], our_hops: i16) -> bool {
    if *remote == [0; 16] {
        return false;
    }
    remote == localhost_id || path.iter().any(|p| p.host_id == *remote && p.hops < our_hops)
}

/// What the probe and the connection need of this agent: localhost's host id, its `program_name/version`, and
/// `nd_profile.update_every` (`[db] update every`), which the request reports for every host.
#[derive(Debug, Clone)]
pub struct Local {
    pub host_id: [u8; 16],
    pub user_agent: String,
    pub update_every: i32,
}

/// `host->stream.snd.parents`.
#[derive(Debug, Default)]
pub struct Parents {
    /// In C's list order: the parent last connected to moves to the end.
    pub list: Vec<Parent>,
    /// `current`: the parent last connected to, an index into `list`.
    pub current: Option<usize>,
}

impl Parents {
    /// `rrdhost_stream_parents_update_from_destination()` from the destination's entries, each with its `:SSL`.
    pub fn new<'a>(entries: impl Iterator<Item = (&'a str, bool)>) -> Self {
        Parents { list: entries.map(|(d, ssl)| Parent::new(d, ssl)).collect(), current: None }
    }

    /// Whether one of them is reached over TLS.
    pub fn any_ssl(&self) -> bool {
        self.list.iter().any(|d| d.ssl)
    }

    /// `stream_parents_host_reset()`: every parent waits one draw of `[max(5, delay / 2), delay + 5)` seconds.
    pub fn reset(&mut self, reason: Reason, reconnect_delay_s: i64) {
        let until_ut = randomize_wait_ut(reconnect_delay_s / 2, reconnect_delay_s + 5);
        for d in &mut self.list {
            d.postpone_until_ut = until_ut;
            d.banned_for_this_session = false;
            d.reason = reason;
        }
    }

    pub fn current(&self) -> Option<&Parent> {
        self.current.and_then(|i| self.list.get(i))
    }

    /// `stream_parent_set_host_reconnect_delay()`.
    pub fn set_reconnect_delay(&mut self, reason: Reason, secs: i64) {
        if let Some(d) = self.current.and_then(|i| self.list.get_mut(i)) {
            d.set_reconnect_delay(reason, secs);
        }
    }

    /// `stream_parent_set_host_connect_failure_reason()`: the host's reason and pulse state, and the current parent's
    /// delay.
    pub fn set_connect_failure_reason(&mut self, host: &Host, status_reason: &mut Reason, reason: Reason, secs: i64) {
        *status_reason = reason;
        host.pulse_status(host_status::SND_NO_DST_FAILED);
        self.set_reconnect_delay(reason, secs);
    }

    /// `stream_parent_connect_to_one_unsafe()`: probes every usable parent, ranks them, and connects `sock` to the
    /// first that accepts; the connected parent becomes `current` and moves to the end of the list.
    #[allow(clippy::too_many_arguments)]
    pub fn connect_to_one(
        &mut self,
        sock: &mut NdSock,
        host: &Host,
        local: &Local,
        hops: i16,
        status_reason: &mut Reason,
        default_port: u16,
        timeout_s: i64,
        th: &Thread<'_>,
    ) -> bool {
        sock.error = SockError::NoDestinationAvailable;
        let hostname = host.hostname();
        for d in &mut self.list {
            d.selection = Selection { skipped: true, ..Selection::default() };
        }
        if self.list.is_empty() {
            log_errno!(th, Priority::Debug, "STREAM PARENTS '{hostname}': no parents configured");
            return false;
        }
        let now_ut = now_realtime_ut();
        let path = host.stream_path();
        let (mut skipped_but_useful, mut skipped_not_useful, mut potential) = (0usize, 0usize, 0usize);
        let mut array: Vec<usize> = Vec::with_capacity(self.list.len());
        for (index, d) in self.list.iter_mut().enumerate() {
            if th.cancelled() {
                sock.error = SockError::ThreadCancelled;
                return false;
            }
            // the parent's own nonce replaces it when its stream_info answers
            d.remote.nonce = os_random32();
            d.banned_temporarily_erroneous = is_blocked(&d.destination);
            if d.banned_permanently || d.banned_for_this_session {
                continue;
            }
            if d.banned_temporarily_erroneous {
                potential += 1;
                *status_reason = d.reason;
                continue;
            }
            if d.postpone_until_ut > now_ut {
                skipped_but_useful += 1;
                potential += 1;
                *status_reason = d.reason;
                log_errno!(
                    th,
                    Priority::Debug,
                    "STREAM PARENTS '{hostname}': skipping useful parent '{}': POSTPONED FOR {} SECS MORE: {}",
                    d.destination,
                    (d.postpone_until_ut - now_ut) / USEC_PER_SEC,
                    d.reason.text()
                );
                continue;
            }
            let tls = (sock.ctx.clone(), sock.verify);
            if stream_info_fetch(d, host.machine_guid(), default_port, &local.user_agent, &hostname, tls, th) {
                if matches!(d.remote.ingest_type(), IngestType::Virtual | IngestType::Localhost) {
                    d.reason = Reason::PARENT_IS_LOCALHOST;
                    d.since_ut = now_ut;
                    d.postpone_until_ut = randomize_wait_ut(3600, 7200);
                    d.banned_permanently = true;
                    skipped_not_useful += 1;
                    // hops 1: only when the parent is this node's origin
                    if is_in_stream_path_before_us(&local.host_id, &path, &d.remote.host_id, 1) {
                        log_errno!(
                            th,
                            Priority::Info,
                            "STREAM PARENTS '{hostname}': destination '{}' is banned permanently because it is the \
                             origin server",
                            d.destination
                        );
                    } else {
                        log_errno!(
                            th,
                            Priority::Warning,
                            "STREAM PARENTS '{hostname}': destination '{}' is banned permanently because it is the \
                             origin server, but it is not in the stream path before us!",
                            d.destination
                        );
                    }
                    continue;
                }
                match d.remote.ingest_status() {
                    IngestStatus::Initializing => {
                        d.reason = Reason::PARENT_IS_INITIALIZING;
                        d.since_ut = now_ut;
                        d.postpone_until_ut = randomize_wait_ut(30, 60);
                        skipped_but_useful += 1;
                        potential += 1;
                        *status_reason = d.reason;
                        log_errno!(
                            th,
                            Priority::Debug,
                            "STREAM PARENTS '{hostname}': skipping useful parent '{}': {}",
                            d.destination,
                            d.reason.text()
                        );
                        continue;
                    }
                    IngestStatus::Replicating | IngestStatus::Online
                        if is_in_stream_path_before_us(&local.host_id, &path, &d.remote.host_id, hops) =>
                    {
                        d.reason = Reason::PARENT_NODE_ALREADY_CONNECTED;
                        d.since_ut = now_ut;
                        d.postpone_until_ut = randomize_wait_ut(3600, 7200);
                        d.banned_for_this_session = true;
                        skipped_not_useful += 1;
                        log_errno!(
                            th,
                            Priority::Info,
                            "STREAM PARENTS '{hostname}': destination '{}' is banned for this session, because it is \
                             in our path before us.",
                            d.destination
                        );
                        continue;
                    }
                    _ => {}
                }
            }
            d.selection.skipped = false;
            d.selection.batch = array.len() + 1;
            d.selection.order = array.len() + 1;
            array.push(index);
        }
        let count = array.len();
        if count == 0 {
            log_errno!(
                th,
                Priority::Debug,
                "STREAM PARENTS '{hostname}': no parents available ({skipped_but_useful} skipped but useful, \
                 {skipped_not_useful} skipped not useful, {potential} potential)"
            );
            if potential == 0 {
                *status_reason = Reason::SP_NO_DESTINATION;
                host.pulse_status(host_status::SND_NO_DST);
            }
            return false;
        }
        if count > 1 {
            self.rank(&mut array, &hostname, th);
        } else {
            let d = &mut self.list[array[0]];
            d.selection.order = 1;
            d.selection.batch = 1;
            d.selection.random = false;
            log_errno!(th, Priority::Debug, "STREAM PARENTS '{hostname}': only 1 parent is available: '{}'", d.destination);
        }
        for (i, &index) in array.iter().enumerate() {
            if self.list[index].postpone_until_ut > now_ut {
                continue;
            }
            if th.cancelled() {
                sock.error = SockError::ThreadCancelled;
                *status_reason = Reason::DISCONNECT_SIGNALED_TO_STOP;
                host.pulse_status(host_status::SND_OFFLINE);
                return false;
            }
            let d = &mut self.list[index];
            log_errno!(
                th,
                Priority::Debug,
                "STREAM PARENTS '{hostname}': connecting to '{}' (default port: {default_port}, parent {} of {count})...",
                d.destination,
                i + 1
            );
            let service = effective_service(&d.destination, default_port);
            let _frame = push(vec![
                (Field::DstIp, Value::txt(d.destination.as_str())),
                (Field::DstPort, Value::txt(service.as_str())),
            ]);
            d.since_ut = now_ut;
            d.attempts += 1;
            host.pulse_status(host_status::SND_CONNECTING);
            if sock.connect_to_this(&d.destination, default_port, timeout_s, d.ssl, th) {
                let fd = sock.fd();
                // the connected parent goes last, so that a parent failing later does not stop the others
                let d = self.list.remove(index);
                let destination = d.destination.clone();
                self.list.push(d);
                self.current = Some(self.list.len() - 1);
                log_errno!(
                    th,
                    Priority::Debug,
                    "STREAM PARENTS '{hostname}': connected to '{destination}' (default port: {default_port}, fd {fd})..."
                );
                sock.error = SockError::None;
                *status_reason = Reason::SP_CONNECTED;
                host.pulse_status(host_status::SND_CONNECTING);
                return true;
            }
            d.sock_error_to_reason(sock.error);
            *status_reason = d.reason;
            host.pulse_status(host_status::SND_CONNECTING);
            log_errno!(
                th,
                Priority::Debug,
                "STREAM PARENTS '{hostname}': stream connection to '{}' failed (default port: {default_port}): {}",
                d.destination,
                sock.error.text()
            );
        }
        host.pulse_status(host_status::SND_OFFLINE);
        false
    }

    /// The ranking of `connect_to_one`: newest data first, then batches of parents whose newest data are within
    /// 120 s of the batch's first shuffled by coin flips.
    fn rank(&mut self, array: &mut [usize], hostname: &str, th: &Thread<'_>) {
        let list = &mut self.list;
        // qsort(compare_last_time); glibc's is a merge sort, stable as this one
        array.sort_by(|&a, &b| {
            let (a, b) = (&list[a], &list[b]);
            b.remote
                .db_last_time_s
                .cmp(&a.remote.db_last_time_s)
                .then(a.since_ut.cmp(&b.since_ut))
                .then(a.attempts.cmp(&b.attempts))
        });
        let count = array.len();
        let (mut base, mut batch) = (0, 0);
        while base < count {
            if list[array[base]].remote.nonce == 0 {
                list[array[base]].remote.nonce = os_random32();
            }
            let t_base = list[array[base]].remote.db_last_time_s;
            let mut similar = 1;
            for &i in &array[base + 1..] {
                let t_next = list[i].remote.db_last_time_s;
                if (t_next > t_base && t_next.wrapping_sub(t_base) <= SIMILAR_S) || t_base.wrapping_sub(t_next) <= SIMILAR_S {
                    similar += 1;
                } else {
                    break;
                }
            }
            if similar == 1 {
                let d = &mut list[array[base]];
                log_errno!(
                    th,
                    Priority::Debug,
                    "STREAM PARENTS '{hostname}': reordering keeps parent No {base}, '{}'",
                    d.destination
                );
                d.selection.order = base + 1;
                d.selection.batch = batch + 1;
                d.selection.random = false;
                base += 1;
                batch += 1;
                continue;
            }
            while similar > 1 {
                let mut chosen = base;
                for i in base + 1..base + similar {
                    let i_nonce = list[array[i]].remote.nonce | os_random32();
                    let chosen_nonce = list[array[chosen]].remote.nonce | os_random32();
                    if i_nonce > chosen_nonce {
                        chosen = i;
                    }
                }
                array.swap(base, chosen);
                let d = &mut list[array[base]];
                log_errno!(
                    th,
                    Priority::Debug,
                    "STREAM PARENTS '{hostname}': random reordering of {similar} similar parents (slots {base} to {}), \
                     No {base} is '{}'",
                    base + similar,
                    d.destination
                );
                d.selection.order = base + 1;
                d.selection.batch = batch + 1;
                d.selection.random = true;
                base += 1;
                similar -= 1;
            }
            let d = &mut list[array[base]];
            d.selection.order = base + 1;
            d.selection.batch = batch + 1;
            d.selection.random = true;
            base += 1;
            batch += 1;
        }
    }
}

/// `stream_info_json_parse_v1()` with path `""`: the members parsed before a failure stay in `remote`.
fn parse_v1(root: &serde_json::Value, remote: &mut Remote, error: &mut String) -> bool {
    let empty = serde_json::Map::new();
    let obj = root.as_object().unwrap_or(&empty);
    let req = Presence::Required;
    let Some(_version) = jsonc::uint64(obj, "", "version", req, error) else {
        return false;
    };
    let Some(status) = jsonc::uint64(obj, "", "status", req, error) else {
        return false;
    };
    remote.status = status as i32;
    let Some(host_id) = jsonc::uuid(obj, "", "host_id", req, error) else {
        return false;
    };
    remote.host_id = host_id;
    let Some(nodes) = jsonc::uint64(obj, "", "nodes", req, error) else {
        return false;
    };
    remote.nodes = nodes;
    let Some(receivers) = jsonc::uint64(obj, "", "receivers", req, error) else {
        return false;
    };
    remote.receivers = receivers;
    let Some(nonce) = jsonc::uint64(obj, "", "nonce", req, error) else {
        return false;
    };
    remote.nonce = nonce as u32;
    if remote.status == 200 {
        let Some(first) = jsonc::uint64(obj, "", "first_time_s", req, error) else {
            return false;
        };
        remote.db_first_time_s = first as i64;
        let Some(last) = jsonc::uint64(obj, "", "last_time_s", req, error) else {
            return false;
        };
        remote.db_last_time_s = last as i64;
        let enum_text = |member: &str, error: &mut String| jsonc::enum_text(obj, "", member, req, error);
        let Some(text) = enum_text("db_status", error) else {
            return false;
        };
        if let Some(text) = text {
            remote.db_status = Some(DbStatus::from_name(text));
        }
        let Some(text) = enum_text("db_liveness", error) else {
            return false;
        };
        if let Some(text) = text {
            remote.db_liveness = Some(DbLiveness::from_name(text));
        }
        let Some(text) = enum_text("ingest_type", error) else {
            return false;
        };
        if let Some(text) = text {
            remote.ingest_type = Some(IngestType::from_name(text));
        }
        let Some(text) = enum_text("ingest_status", error) else {
            return false;
        };
        if let Some(text) = text {
            remote.ingest_status = Some(IngestStatus::from_name(text));
        }
        return true;
    }
    error.push_str(&format!("status reported ({}) is not OK (200)", remote.status));
    remote.db_first_time_s = 0;
    remote.db_last_time_s = 0;
    remote.db_status = None;
    remote.db_liveness = None;
    remote.ingest_type = None;
    remote.ingest_status = None;
    false
}

/// `stream_info_fetch()`: `GET /api/v3/stream_info` of this host on its own connection (TLS as the sender's: its
/// context and verification), 5 s for each step. A failure at the socket level postpones the parent; one in the
/// answer only marks its reason. The connection's close at the function's end (`CLEAN_ND_SOCK`) leaves `errno` as
/// the close does: 0, or what a TLS shutdown set.
#[allow(clippy::too_many_arguments)]
fn stream_info_fetch(
    d: &mut Parent,
    machine_guid: &str,
    default_port: u16,
    user_agent: &str,
    hostname: &str,
    (ctx, verify): (Option<SslContext>, bool),
    th: &Thread<'_>,
) -> bool {
    let mut sock = NdSock::new(ctx, verify);
    let fetched = fetch(&mut sock, d, machine_guid, default_port, user_agent, hostname, th);
    sock.close(th);
    fetched
}

fn fetch(
    sock: &mut NdSock,
    d: &mut Parent,
    machine_guid: &str,
    default_port: u16,
    user_agent: &str,
    hostname: &str,
    th: &Thread<'_>,
) -> bool {
    let service = effective_service(&d.destination, default_port);
    let _frame = push(vec![
        (Field::DstIp, Value::txt(d.destination.as_str())),
        (Field::DstPort, Value::txt(service.as_str())),
        (Field::RequestMethod, Value::txt("GET")),
    ]);
    let request = format!(
        "GET /api/v3/stream_info?machine_guid={machine_guid} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {user_agent}\r\n\
         Accept: */*\r\nAccept-Encoding: identity\r\nTE: identity\r\nPragma: no-cache\r\nCache-Control: no-cache\r\n\
         Connection: close\r\n\r\n",
        d.destination
    );
    log_errno!(th, Priority::Debug, "STREAM PARENTS '{hostname}': fetching stream info from '{}'...", d.destination);
    d.reason = Reason::SP_CONNECTING;
    if !sock.connect_to_this(&d.destination, default_port, PROBE_TIMEOUT_S, d.ssl, th) {
        d.selection.info = false;
        d.sock_error_to_reason(sock.error);
        log_errno!(
            th,
            Priority::Warning,
            "STREAM PARENTS '{hostname}': failed to connect for stream info to '{}': {}",
            d.destination,
            sock.error.text()
        );
        return false;
    }
    // snprintf() into the 8 KiB buffer
    let request = &request.as_bytes()[..request.len().min(HTTP_HEADER_SIZE - 1)];
    if sock.send_timeout(request, PROBE_TIMEOUT_S, th) <= 0 {
        d.selection.info = false;
        d.sock_error_to_reason(sock.error);
        log_errno!(
            th,
            Priority::Warning,
            "STREAM PARENTS '{hostname}': failed to send stream info request to '{}': {}",
            d.destination,
            sock.error.text()
        );
        return false;
    }
    let mut buf = vec![0u8; HTTP_HEADER_SIZE];
    let (mut total, mut payload_received, mut content_length) = (0usize, 0usize, 0usize);
    let mut payload_start = None;
    // C's condition: the loop ends at the first payload bytes unless they exceed the content length
    while payload_received == 0 || content_length < payload_received {
        let remaining = HTTP_HEADER_SIZE - total;
        if remaining <= 1 {
            log_errno!(
                th,
                Priority::Warning,
                "STREAM PARENTS '{hostname}': stream info receive buffer is full while receiving response from '{}'",
                d.destination
            );
            d.selection.info = false;
            d.reason = Reason::PARENT_INTERNAL_ERROR;
            return false;
        }
        let received = sock.recv_timeout(&mut buf[total..HTTP_HEADER_SIZE - 1], PROBE_TIMEOUT_S, th);
        if received <= 0 {
            log_errno!(
                th,
                Priority::Warning,
                "STREAM PARENTS '{hostname}': socket receive error while querying stream info on '{}' (total \
                 received {total}, payload received {payload_received}, content length {content_length}): {}",
                d.destination,
                sock.error.text()
            );
            d.selection.info = false;
            d.sock_error_to_reason(sock.error);
            return false;
        }
        total += received as usize;
        buf[total] = 0;
        let text = c_str(&buf[..total]);
        let start = match payload_start {
            Some(start) => start,
            None => match netdata_agent_text::c::find(text, b"\r\n\r\n") {
                Some(end) => *payload_start.insert(end + 4),
                None => continue,
            },
        };
        payload_received = total - start;
        if content_length == 0 {
            let Some(at) = netdata_agent_text::c::find(text, b"Content-Length: ") else {
                log_errno!(
                    th,
                    Priority::Warning,
                    "STREAM PARENTS '{hostname}': stream info response from '{}' does not have a Content-Length",
                    d.destination
                );
                d.selection.info = false;
                d.reason = Reason::PARENT_INTERNAL_ERROR;
                return false;
            };
            // strtoul()
            content_length =
                netdata_agent_text::parse::strtoull10(&text[at + b"Content-Length: ".len()..]).0 as usize;
            if content_length == 0 {
                log_errno!(
                    th,
                    Priority::Warning,
                    "STREAM PARENTS '{hostname}': stream info response from '{}' has invalid Content-Length",
                    d.destination
                );
                d.selection.info = false;
                d.reason = Reason::PARENT_INTERNAL_ERROR;
                return false;
            }
        }
    }
    let payload = c_str(&buf[payload_start.unwrap_or(total)..total]);
    let shown = String::from_utf8_lossy(payload);
    let Some(root) = jsonc::tokener_parse(payload) else {
        d.selection.info = false;
        d.reason = Reason::SP_NO_STREAM_INFO;
        log_errno!(
            th,
            Priority::Warning,
            "STREAM PARENTS '{hostname}': failed to parse stream info response from '{}', JSON data: {shown}",
            d.destination
        );
        return false;
    };
    let mut error = String::new();
    if !parse_v1(&root, &mut d.remote, &mut error) {
        d.selection.info = false;
        d.reason = Reason::SP_NO_STREAM_INFO;
        log_errno!(
            th,
            Priority::Warning,
            "STREAM PARENTS '{hostname}': failed to extract fields from JSON stream info response from '{}': {error} \
             - JSON data: {shown}",
            d.destination
        );
        return false;
    }
    let r = &d.remote;
    log_errno!(
        th,
        Priority::Debug,
        "STREAM PARENTS '{hostname}': received stream_info data from '{}': status: {}, nodes: {}, receivers: {}, \
         first_time_s: {}, last_time_s: {}, db status: {}, db liveness: {}, ingest type: {}, ingest status: {}",
        d.destination,
        r.status,
        r.nodes,
        r.receivers,
        r.db_first_time_s,
        r.db_last_time_s,
        r.db_status.unwrap_or(DbStatus::Initializing).name(),
        r.db_liveness.unwrap_or(DbLiveness::Stale).name(),
        r.ingest_type().name(),
        r.ingest_status().name()
    );
    d.selection.info = true;
    d.reason = Reason::NEVER;
    true
}

#[cfg(test)]
mod tests;
