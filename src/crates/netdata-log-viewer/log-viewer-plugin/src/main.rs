#![allow(dead_code)]

mod charts;
mod tracing_config;

use async_trait::async_trait;
use netdata_plugin_error::Result;
use netdata_plugin_protocol::FunctionDeclaration;
use netdata_plugin_schema::HttpAccess;
use rt::{FunctionHandler, PluginRuntime};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, instrument, warn};
use types::{
    Auxiliary, JournalRequest, JournalResponse, MultiSelection, MultiSelectionOption, Pagination,
    RequestParam, RequiredParam, Version,
};

use journal_query::{
    FieldName, FieldValuePair, Filter, HistogramRequest, HistogramService, IndexingService, ui,
};

// Import chart definitions
use charts::Metrics;

struct Journal {
    metrics: Metrics,
    histogram_cache: Arc<RwLock<HistogramService>>,
}

/// Builds a FilterExpr from the selections HashMap
#[instrument(skip(selections))]
fn build_filter_from_selections(selections: &HashMap<String, Vec<String>>) -> Filter {
    if selections.is_empty() {
        info!("No selections provided, using empty filter");
        return Filter::none();
    }

    let mut field_filters = Vec::new();

    for (field, values) in selections {
        // Ignore log sources. We've not implemented this functionality.
        if field == "__logs_sources" {
            info!("Ignoring __logs_sources field");
            continue;
        }

        if values.is_empty() {
            continue;
        }

        info!(
            "Building filter for field '{}' with {} values",
            field,
            values.len()
        );

        // Build OR filter for all values of this field
        let value_filters: Vec<_> = values
            .iter()
            .filter_map(|value| {
                let pair_str = format!("{}={}", field, value);
                FieldValuePair::parse(&pair_str).map(Filter::match_field_value_pair)
            })
            .collect();

        if value_filters.is_empty() {
            warn!("All values failed to parse for field '{}'", field);
            continue;
        }

        let field_filter = Filter::or(value_filters);
        field_filters.push(field_filter);
    }

    if field_filters.is_empty() {
        info!("No valid field filters, using empty filter");
        Filter::none()
    } else {
        info!("Created filter with {} field filters", field_filters.len());
        Filter::and(field_filters)
    }
}

fn get_facets() -> HashSet<String> {
    let v: Vec<&[u8]> = vec![
        b"_HOSTNAME",
        b"PRIORITY",
        b"SYSLOG_FACILITY",
        b"ERRNO",
        b"SYSLOG_IDENTIFIER",
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
        b"_NAMESPACE",
        b"_TRANSPORT",
        b"_RUNTIME_SCOPE",
        b"_STREAM_ID",
        b"ND_NIDL_CONTEXT",
        b"ND_ALERT_STATUS",
        b"ND_NIDL_NODE",
        b"ND_ALERT_COMPONENT",
        b"_COMM",
        b"_SYSTEMD_USER_UNIT",
        b"_SYSTEMD_USER_SLICE",
        b"__logs_sources",
    ];

    let mut facets = HashSet::default();
    for e in v {
        facets.insert(String::from_utf8_lossy(e).into_owned());
    }
    facets
}

fn accepted_params() -> Vec<RequestParam> {
    vec![
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
        RequestParam::Auxiliary,
    ]
}

