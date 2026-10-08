#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Regenerates tests/vectors/ksfbar.txt of netdata-agent-query from the C implementation (gen-ks-vectors.c, which
# includes the production KolmogorovSmirnovDist.c unchanged), with the flags the C agent is built with. Run it on
# the machine class the tests run on: the vector's header names the glibc and the CPU class, and the test compares
# bit for bit only where both match.
#
# Usage:
#   NETDATA_SRC=/path/to/netdata [NETDATA_BUILD=/path/to/netdata/build] tests/oracle/gen-ks-vectors.sh

set -euo pipefail

# shellcheck source=../../../c-oracle/lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/../../../c-oracle/lib.sh"

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
CRATE_DIR=$(cd -- "${SCRIPT_DIR}/../.." && pwd)
[[ -f "${CRATE_DIR}/Cargo.toml" && -d "${CRATE_DIR}/tests/vectors" ]] || die "unexpected crate layout at ${CRATE_DIR}"

c_oracle_env

WORK=$(mktemp -d "${TMPDIR:-/tmp}/netdata-agent-query-oracle.XXXXXX")
trap 'rm -rf -- "${WORK}"' EXIT

# the distribution's file needs libm alone
run cc "${CFLAGS[@]}" "${SCRIPT_DIR}/gen-ks-vectors.c" -o "${WORK}/gen-ks-vectors" -lm
run "${WORK}/gen-ks-vectors" "${CRATE_DIR}/tests/vectors/ksfbar.txt"
printf >&2 "vectors written to %s\n" "${CRATE_DIR}/tests/vectors/ksfbar.txt"
