//! `pulse-http-api.c`: the web API's clients, requests and response time (the compression chart is extended).

use std::sync::Arc;

use netdata_agent_rrd::chart::{Algorithm, Chart, ChartType, Dim};

use crate::chart::{Def, Localhost, dim, set};

pub(crate) struct Charts {
    clients: Option<(Arc<Chart>, Arc<Dim>)>,
    requests: Option<(Arc<Chart>, Arc<Dim>)>,
    response_time: Option<(Arc<Chart>, Arc<Dim>, Arc<Dim>)>,
    /// The totals of the last cycle, and the average kept while no request completes (-1 before the first).
    old_requests: u64,
    old_usec: u64,
    average: i64,
}

impl Charts {
    pub fn new() -> Self {
        Charts {
            clients: None,
            requests: None,
            response_time: None,
            old_requests: 0,
            old_usec: 0,
            average: -1,
        }
    }

    /// `pulse_web_do()`: the counters with the response time's max reset.
    pub fn update(&mut self, localhost: &Localhost<'_>) {
        let stats = localhost.host.storage().pulse().web.read(true);
        let def = |id, context, title, units, priority| Def {
            id,
            family: "HTTP API",
            context: Some(context),
            title,
            units,
            module: "pulse",
            priority,
            chart_type: ChartType::Line,
        };

        let (chart, clients) = self.clients.get_or_insert_with(|| {
            let chart = localhost.create(&def(
                "clients",
                "netdata.http_api_clients",
                "Netdata Web API Clients",
                "connected clients",
                130200,
            ));
            let clients = dim(&chart, "clients", 1, 1, Algorithm::Absolute);
            (chart, clients)
        });
        set(clients, stats.connected_clients);
        localhost.done(chart);

        let (chart, requests) = self.requests.get_or_insert_with(|| {
            let chart = localhost.create(&def(
                "requests",
                "netdata.http_api_requests",
                "Netdata Web API Requests Received",
                "requests/s",
                130300,
            ));
            let requests = dim(&chart, "requests", 1, 1, Algorithm::Incremental);
            (chart, requests)
        });
        set(requests, stats.requests as i64);
        localhost.done(chart);

        let (chart, average, max) = self.response_time.get_or_insert_with(|| {
            let chart = localhost.create(&def(
                "response_time",
                "netdata.http_api_response_time",
                "Netdata Web API Response Time",
                "milliseconds/request",
                130500,
            ));
            let average = dim(&chart, "average", 1, 1000, Algorithm::Absolute);
            let max = dim(&chart, "max", 1, 1000, Algorithm::Absolute);
            (chart, average, max)
        });
        let usec = stats.usec.saturating_sub(self.old_usec);
        let requests = stats.requests.saturating_sub(self.old_requests);
        self.old_usec = stats.usec;
        self.old_requests = stats.requests;
        if let Some(average) = usec.checked_div(requests) {
            self.average = average as i64;
        }
        set(average, if self.average != -1 { self.average } else { 0 });
        let peak = if stats.usec_max != 0 {
            stats.usec_max as i64
        } else {
            self.average
        };
        set(max, peak);
        localhost.done(chart);
    }
}
