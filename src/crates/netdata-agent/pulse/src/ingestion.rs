//! `pulse-ingestion.c`: the points stored per tier.

use netdata_agent_rrd::chart::{Algorithm, ChartType};

use crate::chart::{Def, Localhost, WithDims, dim, set};

#[derive(Default)]
pub(crate) struct Charts {
    points_stored: Option<WithDims>,
}

impl Charts {
    /// `pulse_ingestion_do()`.
    pub fn update(&mut self, localhost: &Localhost<'_>) {
        let storage = localhost.host.storage();
        let stored = storage.pulse().ingestion.read();
        let (chart, dims) = self.points_stored.get_or_insert_with(|| {
            let chart = localhost.create(&Def {
                id: "db_samples_collected",
                family: "Data Collection Samples",
                context: None,
                title: "Netdata Time-Series Collected Samples",
                units: "samples/s",
                module: "pulse",
                priority: 131003,
                chart_type: ChartType::Stacked,
            });
            let dims = (0..storage.storage_tiers())
                .map(|tier| dim(&chart, &format!("tier{tier}"), 1, 1, Algorithm::Incremental))
                .collect();
            (chart, dims)
        });
        for (dim, points) in dims.iter().zip(stored) {
            set(dim, points as i64);
        }
        localhost.done(chart);
    }
}
