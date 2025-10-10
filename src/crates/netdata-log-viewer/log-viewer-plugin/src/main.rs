#![allow(dead_code)]

use async_trait::async_trait;
use netdata_plugin_error::Result;
use netdata_plugin_protocol::FunctionDeclaration;
use netdata_plugin_schema::HttpAccess;
use rt::{FunctionHandler, PluginRuntime};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

#[derive(Deserialize)]
struct EmptyRequest {}

#[derive(Debug, Serialize, Deserialize)]
struct HealthResponse {
    status: String,
    timestamp: String,
}

#[derive(Default)]
struct HealthHandler;

#[async_trait]
impl FunctionHandler for HealthHandler {
    type Request = EmptyRequest;
    type Response = HealthResponse;

    async fn on_call(&self, _request: Self::Request) -> Result<Self::Response> {
        info!("Health function called");

        let response = reqwest::get("http://localhost:8080/health").await.unwrap();
        let resp = response.json::<HealthResponse>().await.unwrap();
        println!("Response: {:?}", resp);

        Ok(resp)
    }

    async fn on_cancellation(&self) -> Result<Self::Response> {
        // Health  function doesn't really need cancellation handling
        Err(netdata_plugin_error::NetdataPluginError::Other {
            message: "Cancelled".to_string(),
        })
    }

    async fn on_progress(&self) {
        info!("Progress requested for health function");
    }

    fn declaration(&self) -> FunctionDeclaration {
        let mut func_decl =
            FunctionDeclaration::new("health", "A health function that responds immediately");
        func_decl.global = true;
        func_decl.access =
            Some(HttpAccess::SIGNED_ID | HttpAccess::SAME_SPACE | HttpAccess::SENSITIVE_DATA);
        func_decl
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JournalRequest {
    #[serde(default)]
    pub info: bool,

    /// Unix timestamp for the start of the time range
    pub after: i64,

    /// Unix timestamp for the end of the time range
    pub before: i64,

    /// Maximum number of results to return
    pub last: Option<u32>,

    /// List of facets to include in the response
    #[serde(default)]
    pub facets: Vec<String>,

    /// Whether to slice the results
    pub slice: Option<bool>,

    /// Query string (empty in your example)
    #[serde(default)]
    pub query: String,

    /// Selection filters
    #[serde(default)]
    pub selections: HashMap<String, Vec<String>>,

    /// Timeout in milliseconds
    pub timeout: Option<u32>,
}

impl Default for JournalRequest {
    fn default() -> Self {
        Self {
            info: true,
            after: 0,
            before: 0,
            last: Some(200),
            facets: Vec::new(),
            slice: None,
            query: String::new(),
            selections: HashMap::new(),
            timeout: None,
        }
    }
}

#[derive(Debug, Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RequestParam {
    Info,
    #[serde(rename = "__logs_sources")]
    LogsSources,
    After,
    Before,
    Anchor,
    Direction,
    Last,
    Query,
    Facets,
    Histogram,
    IfModifiedSince,
    DataOnly,
    Delta,
    Tail,
    Sampling,
    Slice,
}

#[derive(Debug, Serialize, Deserialize)]
struct MultiSelectionOption {
    id: String,
    name: String,
    pill: String,
    info: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct MultiSelection {
    id: RequestParam,
    name: String,
    help: String,
    #[serde(rename = "type", default = "MultiSelection::default_type")]
    type_: String,
    options: Vec<MultiSelectionOption>,
}

impl MultiSelection {
    fn default_type() -> String {
        "multiselect".to_string()
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum RequiredParam {
    MultiSelection(MultiSelection),
}

#[derive(Debug, Serialize, Deserialize)]
struct Version(u32);

impl Default for Version {
    fn default() -> Self {
        Self(3)
    }
}

#[derive(Serialize, Deserialize)]
struct Pagination {
    enabled: bool,
    key: RequestParam,
    column: String,
    units: String,
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            enabled: true,
            key: RequestParam::Anchor,
            column: String::from("timestamp"),
            units: String::from("timestamp_usec"),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct Versions {
    sources: u64,
}

#[derive(Serialize, Deserialize)]
struct Columns {}

#[derive(Serialize, Deserialize)]
struct JournalResponse {
    #[serde(rename = "v")]
    version: Version,

    accepted_params: Vec<RequestParam>,
    required_params: Vec<RequiredParam>,

    available_histograms: serde_json::Value,
    histogram: serde_json::Value,
    // FIXME: columns do not contain u32s
    columns: Columns,
    data: Vec<u32>,
    default_charts: Vec<u32>,

    // Hard-coded stuff
    show_ids: bool,
    has_history: bool,
    status: u32,
    #[serde(rename = "type")]
    response_type: String,
    help: String,
    pagination: Pagination,
    // versions: Versions,
}

use polars::prelude as polars;
use rand::Rng;

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

use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Default)]
struct Journal {
    histogram: Arc<RwLock<Option<Histogram>>>,
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

impl Journal {
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

