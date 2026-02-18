#!/usr/bin/env bash
#
# Build and run the index example with treight bitmaps + allocative.
# Run with --help for all options.
#
set -eu -o pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE="${SCRIPT_DIR}/../.."
BINARY="${WORKSPACE}/target/release/examples/index"

FEATURES="bitmap-treight,allocative"

echo "Building index example (features: ${FEATURES})..."
RUSTFLAGS="-A warnings" cargo build --release -p journal-engine --example index \
    --no-default-features --features "$FEATURES" \
    --manifest-path "${WORKSPACE}/Cargo.toml"

exec "$BINARY" "$@"
