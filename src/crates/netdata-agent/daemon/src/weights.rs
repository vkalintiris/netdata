//! `/api/v1/weights` and `/api/v1/metric_correlations`: `web_client_api_request_weights()`
//! (`src/web/api/v2/api_v2_weights.c`) with the two commands' defaults (`src/web/api/v1/api_v1_weights.c`). The
//! numbers and the formats are `netdata_agent_query::weights`.

use std::sync::Arc;

use netdata_agent_query::weights::Method;
use netdata_agent_query::weights::engine::{Env, Finished, NO_RESULTS, run};
use netdata_agent_query::weights::parse::{Format, LIMIT_ERROR, parse};
use netdata_agent_query::weights::v1;
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::host::Host;
use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::progress::Table;
use netdata_agent_web::status;

use crate::data::profile_of;
use crate::router::{Route, not_ready};
use crate::server::Reply;

/// A format's writer: the body and how many dimensions it printed.
type Writer = fn(&Finished, usize) -> (Vec<u8>, usize);

/// `api_v1_weights()`: the anomaly rates, by context.
pub fn v1_weights(route: &Route<'_>, _host: &Arc<Host>, query: &[u8]) -> Reply {
    answer(route, query, Method::AnomalyRate, Format::Contexts, v1::contexts)
}

/// `api_v1_metric_correlations()`: the two-sample Kolmogorov-Smirnov test, by chart.
pub fn v1_metric_correlations(route: &Route<'_>, _host: &Arc<Host>, query: &[u8]) -> Reply {
    answer(route, query, Method::Ks2, Format::Charts, v1::charts)
}

/// `web_client_api_request_weights()` for version 1. Until the agent is ready: 503 with the request as the body.
/// A limit that is no number: 400 with C's text, whatever else the query holds. Otherwise the engine over every
/// host (the routed host is not looked at: C gives a version-1 request none), its refusal or the format's
/// answer; an answer without a dimension is the 404 of version 1. The reply may be cached when both ends of the
/// highlighted window were absolute.
fn answer(route: &Route<'_>, query: &[u8], method: Method, format: Format, write: Writer) -> Reply {
    if let Some(reply) = not_ready(route) {
        return reply;
    }
    let json =
        |code: u16, body: Vec<u8>| Reply { code, content_type: ContentType::ApplicationJson, body, ..Reply::default() };
    let storage = route.shared.hosts.storage();
    let profile = profile_of(storage);
    let Ok(req) = parse(query, 1, method, format, profile.storage_tiers) else {
        return json(status::BAD_REQUEST, LIMIT_ERROR.as_bytes().to_vec());
    };
    let env = Env {
        hosts: route.shared.hosts.all(),
        cpus: route.shared.cpus,
        profile: &profile,
        windows: route.shared.grouping_windows,
        queries: Some(&storage.pulse().queries),
        now_s: now_realtime_s(),
        interrupted: route.interrupted,
        progress: Some(Table::process().tracker(route.ctx.transaction)),
    };
    let outcome = run(req, &env);
    let storage_tiers = usize::try_from(profile.storage_tiers).unwrap_or(usize::MAX);
    let (code, body) = match &outcome.result {
        Err(refusal) => (refusal.code, refusal.body()),
        Ok(finished) => match write(finished, storage_tiers) {
            (_, 0) => (NO_RESULTS.code, NO_RESULTS.body()),
            (body, _) => (status::OK, body),
        },
    };
    Reply { no_cacheable: !outcome.cacheable, ..json(code, body) }
}
