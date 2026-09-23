#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Regenerates tests/vectors/*.tsv of netdata-agent-storage from the C implementation.
# Usage: NETDATA_SRC=/path/to/netdata [NETDATA_BUILD=/path/to/netdata/build] tests/oracle/gen-vectors.sh

set -euo pipefail

# shellcheck source=../../../c-oracle/lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/../../../c-oracle/lib.sh"

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
VECTORS=$(cd -- "${SCRIPT_DIR}/../vectors" && pwd)

c_oracle_env

WORK=$(mktemp -d "${TMPDIR:-/tmp}/netdata-agent-storage-oracle.XXXXXX")
trap 'rm -rf -- "${WORK}"' EXIT

run cc "${CFLAGS[@]}" "${SCRIPT_DIR}/gen-vectors.c" -o "${WORK}/gen-vectors" "${LIBS[@]}"
run "${WORK}/gen-vectors" "${VECTORS}"
printf >&2 "vectors written to %s\n" "${VECTORS}"
