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
    Facets, FileIndexCache, FileInfo, HistogramRequest, HistogramResponse, HistogramService,
    IndexingService, Monitor, Registry, Result as CatalogResult,
    histogram::{BucketCompleteResponse, BucketRequest, BucketResponse},
    schema::types,
    schema::ui,
};

/*
 * CatalogFunction
*/
use std::collections::HashMap;

/// Request parameters for the catalog function (uses journal request structure)
pub type CatalogRequest = types::JournalRequest;

/// Response from the catalog function (uses journal response structure)
pub type CatalogResponse = types::JournalResponse;

use journal::index::Filter;
use journal::{FieldName, FieldValuePair};

/*
 * Helper Functions
 */

/// Converts catalog's HistogramResponse to journal_query's HistogramResponse
fn convert_histogram_response(catalog_response: &HistogramResponse) -> HistogramResponse {
    let buckets = catalog_response
        .buckets
        .iter()
        .map(|(bucket_req, bucket_resp)| {
            // Convert BucketRequest
            // Convert catalog::Facets (currently just clones as same type)
            let jq_facets = Facets::new(
                bucket_req
                    .facets
                    .iter()
                    .map(|f| f.as_str().to_string())
                    .collect::<Vec<_>>()
                    .as_slice(),
            );

            let jq_bucket_req = BucketRequest {
                start: bucket_req.start,
                end: bucket_req.end,
                facets: jq_facets,
                filter_expr: bucket_req.filter_expr.clone(),
            };

            // Convert BucketResponse
            let jq_bucket_resp = if bucket_resp.is_complete() {
                BucketResponse::complete(BucketCompleteResponse {
                    fv_counts: bucket_resp.fv_counts().clone(),
                    unindexed_fields: bucket_resp.unindexed_fields().clone(),
                })
            } else {
                // For partial responses, we need to extract the metadata
                // Since catalog uses interior mutability, we can't access the inner partial data directly
                // We'll convert partial to complete for simplicity since UI doesn't distinguish
                BucketResponse::complete(BucketCompleteResponse {
                    fv_counts: bucket_resp.fv_counts().clone(),
                    unindexed_fields: bucket_resp.unindexed_fields().clone(),
                })
            };

            (jq_bucket_req, jq_bucket_resp)
        })
        .collect();

    HistogramResponse { buckets }
}

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

fn accepted_params() -> Vec<types::RequestParam> {
    use types::RequestParam;

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

fn required_params() -> Vec<types::RequiredParam> {
    let mut v = Vec::new();

    let id = types::RequestParam::LogsSources;
    let name = String::from("Journal Sources");
    let help = String::from("Select the logs source to query");
    let type_ = String::from("multiselect");
    let mut options = Vec::new();

    let o1 = types::MultiSelectionOption {
        id: String::from("all"),
        name: String::from("all"),
        pill: String::from("100GiB"),
        info: String::from("All the logs"),
    };
    options.push(o1);

    let required_param = types::RequiredParam::MultiSelection(types::MultiSelection {
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
    /// Create a new catalog function with the given monitor and file index cache
    pub fn new(monitor: Monitor, file_index_cache: FileIndexCache) -> Self {
        let registry = Registry::new(monitor);

        // Create indexing service with 24 workers and queue capacity of 100
        let indexing_service = IndexingService::new(
            file_index_cache.clone(),
            registry.clone(),
            24,
            100,
        );

        // Create histogram service
        let histogram_service = HistogramService::new(
            registry.clone(),
            indexing_service.clone(),
            file_index_cache.clone(),
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
        let facets: Vec<String> = Vec::new(); // TODO: Extract from request if provided
        let histogram_request =
            HistogramRequest::new(request.after, request.before, &facets, &filter_expr);

        info!("Getting histogram from catalog");
        let histogram_response = self.get_histogram(histogram_request).await.map_err(|e| {
            netdata_plugin_error::NetdataPluginError::Other {
                message: format!("Failed to get histogram: {}", e),
            }
        })?;
        info!("Histogram computation complete");

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

        // We need to convert our histogram response to the format expected by journal_query::ui
        // let jq_histogram_response = convert_histogram_response(&histogram_result);

        let response = CatalogResponse {
            auxiliary: types::Auxiliary {
                hello: String::from("world"),
            },
            progress: 0,
            version: types::Version::default(),
            accepted_params: accepted_params(),
            required_params: required_params(),
            facets: ui::facets(&histogram_response),
            histogram: ui::histogram(&histogram_response, &priority_field),
            available_histograms: ui::available_histograms(&histogram_response),
            columns: data,
            data: Vec::new(),
            default_charts: Vec::new(),
            show_ids: false,
            has_history: true,
            status: 200,
            response_type: String::from("table"),
            help: String::from("View, search and analyze systemd journal entries."),
            pagination: types::Pagination::default(),
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
