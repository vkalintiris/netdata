# nm - Netdata Metrics OTLP Ingestion Plugin

This crate implements an OpenTelemetry metrics ingestion pipeline for Netdata. It handles the conversion from OpenTelemetry's event-based model to Netdata's fixed-interval collection model.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           NetdataMetricsService                             │
│                         (gRPC OTLP endpoint :4317)                          │
└─────────────────────────────────┬───────────────────────────────────────────┘
                                  │
                                  ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│                              ChartManager                                   │
│  ┌───────────────────────────────────────────────────────────────────────┐  │
│  │                    ChartState (per chart)                             │  │
│  │    ├── Chart (wraps SlotManager with appropriate aggregator type)     │  │
│  │    │   └── SlotManager<A: Aggregator>                                 │  │
│  │    │       ├── dimensions: HashMap<DimensionId, A>                    │  │
│  │    │       ├── buffered: HashMap<(slot, dim), Vec<BufferedPoint>>     │  │
│  │    │       └── pending_slots: BTreeSet<u64>                           │  │
│  │    └── dimension_names: HashMap<DimensionId, String>                  │  │
│  └───────────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Core Problem

**OpenTelemetry** uses an event-based model where metrics arrive at arbitrary times with explicit timestamps.

**Netdata** expects values at fixed intervals (e.g., every 10 seconds).

This creates two challenges:
1. Multiple OTel data points may arrive within a single Netdata slot
2. Some slots may receive no data at all

## Module Structure

| Module | Purpose |
|--------|---------|
| `aggregation.rs` | `Aggregator` trait and implementations for Gauge, DeltaSum, CumulativeSum |
| `slot.rs` | `SlotManager` - time bucketing, grace period handling, finalization |
| `chart.rs` | `Chart` - type-erased wrapper over SlotManager with different aggregators |
| `service.rs` | gRPC service, `ChartManager`, background tick task |
| `otel.rs` | OTLP normalization, comparison, hashing, data point extraction |
| `iter.rs` | Hierarchical iteration over OTLP requests with full context |
| `config.rs` | Chart configuration management |

## Aggregation Logic

### Aggregator Trait

```rust
pub trait Aggregator {
    fn ingest(&mut self, value: f64, timestamp_ns: u64, start_time_ns: u64);
    fn finalize_slot(&mut self) -> Option<f64>;
    fn gap_fill(&self) -> f64;
    fn reset(&mut self);
}
```

### Aggregator Types

| Metric Type | OTel Temporality | Aggregator | Multi-value Strategy | Gap Fill |
|-------------|------------------|------------|---------------------|----------|
| Gauge | N/A | `GaugeAggregator` | Keep last by timestamp | Last emitted value |
| Sum | Delta | `DeltaSumAggregator` | Sum all deltas | 0 |
| Sum | Cumulative | `CumulativeSumAggregator` | Keep last → compute delta | 0 |

### Cumulative Sum Handling

Cumulative sums require stateful processing:

1. **First observation**: Establishes baseline, returns `None`
2. **Subsequent observations**: Computes delta = `current - previous`
3. **Restart detection**: When `start_time_unix_nano` changes, the counter has reset

```
Slot:     S0        S1        S2        S3 (restart)   S4
Cumul:    100       150       180       20             35
Delta:    None      50        30        0 (restart)    15
```

## Slot Management

### Slot Assignment

```rust
fn slot_for_timestamp(timestamp_ns: u64, interval_secs: u64) -> u64 {
    let timestamp_secs = timestamp_ns / 1_000_000_000;
    (timestamp_secs / interval_secs) * interval_secs
}
```

### Timing Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `interval_secs` | 10 | Collection interval (slot size) |
| `grace_period_secs` | 60 | How long to accept late-arriving data |

### Finalization Triggers

1. **Eager finalization**: When data for slot N+1 arrives, slot N is finalized immediately (low latency)
2. **Tick-based finalization**: Background task runs every `interval_secs` and finalizes slots past their grace period

```
Time:     T0        T10       T20       T70       T80
          |---------|---------|---------|---------|
Data:     [S0]      [S10]
                    ↓                   ↓
              Eager finalize S0   Tick finalize S10
              (saw S10 data)      (grace period expired)
```

### Gap Filling

When a slot is finalized without data for a known dimension:

| Aggregator Type | Gap Fill Value |
|-----------------|----------------|
| Gauge | Last emitted value |
| Delta Sum | 0 (no change) |
| Cumulative Sum | 0 (no change) |

## Data Flow

```
OTLP Request arrives
        │
        ▼
normalize_request()  ─────────────────────  Sort attributes for consistent hashing
        │
        ▼
For each DataPointContext:
    ├── Extract: value, timestamps, dimension name
    ├── Build chart name: "{metric_name}.{attrs_hash}"
    ├── Get/create Chart (based on data_kind + temporality)
    ├── Get dimension ID (hash of dimension name)
    └── chart.ingest(dim_id, value, timestamp_ns, start_time_ns)
        │
        ▼
eager_finalize_all()  ────────────────────  Finalize slots with later data
        │
        ▼
emit_slot()  ─────────────────────────────  Output (currently println, later Netdata protocol)
```

## Background Tick Task

```rust
pub fn spawn_tick_task(
    chart_manager: Arc<RwLock<ChartManager>>,
    tick_interval: Duration,
) -> TickTaskHandle
```

The tick task:
- Runs every `tick_interval` (default: same as collection interval)
- Calls `tick_all(current_time_ns)` on the chart manager
- Emits any finalized slots

## Output Format (Current - Placeholder)

```
CHART system.cpu.usage.12345678 @ 1704067220 (slot_timestamp=1704067220)
  DIM user = 42.500000
  DIM system = 15.300000
  DIM idle = 42.200000 (gap-fill)
```

## Key Design Decisions

1. **Plugin-managed state for cumulative sums**: Instead of using Netdata's `incremental` algorithm, we compute deltas ourselves. This allows precise restart detection using `start_time_unix_nano`.

2. **All dimensions use Netdata's `absolute` algorithm**: We always emit the computed value, never raw cumulative values.

3. **Buffered points with ordered finalization**: Data points are buffered by (slot, dimension) and fed to aggregators only at finalization time. This allows handling multiple pending slots during the grace period.

4. **Eager + tick-based finalization**: Provides low latency in the happy path while still handling late-arriving data.

## Usage

```rust
// Create shared state
let chart_config = ChartConfig::default();
let chart_manager = Arc::new(RwLock::new(ChartManager::new(chart_config)));

// Create service
let svc = NetdataMetricsService::new(ccm, Arc::clone(&chart_manager));

// Spawn tick task
let tick_handle = spawn_tick_task(chart_manager, Duration::from_secs(10));

// Run gRPC server
Server::builder()
    .add_service(MetricsServiceServer::new(svc))
    .serve(addr)
    .await?;

// Cleanup
tick_handle.abort();
```

## Future Work

- [ ] Netdata plugin protocol output (CHART, DIMENSION, SET, END commands)
- [ ] Configurable interval and grace period
- [ ] Dimension expiration (archive after 15 minutes of inactivity)
- [ ] Histogram support
