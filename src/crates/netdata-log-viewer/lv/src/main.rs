use axum::{
    Router,
    extract::Query,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;

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
    let mut rng = rand::thread_rng();

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
        let mut info_value = info_base + rng.gen_range(-10.0..10.0);

        // Warning logs: medium baseline with spikes
        let warning_base = 30.0 + 20.0 * ((time_factor * 2.0 * std::f64::consts::PI) + 1.0).sin();
        let mut warning_value = warning_base + rng.gen_range(-8.0..15.0);

        // Error logs: low baseline with occasional spikes
        let error_base = 10.0 + 8.0 * ((time_factor * 2.0 * std::f64::consts::PI) + 2.0).sin();
        let mut error_value = error_base + rng.gen_range(-5.0..10.0);

        // Add spikes if this bucket is recent (simulate live activity)
        if current_time - bucket_time < 300 {
            // Last 5 minutes
            info_value += rng.gen_range(10.0..30.0);
            warning_value += rng.gen_range(5.0..20.0);
            if rng.gen_bool(0.3) {
                // 30% chance of error spike
                error_value += rng.gen_range(10.0..25.0);
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

/// GET /histogram endpoint handler
///
/// Accepts 'after' and 'before' query parameters (Unix timestamps in seconds)
/// Returns histogram data with multiple series per time bucket
///
/// # Example Response
/// ```json
/// {
///   "buckets": [
///     {
///       "time": 1759837000,
///       "data": {
///         "info": 82.5,
///         "warning": 25.3,
///         "error": 8.2
///       }
///     }
///   ]
/// }
/// ```
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

/// GET /health endpoint handler
async fn health_handler() -> impl IntoResponse {
    let current_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();

    let mut response = HashMap::new();
    response.insert("status", "ok".to_string());
    response.insert("timestamp", current_time.to_string());

    PrettyJson(response).into_response()
}

struct AppState {
    registry: Arc<RwLock<journal_registry::Registry>>,
}

impl AppState {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let registry = Arc::new(RwLock::new(journal_registry::Registry::new().unwrap()));

        Ok(Self { registry })
    }

    /// Watch a new directory for journal files
    pub async fn watch_directory(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.registry
            .write()
            .await
            .watch_directory(path)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
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

/// Query parameters for the watch_directory endpoint
#[derive(Debug, Deserialize)]
struct WatchDirectoryQuery {
    /// Directory path to watch
    path: String,
}

/// Query parameters for the find_files endpoint
#[derive(Debug, Deserialize)]
struct FindFilesQuery {
    /// Start timestamp in microseconds (inclusive)
    start: u64,
    /// End timestamp in microseconds (exclusive)
    end: u64,
}

/// Response format for the find_files endpoint
#[derive(Debug, Serialize)]
struct FindFilesResponse {
    files: Vec<FileInfo>,
}

/// File information returned to the client
#[derive(Debug, Serialize)]
struct FileInfo {
    path: String,
}

/// POST /watch_directory endpoint handler
///
/// Accepts 'path' query parameter to add a new directory to watch
async fn watch_directory_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<WatchDirectoryQuery>,
) -> impl IntoResponse {
    match state.watch_directory(&params.path).await {
        Ok(_) => {
            let mut response = HashMap::new();
            response.insert("status", "ok".to_string());
            response.insert("path", params.path);
            PrettyJson(response).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            PrettyJson(ErrorResponse {
                error: format!("Failed to watch directory: {}", e),
            }),
        )
            .into_response(),
    }
}

/// GET /find_files endpoint handler
///
/// Accepts 'start' and 'end' query parameters (Unix timestamps in microseconds)
/// Returns list of journal files that cover the given time range
async fn find_files_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FindFilesQuery>,
) -> impl IntoResponse {
    let start = 1000 * 1000 * params.start;
    let end = 1000 * 1000 * params.end;
    let files = state.find_files_in_range(start, end).await;

    let file_infos: Vec<FileInfo> = files
        .into_iter()
        .map(|file| FileInfo { path: file.path })
        .collect();

    PrettyJson(FindFilesResponse { files: file_infos }).into_response()
}

#[tokio::main]
async fn main() {
    let state = AppState::new().unwrap();

    // Build router with CORS support
    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/histogram", get(histogram_handler))
        .route("/watch_directory", post(watch_directory_handler))
        .route("/find_files", get(find_files_handler))
        .with_state(Arc::new(state))
        .layer(CorsLayer::permissive());

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();

    println!("Starting Rust backend service on http://localhost:8080");
    println!("Health endpoint: http://localhost:8080/health");
    println!(
        "Histogram endpoint: http://localhost:8080/histogram?after=<timestamp>&before=<timestamp>"
    );

    axum::serve(listener, app).await.unwrap();
}
