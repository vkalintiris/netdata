# Port Configuration

## Overview

The log-viewer-plugin uses **port 4318** for OTLP/gRPC communication with Jaeger to avoid conflicts with Netdata's built-in otel-plugin.

## Port Usage

### Netdata Services
- **4317** - Netdata's otel-plugin (OTLP gRPC receiver)

### Log Viewer Plugin
- **4318** - Jaeger OTLP receiver (forwarded to internal 4317)
- **16686** - Jaeger Web UI

## Docker Command

```bash
docker run -d --name jaeger \
  -p 16686:16686 \    # Jaeger UI
  -p 4318:4317 \      # Map host 4318 to container 4317
  jaegertracing/all-in-one:latest
```

**Explanation:**
- Jaeger container listens on port 4317 internally
- We map host port 4318 → container port 4317
- Plugin sends to localhost:4318
- No conflict with Netdata's localhost:4317

## Flow Diagram

```
┌─────────────────────┐
│  log-viewer-plugin  │
│  (sends traces)     │
└──────────┬──────────┘
           │
           ▼
    localhost:4318
           │
           │ (Docker port mapping)
           │
           ▼
    ┌──────────────┐
    │   Jaeger     │
    │ (container)  │
    │  port 4317   │
    └──────────────┘
```

Meanwhile:
```
┌─────────────────────┐
│  Netdata Agent      │
│  otel-plugin        │
└──────────┬──────────┘
           │
           ▼
    localhost:4317
    (Netdata's own OTLP receiver)
```

## Code References

### Plugin Configuration
File: `log-viewer-plugin/src/main.rs:318-319`
```rust
.with_endpoint("http://localhost:4318") // Jaeger's OTLP gRPC endpoint
```

## Troubleshooting

### Port Already in Use
```bash
# Check what's using port 4318
lsof -i :4318

# Check what's using port 4317 (should be Netdata)
lsof -i :4317
```

### Connection Refused
```bash
# Verify Jaeger is running and accepting on 4318
nc -zv localhost 4318

# Check Docker port mapping
docker port jaeger
```

### Wrong Port
If you accidentally connect to 4317, traces will go to Netdata's otel-plugin instead of Jaeger. Symptoms:
- No traces visible in Jaeger UI
- Traces may appear in Netdata's own observability (if configured)

## Alternative: Using Environment Variable

If you want to make the port configurable:

```rust
let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
    .unwrap_or_else(|_| "http://localhost:4318".to_string());

let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
    .with_tonic()
    .with_endpoint(endpoint)
    .build()
    .expect("Failed to build OTLP exporter");
```

Then run:
```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="http://localhost:4318"
cargo run --bin log-viewer-plugin
```
