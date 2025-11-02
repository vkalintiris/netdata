# Log Viewer Plugin Metrics

## Overview

The log-viewer-plugin exposes several Netdata charts to provide visibility into its internal operation and performance.

## Available Charts

### 1. Journal Function Calls (`log_viewer.journal_calls`)

**Family:** `requests`
**Units:** `calls/s`
**Type:** `line`

Tracks the rate and outcome of journal function calls from the Netdata dashboard.

**Dimensions:**
- `successful`: Successful function calls that returned data
- `failed`: Failed function calls (invalid parameters, errors)
- `cancelled`: Function calls cancelled by the user

**Use cases:**
- Monitor request rate and success rate
- Identify error patterns
- Track user cancellations

---

### 2. Response Cache Size (`log_viewer.cache_size`)

**Family:** `cache`
**Units:** `entries`
**Type:** `line`

Tracks the current size of the LRU caches that store histogram bucket responses.

**Dimensions:**
- `partial_responses`: Number of partial bucket responses in cache
- `complete_responses`: Number of complete bucket responses in cache

**Use cases:**
- Monitor cache utilization
- Understand cache hit patterns
- Identify cache pressure (if constantly at max capacity of 1000)
- Track memory usage (each entry has overhead)

**Notes:**
- Cache capacity is 1000 entries for each type
- Partial responses are promoted to complete responses as files are indexed
- High partial count suggests active indexing workload

---

### 3. Bucket Response Types (`log_viewer.bucket_responses`)

**Family:** `responses`
**Units:** `buckets/s`
**Type:** `stacked`

Tracks the rate at which histogram buckets are returned as complete vs partial.

**Dimensions:**
- `complete`: Buckets with fully indexed data
- `partial`: Buckets still waiting for files to be indexed

**Use cases:**
- Monitor cache effectiveness (high complete ratio = good cache performance)
- Identify slow indexing (high partial ratio)
- Track system responsiveness

**Interpretation:**
- **High complete ratio**: Good - data is cached and index is up to date
- **High partial ratio**: System is actively indexing new data or cache is cold
- **100% partial**: Cache miss - new time range or filter being queried

---

### 4. Histogram Request Details (`log_viewer.histogram_requests`)

**Family:** `requests`
**Units:** `count/s`
**Type:** `line`

Tracks characteristics of histogram requests being processed.

**Dimensions:**
- `total_buckets`: Total number of time buckets requested per second
- `pending_files`: Number of journal files needing indexing (future enhancement)

**Use cases:**
- Monitor query complexity (more buckets = larger time ranges)
- Understand indexing workload
- Correlate with performance

**Notes:**
- A typical 1-hour query with 1-minute buckets = 60 buckets
- Larger time ranges (days/weeks) result in higher bucket counts
- More buckets = more cache lookups

---

## Monitoring Recommendations

### Normal Operation

- **Success rate**: > 99%
- **Cache size**: Gradually fills up to 1000, then stabilizes
- **Complete ratio**: > 80% (depends on query patterns)
- **Total buckets**: Varies with time range selections

### Warning Signs

1. **High failure rate**
   - Check logs for errors
   - Verify time range parameters in queries

2. **Cache thrashing**
   - Cache size constantly at max (1000) with high turnover
   - May need larger cache capacity

3. **Low complete ratio**
   - High partial responses suggest:
     - Slow indexing (check pending_files metric when available)
     - Cold cache (after restart)
     - Queries on very recent data (still being indexed)

4. **High cancellation rate**
   - Users are cancelling slow queries
   - May indicate performance issues

### Performance Correlation

Compare these metrics with:
- Response times in Jaeger traces
- System CPU/memory usage
- Journal file count and size
- Query time ranges (from traces)

## Dashboard Examples

### Cache Efficiency

```
Complete Buckets / (Complete + Partial) * 100%
```

A value > 80% indicates good cache performance.

### Request Complexity

```
Total Buckets / Successful Calls
```

Average number of buckets per request. Higher values = longer time ranges.

### Cache Pressure

```
(Partial Responses + Complete Responses) / 1000 * 100%
```

Percentage of cache capacity used. Consistently at 100% suggests need for larger cache.

## Future Enhancements

Potential additional metrics:

- **Index cache stats**: Memory usage, entry count, eviction rate
- **Pending files**: Currently noted as TODO in code
- **Query latency**: Response time distribution (P50, P95, P99)
- **File indexing rate**: Files indexed per second
- **Time range distribution**: Histogram of requested time ranges
