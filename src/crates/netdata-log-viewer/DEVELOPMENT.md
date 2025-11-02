# Log Viewer Plugin - Development Guide

## Architecture Overview

The log-viewer plugin is a Netdata external plugin that runs in **production mode only** - it reads from stdin and writes to stdout using Netdata's external plugin protocol.

```
Netdata Agent
    ↓ (spawns)
log-viewer-plugin (stdin/stdout)
    ↓ (tracing)
Jaeger/Stderr
```

### Key Design Principles

1. **Single Mode**: No TCP bridges, no HTTP servers - just production mode
2. **Observability First**: Comprehensive tracing and logging for development visibility
3. **Fast Iteration**: Rebuild only the plugin, not all of Netdata
4. **Shared State**: All functionality uses a single `Arc<RwLock<AppState>>`

## Development Workflow

### Prerequisites

1. **Jaeger** (for distributed tracing - optional but highly recommended):
```bash
# NOTE: Port mapping 4318:4317 avoids conflict with Netdata's otel-plugin on host 4317
docker run -d --name jaeger \
  -p 16686:16686 \
  -p 4318:4317 \
  jaegertracing/all-in-one:latest
```

2. **Netdata** with the plugin configured

### Building

```bash
# Build the plugin
cargo build --bin log-viewer-plugin

# Or for release
cargo build --bin log-viewer-plugin --release
```

### Running with Netdata

#### Option 1: Let Netdata spawn the plugin (recommended)

1. Configure Netdata to spawn your plugin:
```bash
# In /etc/netdata/netdata.conf or similar
[plugins]
    log-viewer = yes
```

2. Set environment variables for logging:
```bash
# In /etc/netdata/go.d/log-viewer.conf or plugin config
[log-viewer-plugin]
    env RUST_LOG = "debug,log_viewer_plugin=trace,histogram_service=debug"
```

3. Start Netdata:
```bash
sudo netdata -D  # -D runs in foreground with debug output
```

#### Option 2: Manual testing with Netdata running

If Netdata is already running, you can test the plugin manually:

```bash
# Netdata will pipe stdin/stdout to your plugin
sudo /path/to/target/debug/log-viewer-plugin
```

### Viewing Logs and Traces

#### Stderr Logs

The plugin logs to stderr with detailed information:

```bash
# When running Netdata in foreground (-D), you'll see:
[log-viewer-plugin] Initializing tracing...
[log-viewer-plugin] Tracing initialized - logs to stderr, traces to Jaeger
[log-viewer-plugin] Log Viewer Plugin starting...
2025-11-04T10:30:15.123Z INFO log_viewer_plugin: Creating shared state
2025-11-04T10:30:15.456Z INFO log_viewer_plugin: Plugin runtime created
```

#### Jaeger Traces

1. Open Jaeger UI: http://localhost:16686
2. Select service: `log-viewer-plugin`
3. Click "Find Traces"

You'll see:
- **journal_function_call** spans showing each function invocation
- Request parameters (after, before, num_selections)
- Duration of histogram computation
- Error traces if something fails

#### Adjusting Log Levels

```bash
# More verbose logging
export RUST_LOG="trace"

# Specific module logging
export RUST_LOG="log_viewer_plugin=trace,histogram_service=debug,journal=info"

# Production level
export RUST_LOG="info"
```

### Triggering Function Calls

From the Netdata UI:
1. Navigate to the Functions tab
2. Find "systemd-journal" function
3. Execute with parameters
4. Check Jaeger for the trace

Or via netdatacli:
```bash
# Example function call
echo 'FUNCTION systemd-journal {"after": 0, "before": 9999999999, "selections": {}}' | \
  sudo -u netdata /usr/libexec/netdata/plugins.d/log-viewer-plugin
```

## Code Structure

```
log-viewer-plugin/
├── src/
│   └── main.rs          # Single-file plugin
├── Cargo.toml
└── README.md
```

### Key Components

1. **Journal Handler** (`Journal` struct)
   - Implements `FunctionHandler` trait
   - Handles function calls from Netdata
   - Tracks metrics (successful/failed/cancelled)

