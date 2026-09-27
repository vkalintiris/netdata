//! The agent's counters for its own pulse charts (`daemon/pulse/`): what the web server, the stream receiver and the
//! query engine count as they work, read once a second by the PULSE thread. They live in the storage layout, with
//! C's other process globals (D77.2, D80.5).

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

/// Every counter group.
#[derive(Debug, Default)]
pub struct Pulse {
    pub web: Web,
    pub network: Network,
}

/// `web_statistics` (`pulse-http-api.c`): the web server's clients and completed requests.
#[derive(Debug, Default)]
pub struct Web {
    connected_clients: AtomicI64,
    requests: AtomicU64,
    usec: AtomicU64,
    usec_max: AtomicU64,
    content_size_uncompressed: AtomicU64,
    content_size_compressed: AtomicU64,
}

/// A read of [`Web`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WebStats {
    pub connected_clients: i64,
    pub requests: u64,
    pub usec: u64,
    pub usec_max: u64,
    pub content_size_uncompressed: u64,
    pub content_size_compressed: u64,
}

impl Web {
    /// `pulse_web_client_connected()`.
    pub fn client_connected(&self) {
        self.connected_clients.fetch_add(1, Ordering::Relaxed);
    }

    /// `pulse_web_client_disconnected()`.
    pub fn client_disconnected(&self) {
        self.connected_clients.fetch_sub(1, Ordering::Relaxed);
    }

    /// `pulse_web_request_completed()`: a request that took `dt_usec`, with its content size before and after
    /// compression.
    pub fn request_completed(&self, dt_usec: u64, content_size: u64, compressed_content_size: u64) {
        self.usec_max.fetch_max(dt_usec, Ordering::Relaxed);
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.usec.fetch_add(dt_usec, Ordering::Relaxed);
        self.content_size_uncompressed
            .fetch_add(content_size, Ordering::Relaxed);
        self.content_size_compressed
            .fetch_add(compressed_content_size, Ordering::Relaxed);
    }

    /// `pulse_web_copy()`: the counters; with `reset_max` the maximum starts again from 0 unless a request raised it
    /// since it was read.
    pub fn read(&self, reset_max: bool) -> WebStats {
        let stats = WebStats {
            connected_clients: self.connected_clients.load(Ordering::Relaxed),
            requests: self.requests.load(Ordering::Relaxed),
            usec: self.usec.load(Ordering::Relaxed),
            usec_max: self.usec_max.load(Ordering::Relaxed),
            content_size_uncompressed: self.content_size_uncompressed.load(Ordering::Relaxed),
            content_size_compressed: self.content_size_compressed.load(Ordering::Relaxed),
        };
        if reset_max {
            let _ = self.usec_max.compare_exchange(
                stats.usec_max,
                0,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
        stats
    }
}

/// `network_statistics` (`pulse-network.c`): the bytes of the web API and of streaming, both directions.
#[derive(Debug, Default)]
pub struct Network {
    api_received: AtomicU64,
    api_sent: AtomicU64,
    stream_received: AtomicU64,
    stream_sent: AtomicU64,
}

/// A read of [`Network`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NetworkStats {
    pub api_received: u64,
    pub api_sent: u64,
    pub stream_received: u64,
    pub stream_sent: u64,
}

impl Network {
    /// `pulse_web_server_received_bytes()`.
    pub fn api_received(&self, bytes: usize) {
        self.api_received.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// `pulse_web_server_sent_bytes()`.
    pub fn api_sent(&self, bytes: usize) {
        self.api_sent.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// `pulse_stream_received_bytes()`.
    pub fn stream_received(&self, bytes: usize) {
        self.stream_received
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// `pulse_stream_sent_bytes()`.
    pub fn stream_sent(&self, bytes: usize) {
        self.stream_sent.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// `pulse_network_copy()`.
    pub fn read(&self) -> NetworkStats {
        NetworkStats {
            api_received: self.api_received.load(Ordering::Relaxed),
            api_sent: self.api_sent.load(Ordering::Relaxed),
            stream_received: self.stream_received.load(Ordering::Relaxed),
            stream_sent: self.stream_sent.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The web counters as C keeps them: clients up and down, requests with their time and sizes, the maximum reset
    /// at a read unless a request raised it in between.
    #[test]
    fn web_counters_follow_c() {
        let web = Web::default();
        web.client_connected();
        web.client_connected();
        web.client_disconnected();
        web.request_completed(300, 1000, 400);
        web.request_completed(100, 50, 50);
        assert_eq!(
            web.read(true),
            WebStats {
                connected_clients: 1,
                requests: 2,
                usec: 400,
                usec_max: 300,
                content_size_uncompressed: 1050,
                content_size_compressed: 450,
            }
        );
        assert_eq!(web.read(false).usec_max, 0, "reset by the read");
        web.request_completed(20, 0, 0);
        assert_eq!(web.read(false).usec_max, 20);
    }

    #[test]
    fn network_counters_add_up() {
        let net = Network::default();
        net.api_received(10);
        net.api_sent(20);
        net.stream_received(30);
        net.stream_sent(40);
        net.api_sent(1);
        assert_eq!(
            net.read(),
            NetworkStats {
                api_received: 10,
                api_sent: 21,
                stream_received: 30,
                stream_sent: 40,
            }
        );
    }
}
