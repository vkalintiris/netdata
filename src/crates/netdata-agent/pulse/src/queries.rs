//! `pulse-queries.c`: the queries, the samples they read and the points they generated, per source. The exporters
//! are not ported and the replication queries are the stream sender's, which is not either: both are 0.

use netdata_agent_rrd::chart::{Algorithm, ChartType};
use netdata_agent_rrd::pulse::{QuerySource, SourceStats};

use crate::chart::{Def, Localhost, WithDims, dim, set};

/// The dimensions of the queries and samples read charts; the points generated chart has neither exporters nor
/// backfill.
const SOURCES: [&str; 8] = [
    "/api/vX/data",
    "/api/vX/weights",
    "/api/vX/badge",
    "health",
    "ml",
    "exporters",
    "backfill",
    "replication",
];
const GENERATED: [&str; 6] = [
    "/api/vX/data",
    "/api/vX/weights",
    "/api/vX/badge",
    "health",
    "ml",
    "replication",
];

#[derive(Default)]
pub(crate) struct Charts {
    queries: Option<WithDims>,
    points_read: Option<WithDims>,
    points_generated: Option<WithDims>,
}

/// A chart of the query statistics, its dimensions incremental.
fn update(
    slot: &mut Option<WithDims>,
    localhost: &Localhost<'_>,
    def: &Def<'_>,
    dims: &[&str],
    values: &[u64],
) {
    let (chart, rds) = slot.get_or_insert_with(|| {
        let chart = localhost.create(def);
        let rds = dims
            .iter()
            .map(|id| dim(&chart, id, 1, 1, Algorithm::Incremental))
            .collect();
        (chart, rds)
    });
    for (rd, value) in rds.iter().zip(values) {
        set(rd, *value as i64);
    }
    localhost.done(chart);
}

impl Charts {
    /// `pulse_queries_do()`.
    pub fn update(&mut self, localhost: &Localhost<'_>) {
        let queries = &localhost.host.storage().pulse().queries;
        let [data, weights, badge, health, ml] = [
            QuerySource::ApiData,
            QuerySource::ApiWeights,
            QuerySource::ApiBadge,
            QuerySource::Health,
            QuerySource::Ml,
        ]
        .map(|source| queries.source(source));
        let (backfill_queries, backfill_points) = queries.backfill();
        let (exporters, replication) = (SourceStats::default(), SourceStats::default());
        let def = |id, context, title, units, priority| Def {
            id,
            family: "Time-Series Queries",
            context,
            title,
            units,
            module: "pulse",
            priority,
            chart_type: ChartType::Stacked,
        };

        update(
            &mut self.queries,
            localhost,
            &def(
                "queries",
                Some("netdata.db_queries"),
                "Netdata Time-Series DB Queries",
                "queries/s",
                131000,
            ),
            &SOURCES,
            &[
                data.queries,
                weights.queries,
                badge.queries,
                health.queries,
                ml.queries,
                exporters.queries,
                backfill_queries,
                replication.queries,
            ],
        );
        update(
            &mut self.points_read,
            localhost,
            &def(
                "db_samples_read",
                None,
                "Netdata Time-Series DB Samples Read",
                "samples/s",
                131001,
            ),
            &SOURCES,
            &[
                data.points_read,
                weights.points_read,
                badge.points_read,
                health.points_read,
                ml.points_read,
                exporters.points_read,
                backfill_points,
                replication.points_read,
            ],
        );
        // created once a data query or a replication generated points
        if data.points_generated != 0 || replication.points_generated != 0 {
            update(
                &mut self.points_generated,
                localhost,
                &def(
                    "db_points_results",
                    None,
                    "Netdata Time-Series Points Generated",
                    "points/s",
                    131002,
                ),
                &GENERATED,
                &[
                    data.points_generated,
                    weights.points_generated,
                    badge.points_generated,
                    health.points_generated,
                    ml.points_generated,
                    replication.points_generated,
                ],
            );
        }
    }
}
