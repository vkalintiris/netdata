//! The sender's hooks (`src/streaming/stream-sender.c`, `protocol/command-*.c`): on connect (on the connector
//! thread), ready to dispatch (on the stream thread), on disconnect (the connector's remove), the egress interface
//! label, the metadata the child sends first, and NODE_ID from the parent (`protocol/command-nodeid.c`). Map:
//! `knowledge/map-m7-commit4-runtime.md` §5 (NODE_ID), §6, §7.

use std::sync::atomic::Ordering;

use netdata_agent_evloop::conn::Conn;
use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_tls::Link;
use netdata_agent_rrd::host::{Host, sender_flags};
use netdata_agent_rrd::labels::SRC_AUTO;
use netdata_agent_rrd::upstream;
use netdata_agent_text::parse::uuid_parse_flexi;

use super::dispatch::Dispatched;
use super::{Sender, Traffic, shown, text};
use crate::caps;

/// `OS_IFNAME_MAX`: the name is cut to one byte less.
const IFNAME_MAX: usize = 32;

impl Sender {
    /// `host->stream.snd.status.replication.counter_in++`.
    pub(crate) fn replication_counter_in(&self) {
        self.counter_in.fetch_add(1, Ordering::Relaxed);
    }

    /// `stream_sender_on_connect()`, on the connector thread: connected, reset, and the egress interface recorded
    /// before the first labels go out.
    pub(crate) fn on_connect(&self, host: &Host, link: &Link<Conn>) {
        nd_log!(Source::Daemon, Priority::Debug, "STREAM SND [{}]: running on-connect hooks...", host.hostname());
        host.sender_flags_set(sender_flags::CONNECTED);
        self.on_connect_and_disconnect(host);
        if let Some(iface) = link.socket().and_then(|c| egress_interface(&socket2::SockRef::from(c))) {
            host.update_labels(|l| {
                let _ = l.add_changed(b"_net_default_iface", iface.as_bytes(), SRC_AUTO);
            });
        }
    }

    /// `stream_sender_on_disconnect()`, on the connector's remove: not ready (before the reset, so a definition in
    /// flight gives its claim back), reset, and our path cut after this agent.
    pub(crate) fn on_disconnect(&self, host: &Host) {
        nd_log!(Source::Daemon, Priority::Debug, "STREAM SND '{}': running on-disconnect hooks...", host.hostname());
        host.sender_flags_clear(sender_flags::READY_4_METRICS);
        self.on_connect_and_disconnect(host);
        // stream_path_parent_disconnected(): the entries after this agent's go (sent to a child with the proxy)
        host.cut_stream_path_after(self.connector.local().host_id);
    }

    /// `stream_sender_on_connect_and_disconnect()`: the pending replication requests flushed and the charts' state
    /// reset (`stream_sender_charts_and_replication_reset()`), the counters zeroed and the buffer flushed (the
    /// executor's state is the connection's own).
    fn on_connect_and_disconnect(&self, host: &Host) {
        self.connector.replication().delete_pending(&self.replication);
        upstream::reset_charts(host);
        self.replication.replicating_zero();
        self.counter_in.store(0, Ordering::Relaxed);
        self.counter_out.store(0, Ordering::Relaxed);
        self.flush_buffer(&mut self.out());
    }

    /// `stream_sender_on_ready_to_dispatch()`: ready, then the host's metadata, each its own commit.
    pub(crate) fn on_ready_to_dispatch(&self, host: &Host, capabilities: u32) {
        nd_log!(Source::Daemon, Priority::Debug, "STREAM SND '{}': running ready-to-dispatch hooks...", host.hostname());
        host.sender_flags_set(sender_flags::READY_4_METRICS);
        upstream::send_host_variables(host);
        self.send_path(host, capabilities);
        upstream::send_claimed_id(host);
        upstream::send_host_labels(host);
        upstream::send_global_functions(host);
    }

    /// `stream_path_send_to_parent()`.
    pub(crate) fn send_path(&self, host: &Host, capabilities: u32) {
        if capabilities & caps::PATHS == 0 || !host.can_stream_metadata() {
            return;
        }
        let Some(localhost) = self.connector.localhost() else {
            return;
        };
        let message = netdata_agent_ingest::stream_path::message(host, &localhost, None);
        self.commit(&message, Traffic::Metadata);
    }
}

/// `os_socket_egress_interface()`: the first interface whose address is the socket's local one (a v4-mapped address
/// against the IPv4 ones, a link-local IPv6 one with its scope), cut to 31 bytes.
fn egress_interface(socket: &socket2::SockRef<'_>) -> Option<String> {
    use std::net::{IpAddr, SocketAddr};
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
            return Some(name).filter(|n| !n.is_empty());
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