2. **Shared State** (`SharedState` type alias)
   - `Arc<RwLock<histogram_service::AppState>>`
   - Contains journal index cache
   - Shared between all handlers

3. **Tracing Setup** (`initialize_tracing()`)
   - Stderr logs for immediate visibility
   - OTLP exporter for Jaeger
   - Configurable via `RUST_LOG`

## Debugging Tips

### Plugin Not Starting

```bash
# Check Netdata logs
sudo tail -f /var/log/netdata/error.log

# Run plugin manually to see errors
sudo -u netdata /path/to/log-viewer-plugin
```

### Slow Function Calls

Check Jaeger traces for:
- Lock contention on shared state
- Slow `get_histogram` calls
- Cache misses in IndexCache

### Memory Issues

```bash
# Monitor memory usage
ps aux | grep log-viewer-plugin

# Add allocator tracking (see histogram-service features)
cargo build --features allocative
```

### Missing Traces

Ensure Jaeger is running:
```bash
# Check if Jaeger is accepting connections on our custom port
nc -zv localhost 4318

# Restart Jaeger if needed
docker restart jaeger
```

## Performance Considerations

1. **Cache Configuration**
   - Default: 10,000 entries in memory, 64MB on disk
   - Location: `/mnt/ramfs/foyer-storage`
   - Adjust in `create_shared_state()` function

2. **Lock Contention**
   - Uses `RwLock` for shared state
   - Multiple readers, single writer
   - Monitor with tracing spans

3. **Journal Path**
   - Default: `/var/log/journal`
   - Change in `create_shared_state()` if needed

## Adding New Features

### Adding a new function

1. Create request/response types in `types` crate
2. Add handler struct implementing `FunctionHandler`
3. Register in `main()`:
```rust
runtime.register_handler(MyNewHandler { state: shared_state });
```

### Adding tracing to existing code

```rust
use tracing::{info, warn, error, instrument};

#[instrument(skip(complex_param), fields(id = simple_param))]
async fn my_function(simple_param: i64, complex_param: &BigStruct) {
    info!("Starting operation");
    // ... your code
    warn!("Something unusual happened");
}
```

## Troubleshooting

### "Failed to build OTLP exporter"

Jaeger isn't running or port 4318 is blocked:
```bash
docker ps | grep jaeger  # Should show running container
```

### "Cannot open journal path"

Check permissions and path:
```bash
ls -la /var/log/journal
sudo -u netdata ls /var/log/journal  # Test as netdata user
```

### "Failed to create cache directory"

Ensure the cache directory exists and is writable:
```bash
sudo mkdir -p /mnt/ramfs/foyer-storage
sudo chown netdata:netdata /mnt/ramfs/foyer-storage
```

## Migration Notes

### From Old Architecture

If you were using:
- `watcher-plugin` (TCP bridge) - **No longer needed, remove**
- `histogram-service` (HTTP server) - **No longer needed, library only**
- `--tcp` mode - **Removed, use stdio only**
- curl testing - **Use Netdata UI or netdatacli instead**

The new approach is simpler and uses real production setup for development.

## Benefits of This Approach

✅ **Simplicity**: One mode, one way to run
✅ **Production Parity**: Develop with exact production setup
✅ **Fast Feedback**: Logs and traces show what's happening
✅ **No Mock Infrastructure**: No TCP bridges, no test harnesses
✅ **Easy Debugging**: Jaeger shows request timelines visually
✅ **Quick Iteration**: Rebuild plugin in seconds, Netdata restarts it

## Next Steps

1. Start Jaeger: `docker run -d -p 16686:16686 -p 4318:4317 jaegertracing/all-in-one`
2. Build plugin: `cargo build --bin log-viewer-plugin`
3. Configure Netdata to use your binary
4. Start Netdata: `sudo netdata -D`
5. Open Jaeger: http://localhost:16686
6. Make function calls from Netdata UI
7. Watch traces appear in Jaeger!
