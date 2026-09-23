#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Regenerates tests/expected/*.out of netdata-agent-inicfg by running every tests/scripts/*.script through the C
# inicfg (inicfg-oracle.c linked against a CMake build of the C agent).
#
# Usage:
#   NETDATA_SRC=/path/to/netdata [NETDATA_BUILD=/path/to/netdata/build] tests/oracle/gen-expected.sh

set -euo pipefail

# shellcheck source=../../../c-oracle/lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/../../../c-oracle/lib.sh"

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
TESTS_DIR=$(cd -- "${SCRIPT_DIR}/.." && pwd)
[[ -d "${TESTS_DIR}/scripts" && -d "${TESTS_DIR}/fixtures" ]] || die "unexpected crate layout at ${TESTS_DIR}"

c_oracle_env

WORK=$(mktemp -d "${TMPDIR:-/tmp}/netdata-agent-inicfg-oracle.XXXXXX")
trap 'rm -rf -- "${WORK}"' EXIT

run cc "${CFLAGS[@]}" "${SCRIPT_DIR}/inicfg-oracle.c" -o "${WORK}/inicfg-oracle" "${LIBS[@]}"
for script in "${TESTS_DIR}"/scripts/*.script; do
    name=$(basename -- "${script}" .script)
    # C logs go to stderr; only the step results are the contract here.
    run "${WORK}/inicfg-oracle" "${script}" "${TESTS_DIR}/fixtures" >"${TESTS_DIR}/expected/${name}.out" 2>/dev/null
done
printf >&2 "expected outputs written to %s\n" "${TESTS_DIR}/expected"
