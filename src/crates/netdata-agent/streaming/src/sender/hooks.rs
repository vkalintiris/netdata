//! The sender's hooks (`src/streaming/stream-sender.c`, `protocol/command-*.c`): on connect (on the connector
//! thread), ready to dispatch (on the stream thread), on disconnect (the connector's remove), the egress interface
//! label, the metadata the child sends first, and NODE_ID from the parent (`protocol/command-nodeid.c`). Map:
//! `knowledge/map-m7-commit4-runtime.md` §5 (NODE_ID), §6, §7.

use std::os::fd::AsRawFd;
use std::sync::atomic::Ordering;

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_pluginsd_proto::emit::stream as emit;
use netdata_agent_rrd::chart::flags;
use netdata_agent_rrd::host::{Host, sender_flags};
use netdata_agent_rrd::labels::SRC_AUTO;
use netdata_agent_text::parse::uuid_parse_flexi;

use super::dispatch::Dispatched;
use super::{Sender, Traffic};
use crate::caps;
use crate::receiver::now_monotonic_ut;

/// `OS_IFNAME_MAX`: the name is cut to one byte less.
const IFNAME_MAX: usize = 32;

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

impl Sender {
    /// `rrdhost_can_stream_metadata_to_parent()`: the sender runs, is ready, and the host collects.
    pub(crate) fn can_stream_metadata(&self, host: &Host) -> bool {
        host.sender_flags() & sender_flags::READY_4_METRICS != 0 && host.is_online()
    }

    /// `host->stream.snd.status.replication.counter_in++`.
    pub(crate) fn replication_counter_in(&self) {
        self.counter_in.fetch_add(1, Ordering::Relaxed);
    }

    /// `stream_sender_on_connect()`, on the connector thread: connected, reset, and the egress interface recorded
    /// before the first labels go out.
    pub(crate) fn on_connect(&self, host: &Host, socket: &socket2::Socket) {
        nd_log!(Source::Daemon, Priority::Debug, "STREAM SND [{}]: running on-connect hooks...", host.hostname());
        host.sender_flags_set(sender_flags::CONNECTED);
        self.on_connect_and_disconnect(host);
        if let Some(iface) = egress_interface(socket) {
            host.update_labels(|l| {
                let _ = l.add_changed(b"_net_default_iface", iface.as_bytes(), SRC_AUTO);
            });
        }
    }

    /// `stream_sender_on_disconnect()`, on the connector's remove: not ready, reset, and our path cut after this agent.
    pub(crate) fn on_disconnect(&self, host: &Host) {
        nd_log!(Source::Daemon, Priority::Debug, "STREAM SND '{}': running on-disconnect hooks...", host.hostname());
        host.sender_flags_clear(sender_flags::READY_4_METRICS);
        self.on_connect_and_disconnect(host);
        // stream_path_parent_disconnected(): the entries after this agent's go (sent to a child with the proxy)
        let me = self.connector.local().host_id;
        let mut path = host.stream_path();
        if let Some(at) = path.iter().position(|p| p.host_id == me) {
            path.truncate(at + 1);
            host.replace_stream_path(path);
        }
    }

    /// `stream_sender_on_connect_and_disconnect()`: the charts' replication state reset and the buffer flushed (the
    /// executor's state is the connection's own).
    fn on_connect_and_disconnect(&self, host: &Host) {
        // stream_sender_charts_and_replication_reset(), as far as the sender runtime has it (commit 5 adds the
        // exposure and the claims)
        for chart in host.charts().all() {
            chart.update_meta(|m| {
                m.flags |= flags::SENDER_REPLICATION_FINISHED;
                m.flags &= !flags::SENDER_REPLICATION_IN_PROGRESS;
            });
        }
        self.counter_in.store(0, Ordering::Relaxed);
        self.counter_out.store(0, Ordering::Relaxed);
        let max = self.connector.settings.buffer_max_size;
        self.out().buffer.flush(max, now_monotonic_ut());
    }

