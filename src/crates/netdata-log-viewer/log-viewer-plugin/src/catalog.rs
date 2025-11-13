//! Journal catalog functionality with file monitoring and metadata tracking

use async_trait::async_trait;
use netdata_plugin_error::Result;
use netdata_plugin_protocol::FunctionDeclaration;
use netdata_plugin_schema::HttpAccess;
use rt::FunctionHandler;
use std::sync::Arc;
use tracing::{error, info, instrument, warn};

// Import types from journal-function crate
use journal_function::{
    BucketCacheMetrics, BucketOperationsMetrics, Facets, FileIndexCache, FileIndexKey,
    FileIndexingMetrics, FileInfo, HistogramRequest, HistogramResponse, HistogramService,
    IndexingService, Monitor, Registry, Result as CatalogResult, netdata,
};
use rt::ChartHandle;

/*
 * CatalogFunction
*/
use std::collections::HashMap;

/// Request parameters for the catalog function (uses journal request structure)
pub type CatalogRequest = netdata::JournalRequest;

/// Response from the catalog function (uses journal response structure)
pub type CatalogResponse = netdata::JournalResponse;

use journal::index::Filter;
use journal::{FieldName, FieldValuePair};

/// Builds a Filter from the selections HashMap
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

fn accepted_params() -> Vec<netdata::RequestParam> {
    use netdata::RequestParam;

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

fn required_params() -> Vec<netdata::RequiredParam> {
    let mut v = Vec::new();

    let id = netdata::RequestParam::LogsSources;
    let name = String::from("Journal Sources");
    let help = String::from("Select the logs source to query");
    let type_ = String::from("multiselect");
    let mut options = Vec::new();

    let o1 = netdata::MultiSelectionOption {
        id: String::from("all"),
        name: String::from("all"),
        pill: String::from("100GiB"),
        info: String::from("All the logs"),
    };
    options.push(o1);

    let required_param = netdata::RequiredParam::MultiSelection(netdata::MultiSelection {
        id,
        name,
        help,
        type_,
        options,
    });

    v.push(required_param);
    v
}

/// Inner state for CatalogFunction (enables cloning)
struct CatalogFunctionInner {
    registry: Registry,
    file_index_cache: FileIndexCache,
    indexing_service: IndexingService,
    histogram_service: Arc<HistogramService>,
}

/// Function handler that provides catalog information about journal files
#[derive(Clone)]
pub struct CatalogFunction {
    inner: Arc<CatalogFunctionInner>,
}

impl CatalogFunction {
    /// Query log entries from the indexed files (generic).
    ///
    /// This method:
    /// 1. Finds journal files in the time range
    /// 2. Retrieves indexed files from cache
    /// 3. Queries log entries using LogQuery
    /// 4. Returns raw log entry data
    async fn query_logs(
        &self,
        after: u32,
        before: u32,
        anchor: Option<u64>,
        facets: &[String],
        limit: usize,
        direction: journal::index::Direction,
    ) -> Vec<journal_function::logs::LogEntryData> {
        use journal_function::logs::LogQuery;

        info!("Querying logs for time range [{}, {})", after, before);

        // Find files in the time range
        let file_infos = match self.inner.registry.find_files_in_range(after, before) {
            Ok(files) => files,
            Err(e) => {
                warn!("Failed to find files in range: {}", e);
                return Vec::new();
            }
        };

        info!("Found {} files in range", file_infos.len());

        // Collect indexed files from cache
        let mut indexed_files = Vec::new();
        let facets_obj = Facets::new(facets);

        for file_info in file_infos.iter() {
            let key = FileIndexKey::new(&file_info.file, &facets_obj);
            match self.inner.file_index_cache.get(&key).await {
                Ok(Some(index)) => indexed_files.push(index),
                Ok(None) => continue,
                Err(e) => {
                    warn!("Failed to get index from cache: {}", e);
                    continue;
                }
            }
        }

        info!("Found {} indexed files in cache", indexed_files.len());

        if indexed_files.is_empty() {
            info!("No indexed files available for log query");
            return Vec::new();
        }

        // Query log entries
        // Use provided anchor (in microseconds), or compute from after/before based on direction
        let anchor_usec = anchor.unwrap_or_else(|| {
            match direction {
                journal::index::Direction::Forward => after as u64 * 1_000_000,
                journal::index::Direction::Backward => before as u64 * 1_000_000,
            }
        });

        match LogQuery::new(&indexed_files)
            .with_direction(direction)
            .with_anchor_usec(anchor_usec)
            .with_limit(limit)
            .execute()
        {
            Ok(log_entries) => {
                info!("Retrieved {} log entries", log_entries.len());
                log_entries
            }
            Err(e) => {
                error!("Log query error: {}", e);
                Vec::new()
            }
        }
    }

    /// Create a new catalog function with the given monitor, file index cache, and metrics
    pub fn new(
        monitor: Monitor,
        file_index_cache: FileIndexCache,
        file_indexing_metrics: ChartHandle<FileIndexingMetrics>,
        bucket_cache_metrics: ChartHandle<BucketCacheMetrics>,
        bucket_operations_metrics: ChartHandle<BucketOperationsMetrics>,
    ) -> Self {
        let registry = Registry::new(monitor);

        // Create indexing service with 24 workers and queue capacity of 100
        let indexing_service = IndexingService::new(
            file_index_cache.clone(),
            registry.clone(),
            24,
            100,
            file_indexing_metrics,
        );

        // Create histogram service
        let histogram_service = HistogramService::new(
            registry.clone(),
            indexing_service.clone(),
            file_index_cache.clone(),
            bucket_cache_metrics,
            bucket_operations_metrics,
        );

        let inner = CatalogFunctionInner {
            registry,
            file_index_cache,
            indexing_service,
            histogram_service: Arc::new(histogram_service),
        };

        Self {
            inner: Arc::new(inner),
        }
    }

    /// Get a reference to the histogram service
    pub fn histogram_service(&self) -> Arc<HistogramService> {
        self.inner.histogram_service.clone()
    }

    /// Get a histogram for the given request
    pub async fn get_histogram(
        &self,
        request: HistogramRequest,
    ) -> CatalogResult<HistogramResponse> {
        self.inner.histogram_service.get_histogram(request).await
    }

    /// Watch a directory for journal files
    pub fn watch_directory(&self, path: &str) -> Result<()> {
        self.inner.registry.watch_directory(path).map_err(|e| {
            netdata_plugin_error::NetdataPluginError::Other {
                message: format!("Failed to watch directory: {}", e),
            }
        })
    }

    /// Stop watching a directory for journal files
    pub fn unwatch_directory(&self, path: &str) -> Result<()> {
        self.inner.registry.unwatch_directory(path).map_err(|e| {
            netdata_plugin_error::NetdataPluginError::Other {
                message: format!("Failed to unwatch directory: {}", e),
            }
        })
    }

    /// Find files in a time range
    pub fn find_files_in_range(&self, start: u32, end: u32) -> Vec<FileInfo> {
        match self.inner.registry.find_files_in_range(start, end) {
            Ok(files) => files,
            Err(e) => {
                error!("Failed to find files in range: {}", e);
                Vec::new()
            }
        }
    }

    /// Process a notify event
    pub fn process_notify_event(&self, event: notify::Event) {
        if let Err(e) = self.inner.registry.process_event(event) {
            error!("Failed to process notify event: {}", e);
        }
    }
}

#[async_trait]
impl FunctionHandler for CatalogFunction {
    type Request = CatalogRequest;
    type Response = CatalogResponse;

    #[instrument(name = "catalog_function_call", skip_all, fields(
        after = request.after,
        before = request.before,
        num_selections = request.selections.len()
    ))]
    async fn on_call(&self, request: Self::Request) -> Result<Self::Response> {
        info!("Processing catalog function call");

        let filter_expr = build_filter_from_selections(&request.selections);

        if request.after >= request.before {
            error!(
                "Invalid time range: after={} >= before={}",
                request.after, request.before
            );
            return Err(netdata_plugin_error::NetdataPluginError::Other {
                message: "Invalid time range: after must be less than before".to_string(),
            });
        }

        info!(
            "Creating histogram request: after={}, before={}",
            request.after, request.before
        );

        // Get facets from request or use empty list
        // FIXME: Need to review this
        let facets: Vec<String> = request
            .facets
            .iter()
            .filter(|f| *f != "__logs_sources") // Ignore special fields
            .cloned()
            .collect();

        let histogram_request =
            HistogramRequest::new(request.after, request.before, &facets, &filter_expr);

        info!("Getting histogram from catalog");
        let histogram_response = self.get_histogram(histogram_request).await.map_err(|e| {
            netdata_plugin_error::NetdataPluginError::Other {
                message: format!("Failed to get histogram: {}", e),
            }
        })?;
        info!("Histogram computation complete");

        let limit = request.last.unwrap_or(200);
        let log_entries = self
            .query_logs(
                request.after,
                request.before,
                request.anchor,
                &facets,
                limit,
                request.direction,
            )
            .await;

        // Build Netdata UI response (columns + data)
        let (columns, data) = netdata::build_ui_response(&histogram_response, &log_entries);

        // Get transformations for histogram chart labels
        let transformations = netdata::systemd_transformations();

        // Determine which field to use for the histogram (default to PRIORITY if not specified)
        let histogram_field_name = if request.histogram.is_empty() {
            "PRIORITY"
        } else {
            &request.histogram
        };
        let histogram_field = FieldName::new_unchecked(histogram_field_name);

        let response = CatalogResponse {
            auxiliary: netdata::Auxiliary {
                hello: String::from("world"),
            },
            progress: histogram_response.progress(),
            version: netdata::Version::default(),
            accepted_params: accepted_params(),
            required_params: required_params(),
            facets: netdata::facets(&histogram_response, &transformations),
            histogram: netdata::histogram(&histogram_response, &histogram_field, &transformations),
            available_histograms: netdata::available_histograms(&histogram_response),
            columns,
            data,
            default_charts: Vec::new(),
            show_ids: false,
            has_history: true,
            status: 200,
            response_type: String::from("table"),
            help: String::from("View, search and analyze systemd journal entries."),
            pagination: netdata::Pagination::default(),
        };

        info!(
            "Successfully created response with {} facets",
            response.facets.len()
        );
        Ok(response)
    }

    async fn on_cancellation(&self) -> Result<Self::Response> {
        warn!("Catalog function call cancelled by Netdata");

        Err(netdata_plugin_error::NetdataPluginError::Other {
            message: "Catalog function cancelled by user".to_string(),
        })
    }

    async fn on_progress(&self) {
        info!("Progress report requested for catalog function call");
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