fn required_params() -> Vec<RequiredParam> {
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

#[async_trait]
impl FunctionHandler for Journal {
    type Request = JournalRequest;
    type Response = JournalResponse;

    #[instrument(name = "journal_function_call", skip_all, fields(
        after = request.after,
        before = request.before,
        num_selections = request.selections.len()
    ))]
    async fn on_call(&self, request: Self::Request) -> Result<Self::Response> {
        info!("Processing journal function call");

        let filter_expr = build_filter_from_selections(&request.selections);

        if request.after >= request.before {
            error!(
                "Invalid time range: after={} >= before={}",
                request.after, request.before
            );
            self.metrics.call_metrics.update(|m| m.failed += 1);
            return Err(netdata_plugin_error::NetdataPluginError::Other {
                message: "Invalid time range: after must be less than before".to_string(),
            });
        }

        info!(
            "Creating histogram request: after={}, before={}",
            request.after, request.before
        );
        let histogram_request =
            HistogramRequest::new(request.after, request.before, &[], &filter_expr);

        info!("Acquiring write lock on histogram cache");
        let mut cache = self.histogram_cache.write().await;

        let histogram_result = cache.get_histogram(histogram_request).await;

        info!("Histogram computation complete");

        // // Update cache size metrics
        // self.metrics.cache_size.update(|m| {
        //     m.partial = cache.partial_responses.len() as u64;
        //     m.complete = cache.complete_responses.len() as u64;
        // });

        // // Count bucket response types
        // let mut complete_count = 0u64;
        // let mut partial_count = 0u64;
        // for (_, response) in &histogram_result.buckets {
        //     match response {
        //         journal_query::BucketResponse::Complete(_) => complete_count += 1,
        //         journal_query::BucketResponse::Partial(_) => partial_count += 1,
        //     }
        // }

        // // Update bucket response metrics
        // self.metrics.bucket_responses.update(|m| {
        //     m.complete += complete_count;
        //     m.partial += partial_count;
        // });

        // // Update histogram request metrics
        // self.metrics.histogram_requests.update(|m| {
        //     m.total_buckets += histogram_result.buckets.len() as u64;
        //     // Note: pending_files would need to be tracked during processing
        // });

        // // Track successful call
        // self.metrics.call_metrics.update(|m| m.successful += 1);

        // Release the lock
        drop(cache);

        // Read columns
        let path = "/tmp/columns.json";
        let contents = std::fs::read_to_string(path).unwrap_or_else(|e| {
            warn!("Failed to read columns.json: {}", e);
            "{}".to_string()
        });
        let data: serde_json::Value = serde_json::from_str(&contents).unwrap_or_else(|e| {
            warn!("Failed to parse columns.json: {}", e);
            serde_json::json!({})
        });

        // Get the PRIORITY field name for the histogram
        let priority_field = FieldName::new_unchecked("PRIORITY");

        let response = JournalResponse {
            auxiliary: Auxiliary {
                hello: String::from("world"),
            },
            progress: 0,
            version: Version::default(),
            accepted_params: accepted_params(),
            required_params: required_params(),
            facets: ui::facets(&histogram_result),
            histogram: ui::histogram(&histogram_result, &priority_field),
            available_histograms: ui::available_histograms(&histogram_result),
            columns: data,
            data: Vec::new(),
            default_charts: Vec::new(),
            show_ids: false,
            has_history: true,
            status: 200,
            response_type: String::from("table"),
            help: String::from("View, search and analyze systemd journal entries."),
            pagination: Pagination::default(),
        };

        info!(
            "Successfully created response with {} facets",
            response.facets.len()
        );
        Ok(response)
    }

    async fn on_cancellation(&self) -> Result<Self::Response> {
        warn!("Function call was cancelled by user");

        // Track cancelled call
        self.metrics.call_metrics.update(|m| m.cancelled += 1);

        Err(netdata_plugin_error::NetdataPluginError::Other {
            message: "Operation cancelled by user".to_string(),
        })
    }

    async fn on_progress(&self) {
        info!("Progress report requested for function call");
    }

    fn declaration(&self) -> FunctionDeclaration {
        info!("Generating function declaration for systemd-journal");
        let mut func_decl = FunctionDeclaration::new(
            "systemd-journal",
            "Query and visualize systemd journal entries with histograms and facets",
        );
        func_decl.global = true;
        func_decl.tags = Some(String::from("logs"));
        func_decl.access =
            Some(HttpAccess::SIGNED_ID | HttpAccess::SAME_SPACE | HttpAccess::SENSITIVE_DATA);
        func_decl
    }
}

/// Create shared histogram cache
#[instrument(name = "create_histogram_cache")]
fn create_histogram_cache()
-> std::result::Result<Arc<RwLock<HistogramService>>, Box<dyn std::error::Error>> {
    info!("Creating histogram cache");

    let path = "/var/log/journal";
    let cache_dir = "/mnt/ramfs/foyer-storage";
    let memory_capacity = 10000;
    let disk_capacity = 64 * 1024 * 1024;

    info!("Journal path: {}", path);
    info!("Cache dir: {}", cache_dir);
    info!("Memory capacity: {}", memory_capacity);
    info!("Disk capacity: {} bytes", disk_capacity);

    // Create index cache synchronously in a blocking context
    let indexing_service = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            IndexingService::new(
                tokio::runtime::Handle::current(),
                cache_dir,
                memory_capacity,
                disk_capacity,
            )
            .await
        })
    })?;

    info!("Indexing service created successfully");

    let histogram_service = HistogramService::new(path, indexing_service)?;
    info!("Histogram cache initialized");

    Ok(Arc::new(RwLock::new(histogram_service)))
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Tell netdata to trust durations of metrics we are going to emit
    println!("TRUST_DURATIONS 1");

    // Initialize tracing FIRST so we can see all logs
    tracing_config::initialize_tracing(tracing_config::TracingConfig::default());

    info!("Log Viewer Plugin starting...");

    let histogram_cache = create_histogram_cache()?;
    info!("Histogram cache created successfully");

    let mut runtime = PluginRuntime::new("log-viewer");
    info!("Plugin runtime created");

    // Register all metric charts
    let metrics = Metrics::new(&mut runtime);
    info!("Metrics charts registered");

    // Register journal function handler
    runtime.register_handler(Journal {
        metrics,
        histogram_cache,
    });
    info!("Journal function handler registered");

    info!("Starting plugin runtime - ready to process function calls");
    runtime.run().await?;

    info!("Plugin runtime stopped");
    Ok(())
}
