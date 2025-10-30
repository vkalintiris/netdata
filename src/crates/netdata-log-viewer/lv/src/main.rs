#![allow(unused_imports)]

use axum::{
    Json, Router,
    extract::Query,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing,
};
use histogram_service::ui::available_histogram;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use types::{
    Columns, HealthResponse, JournalRequest, JournalResponse, MultiSelection, MultiSelectionOption,
    Pagination, RequestParam, RequiredParam, Version,
};

// Import console_subscriber - the tokio-console crate exports this module
use console_subscriber;

/// Pretty-printed JSON response wrapper
struct PrettyJson<T>(T);

impl<T> IntoResponse for PrettyJson<T>
where
    T: Serialize,
{
    fn into_response(self) -> Response {
        match serde_json::to_string_pretty(&self.0) {
            Ok(json) => (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                json,
            )
                .into_response(),
            Err(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to serialize response",
            )
                .into_response(),
        }
    }
}

#[tracing::instrument]
async fn health_handler() -> impl IntoResponse {
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let response = HealthResponse {
        status: String::from("I'm healthy baby"),
        timestamp: current_time.to_string(),
    };

    PrettyJson(response).into_response()
}

/// Builds a FilterExpr from the selections HashMap
///
/// # Arguments
/// * `selections` - HashMap where each key is a field name and each value is a Vec of possible values
///
/// # Returns
/// A FilterExpr where:
/// - Values for the same field are combined with OR logic (e.g., PRIORITY=1 OR PRIORITY=2)
/// - Different fields are combined with AND logic (e.g., (PRIORITY=1 OR PRIORITY=2) AND _HOSTNAME=server1)
/// - Returns FilterExpr::None if selections is empty or all value lists are empty
fn build_filter_from_selections(
    selections: &std::collections::HashMap<String, Vec<String>>,
) -> journal::index::FilterExpr<String> {
    if selections.is_empty() {
        return journal::index::FilterExpr::None;
    }

    let mut field_filters = Vec::new();

    for (field, values) in selections {
        // Ignore log sources. We've not implemented this functionality.
        if field == "__logs_sources" {
            continue;
        }

        if values.is_empty() {
            continue; // Skip empty value lists
        }

        // Build OR filter for all values of this field
        // e.g., PRIORITY=1 OR PRIORITY=2 OR PRIORITY=3
        let value_filters: Vec<_> = values
            .iter()
            .map(|value| journal::index::FilterExpr::match_str(format!("{}={}", field, value)))
            .collect();

        let field_filter = journal::index::FilterExpr::or(value_filters);
        field_filters.push(field_filter);
    }

    // Combine all field filters with AND
    // e.g., (PRIORITY=1 OR PRIORITY=2) AND (_HOSTNAME=server1 OR _HOSTNAME=server2)
    if field_filters.is_empty() {
        journal::index::FilterExpr::None
    } else {
        journal::index::FilterExpr::and(field_filters)
    }
}

#[tracing::instrument(skip(state))]
async fn journal_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<JournalRequest>,
) -> impl IntoResponse {
    let filter_expr = build_filter_from_selections(&request.selections);

    let histogram_request = histogram_service::HistogramRequest {
        after: request.after as u64,
        before: request.before as u64,
        filter_expr: Arc::new(filter_expr),
    };

    let histogram_result = state
        .journal_state
        .write()
        .await
        .get_histogram(histogram_request)
        .await;

    PrettyJson(JournalResponse {
        version: Version::default(),
        accepted_params: AppState::accepted_params(),
        required_params: AppState::required_params(),
        facets: histogram_result.ui_facets(),
        histogram: histogram_result.ui_histogram("PRIORITY"),
        available_histograms: histogram_result.ui_available_histograms(),
        columns: Columns {},
        data: Vec::new(),
        default_charts: Vec::new(),
        // Hard coded stuff
        show_ids: false,
        has_history: true,
        status: 200,
        response_type: String::from("table"),
        help: String::from("View, search and analyze systemd journal entries."),
        pagination: Pagination::default(),
    })
    .into_response()
}

struct AppState {
    journal_state: Arc<RwLock<histogram_service::AppState>>,
}

impl AppState {
    pub const ACCEPTED_PARAMS: &'static [RequestParam] = &[
        RequestParam::Info,
        RequestParam::LogsSources,
        RequestParam::After,
        RequestParam::Before,
        RequestParam::Anchor,
        RequestParam::Direction,
        RequestParam::Last,
        RequestParam::Query,
        RequestParam::Facets,
        RequestParam::Histogram,
        RequestParam::IfModifiedSince,
        RequestParam::DataOnly,
        RequestParam::Delta,
        RequestParam::Tail,
        RequestParam::Sampling,
        RequestParam::Slice,
    ];

    pub fn accepted_params() -> Vec<RequestParam> {
        Self::ACCEPTED_PARAMS.to_vec()
    }

    pub fn required_params() -> Vec<RequiredParam> {
        let mut v = Vec::new();

        let id = RequestParam::LogsSources;
        let name = String::from("Journal Sources");
        let help = String::from("Select the logs source to query");
        let type_ = String::from("multiselect");
        let mut options = Vec::new();

        let o1 = MultiSelectionOption {
            id: String::from("all"),
            name: String::from("all"),
            pill: String::from("100GiB"),
            info: String::from("All the logs"),
        };
        options.push(o1);

        let required_param = RequiredParam::MultiSelection(MultiSelection {
            id,
            name,
            help,
            type_,
            options,
        });

        v.push(required_param);
        v
    }
}