    fn new(histogram: Histogram) -> Self {
        Self {
            histogram: Arc::new(RwLock::new(Some(histogram))),
        }
    }

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

#[async_trait]
impl FunctionHandler for Journal {
    type Request = JournalRequest;
    type Response = JournalResponse;

    async fn on_call(&self, request: Self::Request) -> Result<Self::Response> {
        info!("systemd-journal function request: {:#?}", request);

        let histogram_json = {
            let mut guard = self.histogram.write().await;

            if let Some(histogram) = guard.as_mut() {
                let id = String::from("PRIORITY");
                let name = id.clone();
                let after = request.after;
                let before = request.before;
                let mut rng = rand::rng();

                *histogram =
                    Histogram::new_dummy_with_counts(id, name, after, before, &mut rng).unwrap();
            } else {
                let id = String::from("PRIORITY");
                let name = id.clone();
                let after = request.after;
                let before = request.before;
                let mut rng = rand::rng();

                let histogram =
                    Histogram::new_dummy_with_counts(id, name, after, before, &mut rng).unwrap();
                guard.replace(histogram);
            }

            let histogram = guard.as_mut().unwrap();
            histogram.to_json()
        };

        Ok(JournalResponse {
            version: Version::default(),
            accepted_params: Self::accepted_params(),
            required_params: Self::required_params(),
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
    }

    async fn on_cancellation(&self) -> Result<Self::Response> {
        warn!("Slow function was cancelled!");
        Err(netdata_plugin_error::NetdataPluginError::Other {
            message: "Operation cancelled by user".to_string(),
        })
    }

    async fn on_progress(&self) {
        info!("Progress report requested for slow function");
    }

    fn declaration(&self) -> FunctionDeclaration {
        let mut func_decl = FunctionDeclaration::new(
            "systemd-journal",
            "A slow function that takes 10 seconds and respects cancellation",
        );
        func_decl.global = true;
        func_decl.tags = Some(String::from("logs"));
        func_decl.access =
            Some(HttpAccess::SIGNED_ID | HttpAccess::SAME_SPACE | HttpAccess::SENSITIVE_DATA);
        func_decl
    }
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "info,netdata_plugin_channels=debug".to_string()),
        )
        .init();

    info!("Starting example plugin");

    if true {
        // Check if we should use TCP or stdio based on command line argument
        let args: Vec<String> = std::env::args().collect();
        if args.len() > 1 && args[1] == "--tcp" {
            // TCP mode: expect address as second argument
            let addr = args.get(2).unwrap_or(&"127.0.0.1:9999".to_string()).clone();
            info!("Running in TCP mode, connecting to {}", addr);
            run_tcp_mode(&addr).await?;
        } else {
            // Default stdio mode
            info!("Running in stdio mode");
            run_stdio_mode().await?;
        }
    }

    Ok(())
}

/// Run the plugin using stdin/stdout (default Netdata mode)
async fn run_stdio_mode() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut runtime = PluginRuntime::new("example");
    runtime.register_handler(Journal::default());
    runtime.run().await?;
    Ok(())
}

/// Run the plugin using a TCP connection
async fn run_tcp_mode(addr: &str) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use tokio::net::TcpStream;

    info!("Connecting to TCP server at {}", addr);
    let stream = TcpStream::connect(addr).await?;
    info!("Connected to TCP server");

    let (reader, writer) = stream.into_split();

    let mut runtime = PluginRuntime::with_streams("example", reader, writer);
    runtime.register_handler(Journal::default());
    runtime.register_handler(HealthHandler {});
    runtime.run().await?;

    Ok(())
}
