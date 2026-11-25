#!/usr/bin/env bash
# Start (or restart) the lab's agentgateway: LLM routes on 127.0.0.1:3100, Prometheus on 127.0.0.1:15120/metrics
# (scraped by go.d), admin API on 127.0.0.1:15100, OTLP traces and access logs to the dev Agent.
# Secrets: LITELLM_API_KEY from the caller's environment (the upstream key); the teams' keys are local throwaway
# values generated once into <worktree>/.local/gateways/agentgateway.env and agentgateway-keys.json (for the game).
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(git -C "$here" rev-parse --show-toplevel)
local_dir="$root/.local/gateways"
env_file="$local_dir/agentgateway.env"
image=${AGW_IMAGE:-cr.agentgateway.dev/agentgateway:v1.7.0}
: "${LITELLM_API_KEY:?LITELLM_API_KEY is empty}"

if [ ! -f "$env_file" ]; then
  mkdir -p "$local_dir"
  (umask 077
   for team in SEARCH SUPPORT GROWTH; do printf 'AGW_KEY_%s=agw-%s\n' "$team" "$(openssl rand -hex 16)"; done > "$env_file")
fi
# shellcheck disable=SC1090
set -a; source "$env_file"; set +a
(umask 077
 printf '{"search": "%s", "support": "%s", "growth": "%s"}\n' "$AGW_KEY_SEARCH" "$AGW_KEY_SUPPORT" "$AGW_KEY_GROWTH" \
   > "$local_dir/agentgateway-keys.json")

docker rm -f sjrg-agentgateway >/dev/null 2>&1 || true
docker run -d --name sjrg-agentgateway --network host \
  -v "$here/config.yaml:/config.yaml:ro" \
  -e LITELLM_API_KEY -e AGW_KEY_SEARCH -e AGW_KEY_SUPPORT -e AGW_KEY_GROWTH \
  -e OTEL_SERVICE_NAME=agentgateway \
  "$image" -f /config.yaml >/dev/null

echo "waiting for agentgateway"
for _ in $(seq 1 60); do curl -fs -o /dev/null http://127.0.0.1:15121/healthz/ready && break; sleep 1; done
curl -fs -o /dev/null http://127.0.0.1:15121/healthz/ready || { docker logs --tail 30 sjrg-agentgateway; exit 1; }
echo "ready: http://127.0.0.1:3100"
