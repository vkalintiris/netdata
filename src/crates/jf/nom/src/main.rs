use flatten_otel::flatten_metrics_request;

use opentelemetry_proto::tonic::collector::metrics::v1::{
    metrics_service_server::{MetricsService, MetricsServiceServer},
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use std::collections::HashMap;
use std::sync::RwLock;
use tonic::{transport::Server, Request, Response, Status};

use std::sync::Arc;

mod flattened_point;
use crate::flattened_point::FlattenedPoint;

mod regex_cache;
use crate::regex_cache::RegexCache;

mod samples_table;
use crate::samples_table::NetdataChart;

#[derive(Default)]
struct MyMetricsService {
    regex_cache: RegexCache,
    charts: Arc<RwLock<HashMap<String, NetdataChart>>>,
}

#[tonic::async_trait]
impl MetricsService for MyMetricsService {
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        let hs: std::collections::HashSet<String> = vec![
            String::from("system.cpu.time"),
            String::from("system.network.packets"),
        ]
        .into_iter()
        .collect();

        let req = request.into_inner();

        let flattened_points = flatten_metrics_request(&req)
            .into_iter()
            .filter_map(|fm| FlattenedPoint::new(fm, &self.regex_cache))
            .filter(|fm| hs.contains(&fm.metric_name))
            .collect::<Vec<_>>();

        // ingest
        {
            for fp in flattened_points.iter() {
                let mut guard = self.charts.write().unwrap();

                // eprintln!("fp: {:#?}", fp);

                if !guard.contains_key(&fp.nd_instance_name) {
                    let netdata_chart = NetdataChart::from_flattened_point(fp);
                    // eprintln!("Chart: {:#?}", netdata_chart);
                    guard.insert(fp.nd_instance_name.clone(), netdata_chart);
                }

                let netdata_chart = guard.get_mut(&fp.nd_instance_name).unwrap();
                netdata_chart.ingest(fp);
            }
        }

        // process
        {
            let mut guard = self.charts.write().unwrap();

            for netdata_chart in guard.values_mut() {
                netdata_chart.process();
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
    let metrics_service = MyMetricsService::default();

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
