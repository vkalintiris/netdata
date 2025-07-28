use flatten_otel::flatten_metrics_request;

use opentelemetry_proto::tonic::collector::logs::v1::logs_service_server::LogsServiceServer;
use opentelemetry_proto::tonic::collector::metrics::v1::{
    metrics_service_server::{MetricsService, MetricsServiceServer},
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use std::collections::HashMap;
use tokio::sync::RwLock;
use tonic::{transport::Server, Request, Response, Status};

use std::sync::Arc;

mod flattened_point;
use crate::flattened_point::FlattenedPoint;

mod regex_cache;
use crate::regex_cache::RegexCache;

mod chart_config;

mod netdata_chart;
use crate::netdata_chart::NetdataChart;

mod samples_table;

mod plugin_config;
use crate::plugin_config::{CliConfig, LogsConfig, MetricsConfig, PluginConfig};

mod journal_logs_service;
use crate::journal_logs_service::NetdataJournalLogsService;

#[derive(Default)]
struct NetdataMetricsService {
    regex_cache: RegexCache,
    charts: Arc<RwLock<HashMap<String, NetdataChart>>>,
    config: Arc<PluginConfig>,
    call_count: std::sync::atomic::AtomicU64,
}

impl NetdataMetricsService {
    fn new(config: PluginConfig) -> Self {
        Self {
            regex_cache: RegexCache::default(),
            charts: Arc::default(),
            config: Arc::new(config),
            call_count: std::sync::atomic::AtomicU64::new(0),
        }
    }

    async fn cleanup_stale_charts(&self, max_age: std::time::Duration) {
        let now = std::time::SystemTime::now();

        let mut guard = self.charts.write().await;
        guard.retain(|_, chart| {
            let Some(chart_time) = chart.last_collection_time() else {
                return true;
            };

            now.duration_since(chart_time)
                .unwrap_or(std::time::Duration::ZERO)
                < max_age
        });
    }
}

#[tonic::async_trait]
impl MetricsService for NetdataMetricsService {
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        let req = request.into_inner();

        let flattened_points = flatten_metrics_request(&req)
            .into_iter()
            .filter_map(|jm| {
                let cfg = self
                    .config
                    .metrics_config
                    .chart_config_manager
                    .find_matching_config(&jm);
                FlattenedPoint::new(jm, cfg, &self.regex_cache)
            })
            .collect::<Vec<_>>();

        if self.config.metrics_config.print_flattened {
            // Just print the flattened points
            for fp in &flattened_points {
                println!("{:#?}", fp);
            }

            return Ok(Response::new(ExportMetricsServiceResponse {
                partial_success: None,
            }));
        }

        // ingest
        {
            let mut newly_created_charts = 0;

            for fp in flattened_points.iter() {
                let mut guard = self.charts.write().await;

                if let Some(netdata_chart) = guard.get_mut(&fp.nd_instance_name) {
                    netdata_chart.ingest(fp);
                } else if newly_created_charts < self.config.metrics_config.throttle_charts {
                    let mut netdata_chart = NetdataChart::from_flattened_point(
                        fp,
                        self.config.metrics_config.buffer_samples,
                    );
                    netdata_chart.ingest(fp);
                    guard.insert(fp.nd_instance_name.clone(), netdata_chart);

                    newly_created_charts += 1;
                }
            }
        }

        // process
        {
            let mut guard = self.charts.write().await;

            for netdata_chart in guard.values_mut() {
                netdata_chart.process();
            }
        }

        // cleanup stale charts
        {
            let prev_count = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            if prev_count % 60 == 0 {
                let one_hour = std::time::Duration::from_secs(3600);
                self.cleanup_stale_charts(one_hour).await;
            }
        }

        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: None,
        }))
    }
}

// Old simple logs service removed - now using NetdataJournalLogsService

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli_config = CliConfig::new()?;
    let metrics_config = MetricsConfig::from_cli_config(&cli_config);
    let logs_config = LogsConfig::from_cli_config(&cli_config);
    let plugin_config = PluginConfig::new(&metrics_config, &logs_config);

    let addr = cli_config.otel_endpoint.parse()?;
    let metrics_service = NetdataMetricsService::new(plugin_config);
    let logs_service = NetdataJournalLogsService::new(&logs_config)?;

    println!("TRUST_DURATIONS 1");

    Server::builder()
        .add_service(
            MetricsServiceServer::new(metrics_service)
                .accept_compressed(tonic::codec::CompressionEncoding::Gzip),
        )
        .add_service(
            LogsServiceServer::new(logs_service)
                .accept_compressed(tonic::codec::CompressionEncoding::Gzip),
        )
        .serve(addr)
        .await?;

    Ok(())
}
