#!/usr/bin/env bash
# Add go.d.plugin and its stock config (Prometheus profiles included) to the dev Agent's install: the netdata-build
# MCP builds without go.d. Run again after the MCP reinstalls the Agent or after a change under src/go; then restart
# the Agent. Only the prometheus collector runs (the Agent's etc/go.d.conf), so go.d probes nothing else on the host.
#   install-god.sh [<install prefix>] [<agent config dir>]
set -euo pipefail

root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
prefix=${1:-$HOME/opt/netdata-builds/sjr-gai/netdata}
etc=${2:-$HOME/opt/netdata-mcp/run/sjrg/etc}
plugins=$prefix/usr/libexec/netdata/plugins.d
stock=$prefix/usr/lib/netdata/conf.d
src=$root/src/go/plugin/go.d/config

(cd "$root/src/go" && CGO_ENABLED=0 go build -buildvcs=false -o "$plugins/go.d.plugin" ./cmd/godplugin)

install -m 0644 "$src/go.d.conf" "$stock/go.d.conf"
mkdir -p "$stock/go.d"
install -m 0644 "$src"/go.d/*.conf "$stock/go.d/"
rm -rf "$stock/go.d/prometheus.profiles"
cp -r "$src/go.d/prometheus.profiles" "$stock/go.d/prometheus.profiles"

mkdir -p "$etc/go.d"
cat > "$etc/go.d.conf" <<'EOF'
# Lab: only the prometheus collector (src/genai-lab/gateways/install-god.sh).
enabled: yes
default_run: no
modules:
  prometheus: yes
EOF
echo "go.d.plugin installed under $plugins; restart the Agent"
