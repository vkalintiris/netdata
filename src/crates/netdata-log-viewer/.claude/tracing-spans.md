# Tracing Spans for Histogram Requests

## Overview

We've added structured tracing spans to the histogram request processing pipeline. This provides visibility into request handling, making it easier to:

- Debug performance issues
- Understand request flow
- Track partial vs complete responses
- Monitor cache effectiveness

## Instrumented Functions

### `get_histogram()`

The top-level span that tracks the entire histogram request lifecycle.

**Span fields:**
- `after`: Start timestamp of the time range
- `before`: End timestamp of the time range

**Debug events:**
- `complete`: Number of complete bucket responses
- `partial`: Number of partial bucket responses
- `total`: Total number of buckets returned

**Example trace:**
```
get_histogram{after=1730736000 before=1730739600}
  ├─ process_histogram_request{after=1730736000 before=1730739600 time_range=3600 num_facets=5}
  │  ├─ Creating partial responses num_buckets=60
  │  ├─ Collected pending files pending_files=12
  │  └─ ...
  └─ Histogram result collected complete=45 partial=15 total=60
```

### `process_histogram_request()`

Nested span for the histogram processing logic.

**Span fields:**
- `after`: Start timestamp
- `before`: End timestamp
- `time_range`: Duration of the time range (before - after)
- `num_facets`: Number of facets being tracked

**Debug events:**
- `num_buckets`: Number of time buckets to process
- `pending_files`: Number of journal files needing indexing

## Viewing Traces

### Jaeger UI

With the default configuration, traces are sent to Jaeger on port 4318:

```bash
# View traces in Jaeger
open http://localhost:16686

# Search for:
# - Service: log-viewer-plugin or histogram-service
# - Operation: get_histogram or process_histogram_request
```

### Systemd Journal

When running under Netdata with journal logging, span information is included in structured logs:

```bash
journalctl SYSLOG_IDENTIFIER=log-viewer-plugin -o json-pretty
```

Each log entry includes:
- `SPAN_NAME`: Array of active span names
- `SPAN_TARGET`: Array of span targets (module names)
- `SPAN_CODE_FILE`: Array of source files
- `SPAN_CODE_LINE`: Array of line numbers

## Benefits

1. **Performance analysis**: See exactly where time is spent in request processing
2. **Cache effectiveness**: Track complete vs partial response ratios
3. **Debugging**: Understand request flow and identify bottlenecks
4. **Monitoring**: Export to observability platforms via OpenTelemetry

## Adding More Spans

To add spans to other functions:

```rust
use tracing::instrument;

#[instrument(skip(self), fields(
    custom_field = some_value,
    other_field = request.field,
))]
async fn my_function(&mut self, request: &Request) {
    debug!(detail = "something", "Event message");
    // ... function body
}
```

**Guidelines:**
- Use `skip(self)` to avoid cloning large state
- Include key identifiers in span fields (timestamps, IDs, counts)
- Use `debug!`/`info!`/`warn!` for important events within spans
- Keep span field values simple (primitives, not complex types)

## Example Queries

### In Jaeger

**Find slow histogram requests:**
- Service: `log-viewer-plugin`
- Operation: `get_histogram`
- Min Duration: `100ms`

**Analyze time ranges:**
- Tag: `time_range > 7200` (requests over 2 hours)

**Track cache hit rate:**
- Look at `complete` vs `partial` in span logs
