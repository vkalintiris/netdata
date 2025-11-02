# Quick Start Guide

## Setup (One Time)

### 1. Start Jaeger for tracing
```bash
# NOTE: Using port 4318 to avoid conflict with Netdata's otel-plugin on 4317
docker run -d --name jaeger \
  -p 16686:16686 \
  -p 4318:4317 \
  jaegertracing/all-in-one:latest
```

### 2. Build the plugin
```bash
cd /home/vk/repos/nd/sjr/src/crates/netdata-log-viewer
cargo build --bin log-viewer-plugin
```

## Development Loop

### Iteration Cycle
```bash
# 1. Make code changes to log-viewer-plugin

# 2. Rebuild (fast - only rebuilds plugin)
cargo build --bin log-viewer-plugin

# 3. Netdata will auto-restart the plugin
#    (or restart Netdata if needed)
sudo systemctl restart netdata

# 4. View logs in real-time
sudo journalctl -u netdata -f | grep log-viewer-plugin

# 5. View traces in Jaeger
open http://localhost:16686
```

## Viewing Plugin Activity

### Method 1: Jaeger UI (Recommended)
1. Open http://localhost:16686
2. Service: `log-viewer-plugin`
3. Click "Find Traces"
4. See timeline of all function calls with details

### Method 2: Stderr Logs
```bash
# If running Netdata in foreground
sudo netdata -D

# Or view from systemd journal
sudo journalctl -u netdata -f
```

### Method 3: Increase Verbosity
```bash
# Set before starting Netdata
export RUST_LOG="trace"
sudo netdata -D
```

## Testing Function Calls

### From Netdata UI
1. Navigate to Functions tab
2. Find "systemd-journal"
3. Execute with parameters
4. Check Jaeger for trace

### Manual Test
```bash
# Echo a function call to plugin stdin
echo 'FUNCTION systemd-journal {"after": 0, "before": 9999999999, "selections": {}}' | \
  sudo -u netdata /path/to/target/debug/log-viewer-plugin
```

## Common Commands

```bash
# Build
cargo build --bin log-viewer-plugin

# Build release
cargo build --bin log-viewer-plugin --release

# Check compilation without building
cargo check --bin log-viewer-plugin

# Run tests
cargo test -p log-viewer-plugin

# Clean and rebuild
cargo clean && cargo build --bin log-viewer-plugin

# View binary size
ls -lh target/debug/log-viewer-plugin

# Check if Jaeger is running
docker ps | grep jaeger

# Restart Jaeger
docker restart jaeger

# Stop Jaeger
docker stop jaeger

# Remove Jaeger
docker rm jaeger
```

## Troubleshooting

### Plugin not starting?
```bash
# Check Netdata error log
sudo tail -f /var/log/netdata/error.log

# Run plugin manually to see errors
sudo -u netdata /path/to/target/debug/log-viewer-plugin
```

### No traces in Jaeger?
```bash
# Verify Jaeger is running and accepting connections
docker ps | grep jaeger
nc -zv localhost 4318

# Check plugin logs for connection errors
sudo journalctl -u netdata | grep "OTLP"
```

### Need more verbose logs?
```bash
export RUST_LOG="trace"
# Then restart Netdata
```

## Architecture Summary

**Old (Complex)**:
```
Netdata → watcher-plugin (TCP) → log-viewer-plugin (HTTP) → histogram-service
```

**New (Simple)**:
```
Netdata → log-viewer-plugin → Jaeger/Stderr
                ↓
        (all business logic internal)
```

**Key Benefits**:
- ✅ Single binary, single mode
- ✅ Real production setup
- ✅ Fast rebuild (seconds)
- ✅ Excellent observability via Jaeger
- ✅ No test infrastructure needed

## What We Removed

- ❌ `watcher-plugin` - No longer needed
- ❌ `histogram-service` binary - Now a library only
- ❌ TCP bridge mode - Production mode only
- ❌ HTTP server for testing - Use Netdata UI instead
- ❌ curl testing - Use Jaeger traces instead

## File Locations

- **Plugin source**: `log-viewer-plugin/src/main.rs`
- **Plugin binary**: `target/debug/log-viewer-plugin`
- **Jaeger UI**: http://localhost:16686
- **Cache directory**: `/mnt/ramfs/foyer-storage`
- **Journal path**: `/var/log/journal`

See `DEVELOPMENT.md` for detailed documentation.
