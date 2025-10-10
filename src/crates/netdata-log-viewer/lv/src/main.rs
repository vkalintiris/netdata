use axum::{
    Json, Router,
    extract::Query,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing,
};
use polars::prelude as polars;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use types::{
    Columns, HealthResponse, JournalRequest, JournalResponse, MultiSelection, MultiSelectionOption,
    Pagination, RequestParam, RequiredParam, Version,
};

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

/// Query parameters for the histogram endpoint
#[derive(Debug, Deserialize)]
struct HistogramQuery {
    /// Unix timestamp in seconds - start of time range (inclusive)
    after: u64,
    /// Unix timestamp in seconds - end of time range (exclusive)
    before: u64,
}

/// Multi-series data for a single time bucket
#[derive(Debug, Serialize)]
struct BucketData {
    info: f64,
    warning: f64,
    error: f64,
}

/// A single time bucket with timestamp and multi-series data
#[derive(Debug, Serialize)]
struct Bucket {
    /// Unix timestamp in seconds
    time: u64,
    /// Multi-series histogram data
    data: BucketData,
}

/// Response format for the histogram endpoint
#[derive(Debug, Serialize)]
struct HistogramResponse {
    buckets: Vec<Bucket>,
}

/// Error response format
#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

/// Generate realistic histogram data for the given time range
///
/// Returns buckets with timestamps and multiple data series (info, warning, error)
/// simulating log priority counts with realistic patterns and live activity spikes.
fn generate_histogram_data(after: u64, before: u64) -> Vec<Bucket> {
    let mut rng = rand::rng();

    // Create buckets - aim for ~50-100 data points
    let total_duration = before - after;
    let num_buckets = (total_duration / 60).min(100).max(10) as usize; // At least 1 minute per bucket
    let bucket_size = total_duration as f64 / num_buckets as f64;

    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut buckets = Vec::with_capacity(num_buckets);

    for i in 0..num_buckets {
        let bucket_time = after + (i as f64 * bucket_size) as u64;

        // Generate values for different series with different patterns
        let time_factor = ((bucket_time % 86400) as f64) / 86400.0; // Position in day

        // Info logs: high baseline with daily variation
        let info_base = 80.0 + 30.0 * (time_factor * 2.0 * std::f64::consts::PI).sin();
        let mut info_value = info_base + rng.random_range(-10.0..10.0);

        // Warning logs: medium baseline with spikes
        let warning_base = 30.0 + 20.0 * ((time_factor * 2.0 * std::f64::consts::PI) + 1.0).sin();
        let mut warning_value = warning_base + rng.random_range(-8.0..15.0);

        // Error logs: low baseline with occasional spikes
        let error_base = 10.0 + 8.0 * ((time_factor * 2.0 * std::f64::consts::PI) + 2.0).sin();
        let mut error_value = error_base + rng.random_range(-5.0..10.0);

        // Add spikes if this bucket is recent (simulate live activity)
        if current_time - bucket_time < 300 {
            // Last 5 minutes
            info_value += rng.random_range(10.0..30.0);
            warning_value += rng.random_range(5.0..20.0);
            if rng.random_bool(0.3) {
                // 30% chance of error spike
                error_value += rng.random_range(10.0..25.0);
            }
        }

        buckets.push(Bucket {
            time: bucket_time,
            data: BucketData {
                info: info_value.max(0.0).round() / 10.0 * 10.0, // Round to nearest 10
                warning: warning_value.max(0.0).round() / 10.0 * 10.0,
                error: error_value.max(0.0).round() / 10.0 * 10.0,
            },
        });
    }

    buckets
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

#[tracing::instrument(skip(state))]
async fn histogram_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HistogramQuery>,
) -> impl IntoResponse {
    // Validate parameters
    if params.after >= params.before {
        return (
            StatusCode::BAD_REQUEST,
            PrettyJson(ErrorResponse {
                error: "after must be less than before".to_string(),
            }),
        )
            .into_response();
    }

    let start = 1000 * 1000 * params.after;
    let end = 1000 * 1000 * params.before;
    let files = state.find_files_in_range(start, end).await;

    println!("Found {} files", files.len());
    for (idx, file) in files.iter().enumerate() {
        println!("file[{}]: {}", idx, file.path);
    }

    // Generate histogram data
    let buckets = generate_histogram_data(params.after, params.before);

    PrettyJson(HistogramResponse { buckets }).into_response()
}

