//! The weights endpoints: `web_client_api_request_weights()` (`src/web/api/v2/api_v2_weights.c`) with each
//! command's defaults: `/api/v1/weights` and `/api/v1/metric_correlations` (`src/web/api/v1/api_v1_weights.c`),
//! and `/api/v2/weights`, which version 3 shares (`api_v2_weights()`). The numbers and the formats are
//! `netdata_agent_query::weights`.

use std::sync::Arc;

use netdata_agent_query::weights::Method;
use netdata_agent_query::weights::engine::{Env, Finished, NO_RESULTS, run};
use netdata_agent_query::weights::parse::{Format, LIMIT_ERROR, parse};
use netdata_agent_query::weights::v2::Answering;
use netdata_agent_query::weights::{v1, v2};
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::host::Host;
use netdata_agent_web::content_type::ContentType;
use netdata_agent_web::progress::Table;
use netdata_agent_web::status;

use crate::data::{Answerer, profile_of};
use crate::router::{Route, not_ready};
use crate::server::Reply;

/// `api_v1_weights()`: the anomaly rates, by context.
pub fn v1_weights(route: &Route<'_>, _host: &Arc<Host>, query: &[u8]) -> Reply {
    answer(route, query, 1, Method::AnomalyRate, Format::Contexts, v1::contexts)
}

/// `api_v1_metric_correlations()`: the two-sample Kolmogorov-Smirnov test, by chart.
pub fn v1_metric_correlations(route: &Route<'_>, _host: &Arc<Host>, query: &[u8]) -> Reply {
    answer(route, query, 1, Method::Ks2, Format::Charts, v1::charts)
}

/// `api_v2_weights()`, of `/api/v2/weights` and `/api/v3/weights` alike (both are version 2 to the engine): the
/// values, as a row per result with the rollups, or grouped when the request groups by something.
pub fn v2_weights(route: &Route<'_>, _host: &Arc<Host>, query: &[u8]) -> Reply {
    let answerer = Answerer::new(route.shared);
    let agent = answerer.agent();
    answer(route, query, 2, Method::Value, Format::Multinode, |finished, storage_tiers| {
        v2::multinode(finished, &Answering { agent, storage_tiers })
    })
}

/// `web_client_api_request_weights()`. Until the agent is ready: 503 with the request as the body. A limit that
/// is no number: 400 with C's text, whatever else the query holds. Otherwise the engine over every host (the
/// routed host is not looked at: C gives a version-1 request none, and uses a later version's for nothing), its
/// refusal or the answer `write` makes of its results and the number of storage tiers; a version-1 answer
/// without a dimension is a 404. The reply may be cached when both ends of the highlighted window were absolute.
fn answer(
    route: &Route<'_>,
    query: &[u8],
    version: u8,
    method: Method,
    format: Format,
    write: impl FnOnce(&Finished, usize) -> (Vec<u8>, usize),
) -> Reply {
    if let Some(reply) = not_ready(route) {
        return reply;
    }
    let json =
        |code: u16, body: Vec<u8>| Reply { code, content_type: ContentType::ApplicationJson, body, ..Reply::default() };
    let storage = route.shared.hosts.storage();
    let profile = profile_of(storage);
    let Ok(req) = parse(query, version, method, format, profile.storage_tiers) else {
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
            (_, 0) if version < 2 => (NO_RESULTS.code, NO_RESULTS.body()),
            (body, _) => (status::OK, body),
        },
    };
    Reply { no_cacheable: !outcome.cacheable, ..json(code, body) }
}
