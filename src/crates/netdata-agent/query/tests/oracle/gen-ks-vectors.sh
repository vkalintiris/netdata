#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Regenerates tests/vectors/ksfbar.txt and tests/vectors/ks2samp.txt of netdata-agent-query from the C
# implementation (gen-ks-vectors.c, which includes the production KolmogorovSmirnovDist.c unchanged and the
# two-sample statistic's text cut from weights.c), with the flags the C agent is built with. Run it on the machine
# class the tests run on: a vector's header names the glibc and the CPU class, and the tests compare bit for bit
# only where both match.
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

# The two-sample statistic's text, by its boundaries: from the type of the differences to the line before kstwo().
# weights.c itself needs the whole agent.
WEIGHTS="${SRC}/src/web/api/queries/weights.c"
first=$(grep -n '^typedef long int DIFFS_NUMBERS;$' "${WEIGHTS}" | cut -d: -f1)
after=$(grep -n '^static double kstwo($' "${WEIGHTS}" | cut -d: -f1)
[[ "${first}" =~ ^[0-9]+$ && "${after}" =~ ^[0-9]+$ && "${first}" -lt "${after}" ]] \
    || die "the two-sample statistic's boundaries are not where they were in ${WEIGHTS}"
sed -n "${first},$((after - 1))p" "${WEIGHTS}" > "${WORK}/weights-ks2.inc"
grep -q '^static double ks_2samp($' "${WORK}/weights-ks2.inc" || die "ks_2samp() is not in the text cut from ${WEIGHTS}"

# the two files need libm alone
run cc "${CFLAGS[@]}" -I"${WORK}" "${SCRIPT_DIR}/gen-ks-vectors.c" -o "${WORK}/gen-ks-vectors" -lm
run "${WORK}/gen-ks-vectors" "${CRATE_DIR}/tests/vectors/ksfbar.txt" "${CRATE_DIR}/tests/vectors/ks2samp.txt"
printf >&2 "vectors written to %s\n" "${CRATE_DIR}/tests/vectors"
