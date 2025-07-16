use flatten_otel::flatten_metrics_request;

use opentelemetry_proto::tonic::collector::metrics::v1::{
    metrics_service_server::{MetricsService, MetricsServiceServer},
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use tokio::sync::RwLock;
use tonic::{transport::Server, Request, Response, Status};

use std::sync::Arc;

mod flattened_point;
use crate::flattened_point::FlattenedPoint;

mod regex_cache;
use crate::regex_cache::RegexCache;

mod samples_table;
use crate::samples_table::NetdataChart;

mod chart_config;
use crate::chart_config::ChartConfigManager;

#[derive(Default)]
struct NetdataMetricsService {
    regex_cache: RegexCache,
    charts: Arc<RwLock<HashMap<String, NetdataChart>>>,
    chart_config_manager: ChartConfigManager,
    file_writers: Arc<RwLock<HashMap<String, BufWriter<File>>>>,
}

impl NetdataMetricsService {
    fn new() -> Self {
        Self {
            regex_cache: RegexCache::default(),
            charts: Arc::default(),
            chart_config_manager: ChartConfigManager::with_default_configs(),
            file_writers: Arc::default(),
        }
        // let mut s = Self::default();
        // s.chart_config_manager = ChartConfigManager::with_default_configs();
        // s
    }

    async fn get_or_create_writer(&self, chart_id: &str) -> Option<()> {
        let mut writers = self.file_writers.write().await;

        if writers.contains_key(chart_id) {
            return Some(());
        }

        // Sanitize chart_id to be a valid filename
        let sanitized_id = chart_id
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>();

        let file_path = Path::new("/tmp").join(&sanitized_id);

        match File::create(&file_path) {
            Ok(file) => {
                eprintln!("Created file for chart '{}' at: {:?}", chart_id, file_path);
                writers.insert(chart_id.to_string(), BufWriter::new(file));
                Some(())
            }
            Err(e) => {
                eprintln!(
                    "Failed to create file for chart '{}' at {:?}: {}",
                    chart_id, file_path, e
                );
                None
            }
        }
    }
}

#[tonic::async_trait]
impl MetricsService for NetdataMetricsService {
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        eprintln!("Received request...");

        let req = request.into_inner();

        let flattened_points = flatten_metrics_request(&req)
            .into_iter()
            .filter_map(|jm| {
                let cfg = self.chart_config_manager.find_matching_config(&jm);
                FlattenedPoint::new(jm, cfg, &self.regex_cache)
            })
            .collect::<Vec<_>>();

        if true {
            // ingest
            {
                for fp in flattened_points.iter() {
                    let mut guard = self.charts.write().await;

                    if let Some(netdata_chart) = guard.get_mut(&fp.nd_instance_name) {
                        netdata_chart.ingest(fp);
                    } else {
                        let _ = self.get_or_create_writer(&fp.nd_instance_name).await;

                        let mut netdata_chart = NetdataChart::from_flattened_point(fp);
                        netdata_chart.ingest(fp);
                        guard.insert(fp.nd_instance_name.clone(), netdata_chart);
                    }
                }
            }

            // process
            {
                let mut guard = self.charts.write().await;
                let mut writers_guard = self.file_writers.write().await;

                eprintln!("GVD: number of charts: {:?}", guard.len());

                for (chart_id, netdata_chart) in guard.iter_mut() {
                    if let Some(writer) = writers_guard.get_mut(chart_id) {
                        netdata_chart.process(writer);
                    }
                }
            }
        }

        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: None,
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:21213".parse()?;
    let metrics_service = NetdataMetricsService::new();

    eprintln!("OTEL Metrics Receiver listening on {}", addr);

    Server::builder()
        .add_service(
            MetricsServiceServer::new(metrics_service)
                .accept_compressed(tonic::codec::CompressionEncoding::Gzip),
        )
        .serve(addr)
        .await?;

    Ok(())
}
