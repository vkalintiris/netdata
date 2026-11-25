#!/usr/bin/env bash
# Start (or restart) the lab's LiteLLM proxy and its database: proxy on 127.0.0.1:14000, Postgres on 127.0.0.1:15432.
# Telemetry: Prometheus on the proxy's /metrics (scraped by go.d), OTLP traces and logs to the dev Agent.
# Secrets: LITELLM_API_KEY from the caller's environment (the upstream key), the rest from
# <worktree>/.local/gateways/litellm.env (local throwaway values).
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(git -C "$here" rev-parse --show-toplevel)
env_file="$root/.local/gateways/litellm.env"
: "${LITELLM_API_KEY:?LITELLM_API_KEY is empty}"
# shellcheck disable=SC1090
source "$env_file"
otlp=${LITELLM_OTLP_ENDPOINT:-http://172.17.0.1:44317}
image=${LITELLM_IMAGE:-ghcr.io/berriai/litellm:v1.110.0}

docker rm -f sjrg-litellm sjrg-litellm-db >/dev/null 2>&1 || true

docker run -d --name sjrg-litellm-db -p 127.0.0.1:15432:5432 \
  -e POSTGRES_USER=litellm -e POSTGRES_DB=litellm -e POSTGRES_PASSWORD="$POSTGRES_PASSWORD" \
  -v sjrg-litellm-db:/var/lib/postgresql postgres:18.6 >/dev/null
until docker exec sjrg-litellm-db pg_isready -U litellm -q; do sleep 1; done

docker run -d --name sjrg-litellm --network host \
  -v "$here/config.yaml:/app/config.yaml:ro" \
  -e LITELLM_API_KEY -e LITELLM_MASTER_KEY="$LITELLM_MASTER_KEY" \
  -e DATABASE_URL="postgresql://litellm:$POSTGRES_PASSWORD@127.0.0.1:15432/litellm" \
  -e STORE_MODEL_IN_DB=False \
  -e OTEL_EXPORTER=otlp_grpc -e OTEL_ENDPOINT="$otlp" -e OTEL_SERVICE_NAME=litellm \
  -e LITELLM_OTEL_INTEGRATION_ENABLE_EVENTS=true \
  "$image" --config /app/config.yaml --host 127.0.0.1 --port 14000 >/dev/null

echo "waiting for the proxy"
until curl -sf http://127.0.0.1:14000/health/readiness >/dev/null; do sleep 2; done
echo "ready: http://127.0.0.1:14000"