#[tracing::instrument(skip(state))]
async fn journal_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<JournalRequest>,
) -> impl IntoResponse {
    let histogram_json = {
        let mut guard = state.histogram.write().await;
        let mut rng = rand::rng();

        if let Some(histogram) = guard.as_mut() {
            let id = String::from("PRIORITY");
            let name = id.clone();
            let after = request.after;
            let before = request.before;

            *histogram =
                Histogram::new_dummy_with_counts(id, name, after, before, &mut rng).unwrap();
        } else {
            let id = String::from("PRIORITY");
            let name = id.clone();
            let after = request.after;
            let before = request.before;

            let histogram =
                Histogram::new_dummy_with_counts(id, name, after, before, &mut rng).unwrap();
            guard.replace(histogram);
        }

        let histogram = guard.as_mut().unwrap();
        histogram.to_json()
    };

    PrettyJson(JournalResponse {
        version: Version::default(),
        accepted_params: AppState::accepted_params(),
        required_params: AppState::required_params(),
        histogram: histogram_json,
        available_histograms: serde_json::json!(
            [ { "id": "PRIORITY", "name": "PRIORITY", "order": 1 } ]
        ),
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

pub struct Histogram {
    id: String,
    name: String,
    after: i64,
    before: i64,
    interval: i64,
    df: polars::DataFrame,
}

impl Histogram {
    pub fn new(id: String, name: String, df: polars::DataFrame) -> Self {
        Self {
            id,
            name,
            interval: 1,
            after: 0,
            before: 0,
            df,
        }
    }

    /// Creates a dummy histogram with randomly generated counts
    ///
    /// # Arguments
    /// * `id` - Histogram identifier
    /// * `name` - Histogram name
    /// * `after` - Start timestamp in seconds
    /// * `before` - End timestamp in seconds
    /// * `rng` - Random number generator for generating counts
    ///
    /// # Returns
    /// A Histogram with up to 60 buckets covering the time range with random counts
    pub fn new_dummy_with_counts<R: Rng>(
        id: String,
        name: String,
        after: i64,
        before: i64,
        rng: &mut R,
    ) -> polars::PolarsResult<Self> {
        // Calculate the number of buckets (up to 60)
        let total_duration = before - after;
        let num_buckets = std::cmp::min(60, total_duration.max(1));

        // Calculate the interval between buckets
        let interval = total_duration / num_buckets;

        // Generate timestamps
        let timestamps: Vec<i64> = (0..num_buckets)
            .map(|i| (after + (i * interval)) * 1000)
            .collect();

        // Generate random counts for each label
        let error_counts: Vec<u32> = (0..num_buckets as usize)
            .map(|_| rng.random_range(0..10))
            .collect();
        let warn_counts: Vec<u32> = (0..num_buckets as usize)
            .map(|_| rng.random_range(0..20))
            .collect();
        let info_counts: Vec<u32> = (0..num_buckets as usize)
            .map(|_| rng.random_range(0..50))
            .collect();

        // Create DataFrame
        let df = polars::df! {
            "time" => timestamps,
            "error" => error_counts,
            "warning" => warn_counts,
            "info" => info_counts,
        }?;

        Ok(Self {
            id,
            name,
            after,
            before,
            interval,
            df,
        })
    }

    /// Get the histogram's ID
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Get the histogram's name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get a reference to the underlying DataFrame
    pub fn dataframe(&self) -> &polars::DataFrame {
        &self.df
    }

    pub fn labels(&self) -> Vec<String> {
        self.df
            .get_columns()
            .iter()
            .map(|s| s.name().to_string())
            .collect()
    }

    /// Convert to JSON Value
    pub fn to_json(&self) -> serde_json::Value {
        let columns = self.df.get_columns();

        let labels: Vec<String> = self.labels();

        // Build data rows
        let mut data = Vec::new();
        let height = self.df.height();

        for row_idx in 0..height {
            let mut row_values: Vec<serde_json::Value> = Vec::new();

            // First value is the timestamp
            if let Some(timestamp_col) = columns.first() {
                let timestamp = timestamp_col.get(row_idx).unwrap();
                row_values.push(anyvalue_to_json(&timestamp));
            }

            // Remaining values are the label data (as arrays of u32)
            for col in columns.iter().skip(1) {
                let mut v = Vec::new();
                v.push(serde_json::Value::Number(0.into()));
                v.push(serde_json::Value::Number(0.into()));

                let value = col.get(row_idx).unwrap();
                v.push(anyvalue_to_json(&value));
                row_values.push(serde_json::Value::Array(v));
            }

            data.push(serde_json::Value::Array(row_values));
        }

        serde_json::json!({
            // "histogram": {
                "id": self.id,
                "name": self.name,
                "chart": {
                    "result": {
                        "labels": labels,
                        "data": data,
                        "point": { "value": 2, "arp": 0, "pa": 1 }
                    },
                    "view": {
                        "title": "Events Distribution by PRIORITY",
                        "update_every": self.interval,
                        "after": self.after,
                        "before": self.before,
                        "units": "events",
                        "chart_type": "stackedBar",
                        "dimensions": {
                            "ids": ["3", "4", "6"],
                            "names": ["error", "warning", "info"]
                        }
                    }
                }
            // }
        })
    }
}

fn anyvalue_to_json(value: &polars::AnyValue) -> serde_json::Value {
    match value {
        polars::AnyValue::Null => serde_json::Value::Null,
        polars::AnyValue::Boolean(b) => serde_json::Value::Bool(*b),
        polars::AnyValue::Int64(i) => serde_json::Value::Number((*i).into()),
        polars::AnyValue::UInt32(u) => serde_json::Value::Number((*u).into()),
        polars::AnyValue::Float64(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        polars::AnyValue::String(s) => serde_json::Value::String(s.to_string()),
        polars::AnyValue::List(series) => {
            let values: Vec<serde_json::Value> =
                series.iter().map(|v| anyvalue_to_json(&v)).collect();
            serde_json::Value::Array(values)
        }
        _ => serde_json::Value::String(format!("{:?}", value)),
    }
}

struct AppState {
    registry: Arc<RwLock<journal_registry::Registry>>,
    histogram: Arc<RwLock<Option<Histogram>>>,
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

impl AppState {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let registry = Arc::new(RwLock::new(journal_registry::Registry::new().unwrap()));
        let histogram = Arc::new(RwLock::new(None));

        Ok(Self {
            registry,
            histogram,
        })
    }

    /// Find files in the given time range (Unix timestamps in microseconds)
    pub async fn find_files_in_range(&self, start: u64, end: u64) -> Vec<journal_registry::File> {
        let mut output = Vec::new();
        self.registry
            .read()
            .await
            .find_files_in_range(start, end, &mut output);
        output
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
        .with_service_name("histogram-backent")
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
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("debug,histogram-backend=debug"));

    // Combine all layers
    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(telemetry_layer)
        .init();
}

#[tokio::main]
async fn main() {
    initialize_tracing();

    let state = AppState::new().unwrap();

    // Build router with CORS support
    let app = Router::new()
        .layer(CorsLayer::permissive())
        .layer(axum_tracing_opentelemetry::middleware::OtelInResponseLayer::default())
        .route("/health", routing::get(health_handler))
        .route("/histogram", routing::get(histogram_handler))
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