    /// `stream_sender_on_ready_to_dispatch()`: ready, then the host's metadata, each its own commit.
    pub(crate) fn on_ready_to_dispatch(&self, host: &Host, capabilities: u32) {
        nd_log!(Source::Daemon, Priority::Debug, "STREAM SND '{}': running ready-to-dispatch hooks...", host.hostname());
        host.sender_flags_set(sender_flags::READY_4_METRICS);
        self.send_host_variables(host);
        self.send_path(host, capabilities);
        self.send_claimed_id(host, capabilities);
        self.send_host_labels(host, capabilities);
        self.send_global_functions(host, capabilities);
    }

    /// `stream_sender_send_custom_host_variables()`: `VARIABLE HOST` per variable.
    fn send_host_variables(&self, host: &Host) {
        if !self.can_stream_metadata(host) {
            return;
        }
        let mut out = Vec::new();
        for (name, value) in host.variables() {
            emit::variable(&mut out, emit::VarScope::Host, &name, value);
        }
        self.commit(&out, Traffic::Metadata);
    }

    /// `stream_path_send_to_parent()`.
    pub(crate) fn send_path(&self, host: &Host, capabilities: u32) {
        if capabilities & caps::PATHS == 0 || !self.can_stream_metadata(host) {
            return;
        }
        let Some(localhost) = self.connector.localhost() else {
            return;
        };
        let message = netdata_agent_ingest::stream_path::message(host, &localhost, None);
        self.commit(&message, Traffic::Metadata);
    }

    /// `stream_sender_send_claimed_id()`.
    fn send_claimed_id(&self, host: &Host, capabilities: u32) {
        if capabilities & caps::CLAIM == 0 || !self.can_stream_metadata(host) {
            return;
        }
        let mut out = Vec::new();
        emit::claimed_id(&mut out, host.machine_guid(), host.claim_id().as_ref());
        self.commit(&out, Traffic::Metadata);
    }

    /// `stream_send_host_labels()`: every label with its source, then `OVERWRITE labels` even with none.
    pub(crate) fn send_host_labels(&self, host: &Host, capabilities: u32) {
        if !self.can_stream_metadata(host) || capabilities & caps::HLABELS == 0 {
            return;
        }
        let mut out = Vec::new();
        for label in host.labels().iter() {
            emit::label(&mut out, &text(&label.name), label.flags, &text(&label.value));
        }
        emit::overwrite_labels(&mut out);
        self.commit(&out, Traffic::Metadata);
    }

    /// `stream_send_global_functions()`: the host's functions (the full catalogue, FUNCTION_DEL and DynCfg's line,
    /// comes with the functions milestone, M8; the lines are masked in the checks until then, D100.9).
    fn send_global_functions(&self, host: &Host, capabilities: u32) {
        if capabilities & caps::FUNCTIONS == 0 || !self.can_stream_metadata(host) {
            return;
        }
        let mut out = Vec::new();
        for (name, m) in host.functions().all() {
            let tags = if m.tags.is_empty() { "top".to_string() } else { text(&m.tags) };
            let priority = if m.priority == 0 { 100 } else { m.priority };
            emit::function_global(&mut out, &text(&name), m.timeout_s, &text(&m.help), &tags, m.access, priority, m.version);
        }
        self.commit(&out, Traffic::Metadata);
    }
}

