use std::time::SystemTime;
use tonic::transport::{Channel, ClientTlsConfig, Certificate};
use opentelemetry_proto::tonic::collector::metrics::v1::{
    metrics_service_client::MetricsServiceClient, ExportMetricsServiceRequest,
};
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
use opentelemetry_proto::tonic::metrics::v1::{
    Metric, ResourceMetrics, ScopeMetrics, NumberDataPoint, Gauge,
};
use opentelemetry_proto::tonic::resource::v1::Resource;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    
    if args.len() < 2 {
        eprintln!("Usage: {} <insecure|secure> [ca-cert-path]", args[0]);
        eprintln!("Examples:");
        eprintln!("  {} insecure", args[0]);
        eprintln!("  {} secure", args[0]);
        eprintln!("  {} secure ./test-certs/ca-cert.pem", args[0]);
        std::process::exit(1);
    }

    let use_tls = &args[1] == "secure";
    let endpoint = std::env::var("ENDPOINT").unwrap_or_else(|_| {
        if use_tls {
            "https://localhost:21213".to_string()
        } else {
            "http://localhost:21213".to_string()
        }
    });

    let channel = if use_tls {
        let mut tls_config = ClientTlsConfig::new()
            .domain_name("localhost");

        // If CA certificate is provided, use it for verification
        if args.len() > 2 {
            let ca_cert = std::fs::read(&args[2])?;
            let cert = Certificate::from_pem(ca_cert);
            tls_config = tls_config.ca_certificate(cert);
        } else {
            // For self-signed certificates, we'll use the default verification
            // In production, proper certificates should be used
        }

        Channel::from_shared(endpoint)?
            .tls_config(tls_config)?
            .connect()
            .await?
    } else {
        Channel::from_shared(endpoint)?.connect().await?
    };

    let mut client = MetricsServiceClient::new(channel);

    // Create a simple test metric
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_nanos() as u64;

    let request = ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(Resource {
                attributes: vec![KeyValue {
                    key: "service.name".to_string(),
                    value: Some(AnyValue {
                        value: Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                            "test-client".to_string(),
                        )),
                    }),
                }],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_metrics: vec![ScopeMetrics {
                scope: None,
                metrics: vec![Metric {
                    name: "test_metric".to_string(),
                    description: "A test metric".to_string(),
                    unit: "1".to_string(),
                    metadata: vec![],
                    data: Some(opentelemetry_proto::tonic::metrics::v1::metric::Data::Gauge(
                        Gauge {
                            data_points: vec![NumberDataPoint {
                                attributes: vec![],
                                start_time_unix_nano: now - 1000000000, // 1 second ago
                                time_unix_nano: now,
                                value: Some(opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsDouble(42.0)),
                                exemplars: vec![],
                                flags: 0,
                            }],
                        },
                    )),
                }],
                schema_url: "".to_string(),
            }],
            schema_url: "".to_string(),
        }],
    };

    println!("Sending test metric to {} endpoint...", if use_tls { "secure" } else { "insecure" });
    
    match client.export(tonic::Request::new(request)).await {
        Ok(response) => {
            println!("✅ Successfully sent metric! Response: {:?}", response.into_inner());
        }
        Err(e) => {
            eprintln!("❌ Failed to send metric: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}