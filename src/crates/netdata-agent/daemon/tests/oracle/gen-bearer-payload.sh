#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Regenerates tests/vectors/bearer_payload.txt of netdata-agent-daemon from the C implementation (gen-bearer-payload.c).
#
# Usage:
#   NETDATA_SRC=/path/to/netdata [NETDATA_BUILD=/path/to/netdata/build] tests/oracle/gen-bearer-payload.sh

set -euo pipefail

# shellcheck source=../../../c-oracle/lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/../../../c-oracle/lib.sh"

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
CRATE_DIR=$(cd -- "${SCRIPT_DIR}/../.." && pwd)
[[ -f "${CRATE_DIR}/Cargo.toml" && -d "${CRATE_DIR}/tests/vectors" ]] || die "unexpected crate layout at ${CRATE_DIR}"

c_oracle_env

WORK=$(mktemp -d "${TMPDIR:-/tmp}/netdata-agent-daemon-oracle.XXXXXX")
trap 'rm -rf -- "${WORK}"' EXIT

run cc "${CFLAGS[@]}" "${SCRIPT_DIR}/gen-bearer-payload.c" -o "${WORK}/gen-bearer-payload" "${LIBS[@]}"
run "${WORK}/gen-bearer-payload" "${CRATE_DIR}/tests/vectors/bearer_payload.txt"
printf >&2 "vectors written to %s\n" "${CRATE_DIR}/tests/vectors/bearer_payload.txt"