pub fn get_facets() -> HashSet<String> {
    let v: Vec<&[u8]> = vec![
        b"_HOSTNAME",
        b"PRIORITY",
        b"SYSLOG_FACILITY",
        b"ERRNO",
        b"SYSLOG_IDENTIFIER",
        // b"UNIT",
        b"USER_UNIT",
        b"MESSAGE_ID",
        b"_BOOT_ID",
        b"_SYSTEMD_OWNER_UID",
        b"_UID",
        b"OBJECT_SYSTEMD_OWNER_UID",
        b"OBJECT_UID",
        b"_GID",
        b"OBJECT_GID",
        b"_CAP_EFFECTIVE",
        b"_AUDIT_LOGINUID",
        b"OBJECT_AUDIT_LOGINUID",
        b"CODE_FUNC",
        b"ND_LOG_SOURCE",
        b"CODE_FILE",
        b"ND_ALERT_NAME",
        b"ND_ALERT_CLASS",
        b"_SELINUX_CONTEXT",
        b"_MACHINE_ID",
        b"ND_ALERT_TYPE",
        b"_SYSTEMD_SLICE",
        b"_EXE",
        // b"_SYSTEMD_UNIT",
        b"_NAMESPACE",
        b"_TRANSPORT",
        b"_RUNTIME_SCOPE",
        b"_STREAM_ID",
        b"ND_NIDL_CONTEXT",
        b"ND_ALERT_STATUS",
        // b"_SYSTEMD_CGROUP",
        b"ND_NIDL_NODE",
        b"ND_ALERT_COMPONENT",
        b"_COMM",
        b"_SYSTEMD_USER_UNIT",
        b"_SYSTEMD_USER_SLICE",
        // b"_SYSTEMD_SESSION",
        b"__logs_sources",
    ];

    // let v: Vec<&[u8]> = vec![b"log.severity_number"];

    let mut facets = HashSet::default();
    for e in v {
        facets.insert(String::from_utf8_lossy(e).into_owned());
    }

    facets
}

impl AppState {
    fn new(
        journal_state: histogram_service::AppState,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            journal_state: Arc::new(RwLock::new(journal_state)),
        })
    }
}

fn initialize_tracing() {
    use opentelemetry::trace::TracerProvider;
    use opentelemetry_otlp::WithExportConfig;
    use tracing_subscriber::{EnvFilter, prelude::*};

    // Create Otel layer
    let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint("http://localhost:4317")
        .build()
        .expect("Failed to build OTLP exporter");

    let resource = opentelemetry_sdk::Resource::builder()
        .with_service_name("histogram-backend")
        .build();

    let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(otlp_exporter)
        .with_resource(resource)
        .build();

    let tracer = tracer_provider.tracer("histogram-backend");
    let telemetry_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    // Create the fmt layer with your existing configuration
    let fmt_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);

    // Create the environment filter
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("debug,histogram-backend=debug,tokio=trace,runtime=trace")
    });

    let console_layer = console_subscriber::spawn();

    // Combine all layers
    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(telemetry_layer)
        .with(console_layer)
        .init();
}

#[tokio::main]
async fn main() {
    // initialize_tracing();

    let indexed_fields = get_facets();

    let path = "/var/log/journal";
    // let path = "/home/vk/repos/tmp/flog";
    // let path = "/home/vk/repos/tmp/agent-events-journal";
    let journal_state = histogram_service::AppState::new(
        // "/home/vk/repos/tmp/agent-events-journal",
        path,
        indexed_fields,
        tokio::runtime::Handle::current(),
    )
    .await
    .unwrap();

    let state = AppState::new(journal_state).unwrap();

    // Build router with CORS support
    let app = Router::new()
        .layer(CorsLayer::permissive())
        .route("/health", routing::get(health_handler))
        .route("/journal", routing::post(journal_handler))
        .layer(axum_tracing_opentelemetry::middleware::OtelAxumLayer::default())
        .with_state(Arc::new(state));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();

    println!("Starting Rust backend service on http://localhost:8080");
    println!("Health endpoint: http://localhost:8080/health");
    println!(
        "Histogram endpoint: http://localhost:8080/histogram?after=<timestamp>&before=<timestamp>"
    );

    axum::serve(listener, app).await.unwrap();
}
