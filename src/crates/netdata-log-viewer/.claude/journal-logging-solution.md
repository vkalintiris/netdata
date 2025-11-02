# Journal Logging Solution

## The Problem

When Netdata was configured to send logs to systemd journal, the log-viewer-plugin's logs were:
1. Appearing on the terminal when running `netdata -D`
2. Not appearing in `journalctl` at all

## Root Cause Analysis

### How Netdata Spawns Plugins (POSIX)

1. **Stdin/Stdout**: Netdata creates pipes for these
2. **Stderr**: Netdata passes `nd_log_collectors_fd()` directly as the child's stderr
3. **No log-forwarder for stderr on POSIX**: Unlike Windows, POSIX spawn doesn't create a pipe and use log-forwarder

### What `nd_log_collectors_fd()` Returns

```c
int nd_log_collectors_fd(void) {
    if(nd_log.sources[NDLS_COLLECTORS].method == NDLM_FILE && nd_log.sources[NDLS_COLLECTORS].fd != -1)
        return nd_log.sources[NDLS_COLLECTORS].fd;  // File descriptor to collector.log

    if(nd_log.sources[NDLS_COLLECTORS].method == NDLM_JOURNAL && nd_log.journal_direct.fd != -1)
        return nd_log.journal_direct.fd;  // Unix socket to systemd journal

    return STDERR_FILENO;  // Terminal stderr
}
```

### The Journal FD Problem

The `journal_direct.fd` is **not a regular file descriptor**. It's a **Unix domain socket** connected to systemd-journald that expects **structured messages** in the format:

```
MESSAGE=Log message here
PRIORITY=6
SYSLOG_IDENTIFIER=log-viewer-plugin
CODE_LINE=123
CODE_FILE=main.rs
...
```

The plugin was writing **plain text** like:
```
2025-11-04T07:36:21.226701Z  INFO ThreadId(01) log_viewer_plugin: 249: Processing...
```

This doesn't work because:
1. Systemd journal expects key=value pairs
2. Plain text gets ignored or misinterpreted

## The Solution

The plugin now:

1. **Detects journal logging** via `NETDATA_SYSTEMD_JOURNAL_PATH` environment variable
2. **Uses appropriate tracing layer**:
   - `tracing-journald`: When journal is configured (formats messages properly)
   - `tracing-subscriber::fmt`: When using stderr normally

### Code Changes

```rust
fn initialize_tracing() {
    // Check if Netdata has configured journal logging
    let use_journal = std::env::var("NETDATA_SYSTEMD_JOURNAL_PATH").is_ok();

    if use_journal {
        // Use journald layer - formats messages for systemd journal
        let journald_layer = tracing_journald::layer()
            .expect("Failed to connect to journald");
        registry.with(journald_layer).init();
    } else {
        // Use stderr layer for normal operation
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_writer(|| std::io::stderr())
            ...
        registry.with(fmt_layer).init();
    }
}
```

## How It Works Now

### When Journal Logging is Enabled

1. Netdata sets `NETDATA_SYSTEMD_JOURNAL_PATH` environment variable
2. Netdata's `nd_log_collectors_fd()` returns `journal_direct.fd`
3. Plugin's stderr (fd 2) is dup2'd to `journal_direct.fd`
4. Plugin detects env var and uses `tracing-journald` layer
5. `tracing-journald` formats logs as structured journal messages:
   ```
   MESSAGE=Processing journal function call
   PRIORITY=6
   CODE_FILE=main.rs
   CODE_LINE=215
   SYSLOG_IDENTIFIER=log-viewer-plugin
   ```
6. These messages go through the Unix socket to systemd-journald
7. Logs appear in `journalctl -u netdata`

### When Journal Logging is Disabled

1. `NETDATA_SYSTEMD_JOURNAL_PATH` not set
2. Plugin uses regular stderr with formatted text output
3. Logs go to terminal or wherever stderr is directed

## Environment Variables

- `NETDATA_SYSTEMD_JOURNAL_PATH`: Set by Netdata when journal logging is configured
  - Value: Path to systemd journal socket (e.g., `/run/systemd/journal/socket`)
  - Presence indicates journal logging is enabled

- `RUST_LOG`: Controls log verbosity
  - Example: `RUST_LOG=debug` for debug logs
  - Example: `RUST_LOG=log_viewer_plugin=trace` for trace logs from plugin only

## Verification

### Check if logs appear in journal:
```bash
# Start Netdata
sudo systemctl start netdata

# Watch logs
journalctl -u netdata -f | grep log-viewer-plugin
```

### Check environment variable:
```bash
# When Netdata spawns the plugin, this should be set:
echo $NETDATA_SYSTEMD_JOURNAL_PATH
```

## Dependencies

- `tracing-journald = "0.3"` - Added to workspace and plugin dependencies
- Formats tracing events as systemd journal messages
- Connects to journald automatically

## Module Structure

The tracing configuration has been refactored into a separate module for better maintainability:

### `src/tracing_config.rs`

Provides a clean API for configuring tracing:

```rust
use tracing_config::{TracingConfig, initialize_tracing};

// Use defaults
initialize_tracing(TracingConfig::default());

// Or customize
let config = TracingConfig::new("my-service")
    .with_otlp_endpoint("http://localhost:4318")
    .with_log_level("debug")
    .with_otel(false)           // Disable OpenTelemetry
    .with_force_stderr(true);   // Force stderr even if journal is detected

initialize_tracing(config);
```

**Features:**
- Builder pattern for configuration
- Automatic journal detection via `NETDATA_SYSTEMD_JOURNAL_PATH`
- Configurable OpenTelemetry tracing
- Force stderr mode for testing
- Comprehensive unit tests

## Benefits

1. **Automatic detection**: Plugin adapts based on Netdata's configuration
2. **Proper formatting**: Journal logs are structured with metadata
3. **No configuration needed**: Works out of the box when Netdata enables journal logging
4. **Backward compatible**: Still works with stderr when journal isn't configured
5. **Rich metadata**: Journal logs include file, line, function, priority, etc.

## Troubleshooting

### Logs not appearing in journal

1. Check if Netdata has journal logging enabled:
   ```bash
   grep -i journal /etc/netdata/netdata.conf
   ```

2. Check if environment variable is set:
   ```bash
   ps auxe | grep log-viewer-plugin | grep NETDATA_SYSTEMD_JOURNAL_PATH
   ```

3. Check if journald is running:
   ```bash
   systemctl status systemd-journald
   ```

### Logs appearing on terminal instead of journal

This is expected when running `netdata -D` (debug mode). The `-D` flag may override journal configuration to show logs on terminal for debugging purposes.

To test journal logging properly, run Netdata as a systemd service:
```bash
sudo systemctl start netdata
journalctl -u netdata -f
```
