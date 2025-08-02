use flatten_otel::flatten_metrics_request;

use opentelemetry_proto::tonic::collector::logs::v1::logs_service_server::LogsServiceServer;
use opentelemetry_proto::tonic::collector::metrics::v1::{
    metrics_service_server::{MetricsService, MetricsServiceServer},
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use std::collections::HashMap;
use tokio::sync::RwLock;
use tonic::{
    transport::{Identity, Server, ServerTlsConfig},
    Request, Response, Status,
};

use std::sync::Arc;

mod flattened_point;
use crate::flattened_point::FlattenedPoint;

mod regex_cache;
use crate::regex_cache::RegexCache;

mod chart_config;
use crate::chart_config::ChartConfigManager;

mod netdata_chart;
use crate::netdata_chart::NetdataChart;

mod samples_table;

mod plugin_config;
use crate::plugin_config::PluginConfig;

mod journal_logs_service;
use crate::journal_logs_service::NetdataLogsService;

#[derive(Default)]
struct NetdataMetricsService {
    regex_cache: RegexCache,
    charts: Arc<RwLock<HashMap<String, NetdataChart>>>,
    config: Arc<PluginConfig>,
    chart_config_manager: ChartConfigManager,
    call_count: std::sync::atomic::AtomicU64,
}

impl NetdataMetricsService {
    fn new(config: PluginConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let mut chart_config_manager = ChartConfigManager::with_default_configs();

        // Load user chart configs if directory is specified
        if let Some(chart_configs_dir) = &config.metrics_config.chart_configs_dir {
            chart_config_manager.load_user_configs(chart_configs_dir)?;
        }

        Ok(Self {
            regex_cache: RegexCache::default(),
            charts: Arc::default(),
            config: Arc::new(config),
            chart_config_manager,
            call_count: std::sync::atomic::AtomicU64::new(0),
        })
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
                let cfg = self.chart_config_manager.find_matching_config(&jm);
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = PluginConfig::new()?;

    let addr = config.endpoint_config.path.parse()?;
    let metrics_service = NetdataMetricsService::new(config.clone())?;
    let logs_service = NetdataLogsService::new(&config.logs_config)?;

    println!("TRUST_DURATIONS 1");

    let mut server_builder = Server::builder();

    // Configure TLS if enabled
    if config.endpoint_config.tls.enabled {
        let cert_path = config
            .endpoint_config
            .tls
            .cert_path
            .as_ref()
            .unwrap();
        let key_path = config.endpoint_config.tls.key_path.as_ref().unwrap();

        eprintln!("Loading TLS certificate from: {}", cert_path);
        eprintln!("Loading TLS private key from: {}", key_path);

        let cert = std::fs::read(cert_path)?;
        let key = std::fs::read(key_path)?;
        let identity = Identity::from_pem(cert, key);

        let mut tls_config_builder = ServerTlsConfig::new().identity(identity);

        // If CA certificate is provided, enable client authentication
        if let Some(ca_cert_path) = &config.endpoint_config.tls.ca_cert_path {
            eprintln!("Loading CA certificate from: {}", ca_cert_path);
            let ca_cert = std::fs::read(ca_cert_path)?;
            tls_config_builder =
                tls_config_builder.client_ca_root(tonic::transport::Certificate::from_pem(ca_cert));
        }

        server_builder = server_builder.tls_config(tls_config_builder)?;
        eprintln!("TLS enabled on endpoint: {}", config.endpoint_config.path);
    } else {
        eprintln!(
            "TLS disabled, using insecure connection on endpoint: {}",
            config.endpoint_config.path
        );
    }

    server_builder
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
