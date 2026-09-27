//! `pulse-network.c`: the web API's and the streaming's traffic, each created once it has bytes. The statsd traffic
//! (statsd is not ported) and the ACLK charts (the cloud connection is not) are left out.

use std::sync::Arc;

use netdata_agent_rrd::chart::{Algorithm, Chart, ChartType, Dim};
use netdata_agent_rrd::labels::SRC_AUTO;

use crate::chart::{Def, Localhost, dim, set};

/// `BITS_IN_A_KILOBIT`.
const BITS_IN_A_KILOBIT: i32 = 1000;

#[derive(Default)]
pub(crate) struct Charts {
    api: Option<Traffic>,
    streaming: Option<Traffic>,
}

struct Traffic {
    chart: Arc<Chart>,
    received: Arc<Dim>,
    sent: Arc<Dim>,
}

/// A traffic chart of `netdata.network`, labelled by its endpoint.
fn update(
    slot: &mut Option<Traffic>,
    localhost: &Localhost<'_>,
    id: &str,
    endpoint: &str,
    (received, sent): (u64, u64),
) {
    let traffic = slot.get_or_insert_with(|| {
        let chart = localhost.create(&Def {
            id,
            family: "Network Traffic",
            context: Some("netdata.network"),
            title: "Netdata Network Traffic",
            units: "kilobits/s",
            module: "pulse",
            priority: 130150,
            chart_type: ChartType::Area,
        });
        chart.update_meta(|m| m.labels.add(b"endpoint", endpoint.as_bytes(), SRC_AUTO));
        Traffic {
            received: dim(&chart, "in", 8, BITS_IN_A_KILOBIT, Algorithm::Incremental),
            sent: dim(&chart, "out", -8, BITS_IN_A_KILOBIT, Algorithm::Incremental),
            chart,
        }
    });
    set(&traffic.received, received as i64);
    set(&traffic.sent, sent as i64);
    localhost.done(&traffic.chart);
}

impl Charts {
    /// `pulse_network_do()`.
    pub fn update(&mut self, localhost: &Localhost<'_>) {
        let stats = localhost.host.storage().pulse().network.read();
        if stats.api_received != 0 || stats.api_sent != 0 {
            update(
                &mut self.api,
                localhost,
                "network_api",
                "web-server",
                (stats.api_received, stats.api_sent),
            );
        }
        if stats.stream_received != 0 || stats.stream_sent != 0 {
            update(
                &mut self.streaming,
                localhost,
                "network_streaming",
                "streaming",
                (stats.stream_received, stats.stream_sent),
            );
        }
    }
}
