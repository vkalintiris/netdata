# Netdata Crates Workspace

This directory contains a Cargo workspace with multiple crates for Netdata's Rust-based components.

## Workspace Structure

### Core Libraries

- **journal** - Low-level systemd journal file parser and indexer
- **journalctl** - Binary utility for journal operations
- **journal-sql** - SQL query interface for journal logs using Apache DataFusion

### Netdata Plugin Framework

Located in `netdata-plugin/`:

- **error** - Error types for Netdata plugins
- **protocol** - Communication protocol with Netdata agent
- **rt** - Runtime and chart management
- **schema** - Schema definitions
- **types** - Common types
- **bridge** - Bridge utilities
- **charts-derive** - Procedural macros for chart definitions

### Netdata Log Viewer

Located in `netdata-log-viewer/`:

- **journal-function** - Core journal query and histogram functionality
- **log-viewer-plugin** - Netdata plugin for journal log viewing

### Netdata OTEL (OpenTelemetry)

Located in `netdata-otel/`:

- **otel-plugin** - OpenTelemetry plugin for Netdata
- **flatten_otel** - OTEL data flattening utilities
- **flog-otel** - OTEL log generation

## Building

Build the entire workspace:

```bash
cargo build
```

Build a specific crate:

```bash
cargo build -p journal
cargo build -p log-viewer-plugin
cargo build -p otel-plugin
cargo build -p journal-sql
```

Run a binary crate:

```bash
cargo run -p journalctl -- --help
cargo run -p journal-sql -- --help
```

## Testing

Run all tests:

```bash
cargo test --workspace
```

Run tests for specific crate:

```bash
cargo test -p journal
```

## Build Profiles

- **dev** - Development profile with optimization level 1
- **release** - Release profile with full optimization, LTO, and debug info
- **release-min** - Minimal release profile optimized for size

Build with release profile:

```bash
cargo build --release
```

Build with minimal size optimization:

```bash
cargo build --profile release-min
```

## Workspace Dependencies

All shared dependencies are managed at the workspace level in the root `Cargo.toml`. Individual crates can reference workspace dependencies using:

```toml
[dependencies]
anyhow = { workspace = true }
tokio = { workspace = true }
```

## Adding New Crates

To add a new crate to the workspace:

1. Create the crate directory under the appropriate location
2. Add it to the `members` list in the root `Cargo.toml`
3. Add any new crate-specific dependencies to `[workspace.dependencies]`
