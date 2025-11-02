# Chart Timestamps for Accurate Interpolation

## Problem

Netdata performs interpolation on chart data points to align them to the chart's update interval. When a plugin emits chart updates without specifying exact collection timestamps, Netdata uses the current time when it receives the data. This can cause inaccuracies in the dashboard, especially when:

1. The plugin has processing delays
2. Network latency exists between collection and emission
3. Multiple charts are batched together

## Solution

The Netdata plugin protocol supports specifying exact timestamps for chart data:

```
BEGIN chart_id microseconds
SET dimension = value
END collection_time_secs
```

Where:
- `microseconds`: The update interval in microseconds (helps Netdata understand the collection frequency)
- `collection_time_secs`: Unix timestamp in seconds when the data was actually collected

## Implementation in `rt` Crate

### ChartWriter

Uses proper Rust types for semantic clarity:

```rust
// BEGIN command with update interval (Duration)
pub fn begin_chart(&mut self, chart_id: &str, update_every: Duration)

// END command with collection timestamp (SystemTime)
pub fn end_chart(&mut self, collection_time: SystemTime)
```

**Why these types?**
- `Duration` for intervals - semantically correct, type-safe
- `SystemTime` for timestamps - represents a point in time, not a duration
- Protocol conversion happens internally (Duration → microseconds, SystemTime → Unix seconds)

### TrackedChart

Clean API using standard library types:

```rust
pub fn emit_update(&self, writer: &mut ChartWriter, collection_time: SystemTime)
```

The interval is already stored in the `TrackedChart` and automatically converted to microseconds.

### ChartRegistry

Captures the collection timestamp at the end of each interval:

```rust
let collection_time = std::time::SystemTime::now();
sampler.sample_to_buffer(&mut batch_buffer, collection_time).await;
```

The conversion to Unix seconds happens inside `ChartWriter::end_chart()`.

## Benefits

1. **Accurate interpolation**: Netdata can accurately align data points to the chart's update interval
2. **Correct visualization**: Dashboard displays the data at the correct timestamps
3. **Batch processing**: Multiple charts can be batched without affecting timestamp accuracy
4. **Delayed emission**: Charts can be emitted with slight delays without affecting accuracy

## Example Output

Without timestamps:
```
BEGIN log_viewer.journal_calls
SET successful = 42
SET failed = 1
SET cancelled = 0
END
```

With timestamps:
```
BEGIN log_viewer.journal_calls 1000000
SET successful = 42
SET failed = 1
SET cancelled = 0
END 1730736000
```

## Reference

See the otel-plugin implementation in `/home/vk/repos/nd/sjr/src/crates/netdata-otel/otel-plugin/src/netdata_chart.rs`:
- Line 274-277: `emit_begin` with microseconds
- Line 283-289: `emit_end` with collection time

## Breaking Change

This is a breaking change to the `rt` crate API:
- Timestamps are now **required** (not optional)
- All code using `ChartWriter`, `TrackedChart`, or `ChartRegistry` must be updated
- The registry automatically provides timestamps, so plugin code using `register_chart()` requires no changes
- This ensures accurate interpolation for all charts
