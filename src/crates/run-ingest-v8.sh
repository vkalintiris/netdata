#!/usr/bin/env bash
set -uo pipefail
cd "$(dirname "$0")"

OUT="$HOME/repos/tmp/ng/flat-v8-$(date +%s)"   # fresh dir, never clobbers
mkdir -p "$OUT"

cargo build --profile profiling -p ng-ingest -p ng-index
cargo build --profile profiling -p otel-streams --bins

target/profiling/ng-ingest --listen 127.0.0.1:4317 --out "$OUT" --count 500000 \
  >"$OUT/ng-ingest.log" 2>&1 &
INGEST=$!

# wait for the listener, then start the producers (batch 1000, flush 1s)
for _ in $(seq 1 30); do (echo >/dev/tcp/127.0.0.1/4317) 2>/dev/null && break; sleep 0.5; done

target/profiling/jetstream  --otel-endpoint http://127.0.0.1:4317 --batch-size 1000 --flush-interval-ms 1000 >"$OUT/jetstream.log"  2>&1 &
JET=$!
target/profiling/certstream --otel-endpoint http://127.0.0.1:4317 --batch-size 1000 --flush-interval-ms 1000 >"$OUT/certstream.log" 2>&1 &
CERT=$!

wait "$INGEST"                 # ng-ingest stops itself at 500K and flushes
kill "$JET" "$CERT" 2>/dev/null # stop only the producers we started

echo "OUT=$OUT"
ls -lh "$OUT"/*.wal
tail -n 3 "$OUT/ng-ingest.log"
