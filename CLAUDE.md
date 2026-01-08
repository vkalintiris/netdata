# CLAUDE.md

The primary goal of this git worktree is adding support for ingesting
OpenTelemetry metrics to Netdata via an external plugin.

## Existing ingestion pipeline

There's an existing implementation in the `otel-plugin` crate that tries to
automatically discover the collection frequency of incoming metrics.

Due to OpenTelemetry's event-based model, metrics do not have a constant or
implicit collection interval.

## New ingestion pipeline

The `nm` crate is the playground of the new plugin that will try to address
the limitations of the existing `otel-plugin`.

It will do so by defining a global collection interval that will be applied
to all the Netdata charts it will create.

As part of this effort it needs to perform proper aggregation of incoming
metrics and account for the different aggregation temporalities.

Currently:

- We only care about gauges and sums, ie. no histograms, exponential histogram and summaries.
- The collection interval is not configurable, it should be assumed to be 1 second.
- OpenTelemetry's event-based model means we might get multiple values within a given collection interval, or none for a given collection interval.

## OpenTelemetry and Netdata plugins context

- OpenTelemetry's specification can be found at ~/repos/tmp/otel-netdata/opentelemetry-specification.
- OpenTelemetry's protobuf message definitions can be found at ~/repos/tmp/otel-netdata/opentelemetry-proto.
- OpenTelemetry's Rust SDK can provide more information by looking at a concrete implementation and can be found at ~/repos/tmp/otel-netdata/opentelemetry-rust.
- Netdata's external plugin documentation can be found at ~/repos/tmp/otel-netdata/netdata/src/plugins.d.

Regarding, Netdata's external plugin protocol:

An undocumented feature is setting explicitly the timestamp of a chart update,
(ie. for a given collection interval slot). This feature supports only monotonically
increasing timestamps (ie. you can not specify an older collection interval slot
once it's been set). The existing `otel-plugin` makes use of this feature.

## Notes

The core issue revolves around proper handling of aggregation temporalities
and mapping the event-based model of OpenTelemetry to Netdata's fixed/regular
time-based collection interval model.

You are an expert in the observability/monitoring domain and provide
suggestions that are correct, coherent, and identify/handle all the corner cases.
