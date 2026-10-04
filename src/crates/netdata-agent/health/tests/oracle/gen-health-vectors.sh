#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Regenerates the golden vectors of netdata-agent-health's configuration path from the C implementation:
#   tests/vectors/rules.tsv, records.tsv   from gen-health-vectors.c, over the stock health.d of this tree and the
#                                          files under tests/corpus/
#   tests/vectors/c_unittest.tsv           from C's own unit test (health-config-unittest.c), which must pass, with
#                                          health-unittest-dump.inc and health-unittest-main.inc spliced into a copy
#
# Usage:
#   NETDATA_SRC=/path/to/netdata [NETDATA_BUILD=/path/to/netdata/build] tests/oracle/gen-health-vectors.sh
#
# Health's objects are in no static library, so the configuration path is compiled from NETDATA_SRC with the flags
# of the shared C-oracle helper (gcc, -O2, LTO) and linked with health-oracle-stubs.c and NETDATA_BUILD's
# liblibnetdata.a. It never runs a netdata binary.

set -euo pipefail

# shellcheck source=../../../c-oracle/lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/../../../c-oracle/lib.sh"

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
CRATE_DIR=$(cd -- "${SCRIPT_DIR}/../.." && pwd)
[[ -f "${CRATE_DIR}/Cargo.toml" && -d "${CRATE_DIR}/tests/vectors" && -d "${CRATE_DIR}/tests/corpus" ]] \
    || die "unexpected crate layout at ${CRATE_DIR}"
STOCK_DIR="../../../health/health.d"
[[ -d "${CRATE_DIR}/${STOCK_DIR}" ]] || die "no stock health.d at ${CRATE_DIR}/${STOCK_DIR}"

c_oracle_env

WORK=$(mktemp -d "${TMPDIR:-/tmp}/netdata-agent-health-oracle.XXXXXX")
trap 'rm -rf -- "${WORK}"' EXIT

# the configuration path: the reader, the prototype store and the hash, with what they call
SOURCES=(
    health/health_config.c health/health_prototypes.c health/health_dyncfg.c
    web/api/queries/query-group-over-time.c web/api/maps/rrdr_options.c database/pattern-array.c
    database/rrdlabels.c database/rrdlabels-aggregated.c
)
OBJECTS=()
for source in "${SOURCES[@]}"; do
    [[ -f "${SRC}/src/${source}" ]] || die "missing ${SRC}/src/${source}"
    object="${WORK}/$(basename -- "${source}" .c).o"
    run cc "${CFLAGS[@]}" -c "${SRC}/src/${source}" -o "${object}"
    OBJECTS+=("${object}")
done
run cc "${CFLAGS[@]}" -c "${SCRIPT_DIR}/health-oracle-stubs.c" -o "${WORK}/health-oracle-stubs.o"
OBJECTS+=("${WORK}/health-oracle-stubs.o")

run cc "${CFLAGS[@]}" "${SCRIPT_DIR}/gen-health-vectors.c" "${OBJECTS[@]}" -o "${WORK}/gen-health-vectors" "${LIBS[@]}"
run cc "${CFLAGS[@]}" "${SCRIPT_DIR}/gen-link-vectors.c" "${OBJECTS[@]}" -o "${WORK}/gen-link-vectors" "${LIBS[@]}"

# the items, with paths relative to the crate's directory: the stock files, then each family of the corpus. A
# directory named *.group is one item whose files are read one after the other into the same store.
(
    cd -- "${CRATE_DIR}"
    export LC_ALL=C
    for file in "${STOCK_DIR}"/*.conf; do
        printf '1 %s\n' "${file}"
    done
    for family in tests/corpus/*/; do
        for entry in "${family}"*; do
            if [[ -d "${entry}" && "${entry}" == *.group ]]; then
                printf 'group %s\n' "${entry}"
                for file in "${entry}"/*.conf; do
                    printf '+0 %s\n' "${file}"
                done
            # whatever is named *.conf: a dangling link and a directory are paths the reader cannot read
            elif [[ "${entry}" == *.conf ]]; then
                printf '0 %s\n' "${entry}"
            fi
        done
    done
) >"${WORK}/items.list"

(cd -- "${CRATE_DIR}" && run "${WORK}/gen-health-vectors" tests/vectors "${WORK}/items.list" 2>"${WORK}/gen.log") \
    || die "the generator failed: $(tail -n 5 "${WORK}/gen.log")"

# the matching of rules against hosts and charts, as the scenario file of the corpus sets them up
SCENARIOS=tests/corpus/match/scenarios.txt
[[ -f "${CRATE_DIR}/${SCENARIOS}" ]] || die "missing ${CRATE_DIR}/${SCENARIOS}"
(cd -- "${CRATE_DIR}" && run "${WORK}/gen-link-vectors" tests/vectors "${SCENARIOS}" 2>"${WORK}/link.log") \
    || die "the link generator failed: $(tail -n 5 "${WORK}/link.log")"

# C's unit vectors, dumped while C's own test runs: the dump's wrappers go in right after the test's includes
UNITTEST="${SRC}/src/health/health-config-unittest.c"
[[ -f "${UNITTEST}" ]] || die "missing ${UNITTEST}"
awk -v inc="${SCRIPT_DIR}/health-unittest-dump.inc" '
    { print }
    /^#include "web\/api\/queries\/query.h"$/ && !found {
        while ((getline line < inc) > 0) print line
        found = 1
    }
    END { if (!found) exit 1 }
' "${UNITTEST}" >"${WORK}/health-config-unittest.c" || die "the includes of ${UNITTEST} changed"
cat "${SCRIPT_DIR}/health-unittest-main.inc" >>"${WORK}/health-config-unittest.c"

run cc "${CFLAGS[@]}" -iquote "${SRC}/src/health" -iquote "${SCRIPT_DIR}" "${WORK}/health-config-unittest.c" \
    "${OBJECTS[@]}" -o "${WORK}/health-unittest" "${LIBS[@]}"
(cd -- "${WORK}" && run "${WORK}/health-unittest" "${CRATE_DIR}/tests/vectors" >"${WORK}/unittest.out" 2>"${WORK}/unittest.log") \
    || die "the C unit test failed: $(tail -n 5 "${WORK}/unittest.log")"

# C's own count, which the Rust tests assert again on the dumped rows
grep -E '^Health config parser tests: ' "${WORK}/unittest.log" >&2

printf >&2 "%s items; vectors written to %s\n" "$(grep -vc '^+' "${WORK}/items.list")" "${CRATE_DIR}/tests/vectors"
