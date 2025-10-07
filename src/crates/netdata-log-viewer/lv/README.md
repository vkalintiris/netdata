# Histogram Backend (Rust)

A high-performance Rust implementation of the histogram backend service using Axum web framework.

## Features

- **Fast & Efficient**: Built with Rust and Axum for high performance
- **Multi-series Support**: Returns histogram data with multiple series per bucket (info, warning, error)
- **CORS Enabled**: Ready for cross-origin requests from the frontend
- **Realistic Data**: Generates data with daily patterns and live activity spikes

## Building and Running

### Prerequisites

- Rust toolchain (install from [rustup.rs](https://rustup.rs))

### Build

```bash
cd rust-backend
cargo build --release
```

### Run

```bash
cargo run --release
```

The server will start on `http://localhost:8080`

### Development Mode

For faster compilation during development (with debug symbols):

```bash
cargo run
```

## Endpoints

### GET /histogram

Returns histogram data for the specified time range.

**Query Parameters:**
- `after` (required): Unix timestamp in seconds - start of time range
- `before` (required): Unix timestamp in seconds - end of time range

**Example Request:**
```bash
curl "http://localhost:8080/histogram?after=1759837000&before=1759837900"
```

**Example Response:**
```json
{
  "buckets": [
    {
      "time": 1759837000,
      "data": {
        "info": 80.0,
        "warning": 30.0,
        "error": 10.0
      }
    },
    {
      "time": 1759837060,
      "data": {
        "info": 90.0,
        "warning": 25.0,
        "error": 12.0
      }
    }
  ]
}
```

### GET /health

Health check endpoint.

**Example Response:**
```json
{
  "status": "ok",
  "timestamp": "1759837400"
}
```

## Implementation Details

### Data Generation

The backend generates realistic histogram data with:
- **Info logs**: High baseline (80±30) with daily sine wave variation
- **Warning logs**: Medium baseline (30±20) with offset sine pattern
- **Error logs**: Low baseline (10±8) with different phase
- **Live spikes**: Recent data (last 5 minutes) has increased activity
- **Random noise**: Adds realistic variation to all series

### Bucket Sizing

- Returns 10-100 buckets based on time range
- Minimum bucket size: 1 minute
- Buckets are evenly distributed across the time range

## API Compatibility

This Rust backend implements the same API schema as the Python backend, ensuring full compatibility with the histogram visualization frontend. See the main README for detailed API schema documentation.

## Performance

Built with Rust, this backend offers:
- Low memory footprint
- Fast response times
- Efficient concurrent request handling
- Zero-cost abstractions with Axum

## Dependencies

- **axum**: Web framework
- **tokio**: Async runtime
- **serde**: Serialization/deserialization
- **tower-http**: CORS middleware
- **rand**: Random number generation