/// `os_socket_egress_interface()`: the first interface whose address is the socket's local one (a v4-mapped address
/// against the IPv4 ones, a link-local IPv6 one with its scope), cut to 31 bytes.
fn egress_interface(socket: &socket2::Socket) -> Option<String> {
    use std::net::{IpAddr, SocketAddr};
    let _ = socket.as_raw_fd();
    let local = socket.local_addr().ok()?.as_socket()?;
    for ifa in nix::ifaddrs::getifaddrs().ok()? {
        let Some(addr) = ifa.address else {
            continue;
        };
        let matched = match local {
            SocketAddr::V4(l) => addr.as_sockaddr_in().is_some_and(|a| a.ip() == *l.ip()),
            SocketAddr::V6(l) => match l.ip().to_ipv4_mapped() {
                Some(v4) => addr.as_sockaddr_in().is_some_and(|a| a.ip() == v4),
                None => addr.as_sockaddr_in6().is_some_and(|a| {
                    a.ip() == *l.ip()
                        && (!matches!(IpAddr::V6(*l.ip()), IpAddr::V6(ip) if ip.segments()[0] & 0xffc0 == 0xfe80)
                            || a.scope_id() == l.scope_id())
                }),
            },
        };
        if matched {
            let mut name = ifa.interface_name;
            let mut end = name.len().min(IFNAME_MAX - 1);
            while !name.is_char_boundary(end) {
                end -= 1;
            }
            name.truncate(end);
            return Some(name);
        }
    }
    None
}

/// `stream_sender_get_node_and_claim_id_from_parent()`: the parent's claim id, and, while this agent is not claimed
/// and connected itself, the node id and the cloud URL the parent has.
pub(crate) fn node_and_claim_id_from_parent(
    d: &mut Dispatched,
    claim: Option<&[u8]>,
    node: Option<&[u8]>,
    url: Option<&[u8]>,
) {
    let host = &d.host;
    let hostname = host.hostname();
    let remote = &d.remote_ip;
    let env = d.sender.connector.env();
    let claimed = (env.claimed)();
    let shown = |w: Option<&[u8]>| w.map_or_else(|| "(unset)".to_string(), text);
    let Some(claim_id) = uuid_parse_flexi(claim.unwrap_or_default()) else {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "STREAM SND '{hostname}' [to {remote}] [PCLAIMID]: received invalid claim id '{}'",
            shown(claim)
        );
        return;
    };
    if claim_id == [0; 16] {
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "STREAM SND '{hostname}' [to {remote}] [PCLAIMID]: received zero claim id '{}'",
            shown(claim)
        );
        return;
    }
    let Some(node_id) = uuid_parse_flexi(node.unwrap_or_default()) else {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "STREAM SND '{hostname}' [to {remote}] [PCLAIMID] received an invalid node id '{}'",
            shown(node)
        );
        return;
    };
    if node_id == [0; 16] {
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "STREAM SND '{hostname}' [to {remote}] [PCLAIMID]: received zero node id '{}'",
            shown(node)
        );
        return;
    }
    let Some(url) = url.filter(|u| !u.is_empty()) else {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "STREAM SND '{hostname}' [to {remote}] [PCLAIMID] received an invalid cloud URL '{}'",
            shown(url)
        );
        return;
    };
    if let Some(previous) = host.update_claim_id_of_parent(claim_id) {
        let (verb, was) = if previous == [0; 16] { ("set", "was empty") } else { ("changed", "was set") };
        nd_log!(
            Source::Daemon,
            Priority::Info,
            "STREAM SND '{hostname}' [to {remote}] [PCLAIMID] {verb} parent's claim id to {} ({was})",
            shown(claim)
        );
    }
    let current = host.node_id();
    let mut update_node_id = false;
    if current != [0; 16] && current != node_id {
        if claimed {
            nd_log!(
                Source::Daemon,
                Priority::Warning,
                "STREAM SND '{hostname}' [to {remote}] [PCLAIMID] parent reports different node id '{}', but we are \
                 claimed. Ignoring it.",
                shown(node)
            );
        } else {
            update_node_id = true;
            nd_log!(
                Source::Daemon,
                Priority::Warning,
                "STREAM SND '{hostname}' [to {remote}] [PCLAIMID] changed node id to {}",
                shown(node)
            );
        }
    }
    if claimed && (env.aclk_online)() {
        return;
    }
    if current == [0; 16] || update_node_id {
        host.set_node_id(node_id);
        (env.set_cloud_url)(&text(url));
        // stream_path_node_id_updated(); the node id goes down to children with the proxy
        let caps = d.capabilities;
        d.sender.send_path(&d.host, caps);
    }
}
