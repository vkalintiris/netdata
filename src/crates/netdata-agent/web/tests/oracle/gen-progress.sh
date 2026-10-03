#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Regenerates tests/vectors/progress_order.txt of netdata-agent-web from the C implementation (gen-progress.c).
#
# Usage:
#   NETDATA_SRC=/path/to/netdata [NETDATA_BUILD=/path/to/netdata/build] tests/oracle/gen-progress.sh

set -euo pipefail

# shellcheck source=../../../c-oracle/lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/../../../c-oracle/lib.sh"

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
CRATE_DIR=$(cd -- "${SCRIPT_DIR}/../.." && pwd)
[[ -f "${CRATE_DIR}/Cargo.toml" && -d "${CRATE_DIR}/tests/vectors" ]] || die "unexpected crate layout at ${CRATE_DIR}"

c_oracle_env

WORK=$(mktemp -d "${TMPDIR:-/tmp}/netdata-agent-web-oracle.XXXXXX")
trap 'rm -rf -- "${WORK}"' EXIT

run cc "${CFLAGS[@]}" "${SCRIPT_DIR}/gen-progress.c" -o "${WORK}/gen-progress" "${LIBS[@]}"
run "${WORK}/gen-progress" "${CRATE_DIR}/tests/vectors/progress_order.txt"
printf >&2 "vectors written to %s\n" "${CRATE_DIR}/tests/vectors/progress_order.txt"
