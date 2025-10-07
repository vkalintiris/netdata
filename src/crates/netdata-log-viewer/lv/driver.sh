#!/usr/bin/env bash

set -eu -o pipefail

echo "Adding /var/log/journal to list of watched directories"
curl -X POST "http://localhost:8080/watch_directory?path=/var/log/journal"
echo ""

YESTERDAY=$(date -d "1 year ago" +%s)
NOW=$(date -d "now" +%s)

sleep 2
curl "http://localhost:8080/find_files?start=${YESTERDAY}&end=${NOW}"
echo ""
