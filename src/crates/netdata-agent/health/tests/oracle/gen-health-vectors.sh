#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Regenerates the golden vectors of netdata-agent-health from the C implementation:
#   tests/vectors/rules.tsv, records.tsv   from gen-health-vectors.c, over the stock health.d of this tree and the
#                                          files under tests/corpus/
#   tests/vectors/labels.tsv, link.tsv     from gen-link-vectors.c, over tests/corpus/match/scenarios.txt
#   tests/vectors/delay.tsv, units.tsv,    from gen-loop-vectors.c: its tables, and C's own per-host pass run over
#   edit.tsv, sanitize.tsv,                each scenario under tests/corpus/loop/; with the metadata queue and its
#   aclk-delay.tsv, loop.tsv, queue.tsv,   store job in play, under tests/corpus/queue/; and with C's own
#   sql.tsv                                sqlite_health.c over a real database file, under tests/corpus/sql/
#   tests/vectors/silencers-file.tsv,      from gen-loop-vectors.c: C's own health_silencers.c over file texts, request
#   manage.tsv, silencers-match.tsv,       sequences and alerts, a process per case; and the pass with it, under
#   silencers.tsv                          tests/corpus/silencers/
#   tests/vectors/badge-*.tsv              from gen-loop-vectors.c: six tables of C's own web_buffer_svg.c (the value
#                                          text with a precision, widths, the XML escape, color expressions, color
#                                          arguments, the SVG), its static functions reached by badge-splice.inc
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
LOOP_OBJECTS=("${OBJECTS[@]}")
run cc "${CFLAGS[@]}" -c "${SCRIPT_DIR}/health-oracle-stubs.c" -o "${WORK}/health-oracle-stubs.o"
OBJECTS+=("${WORK}/health-oracle-stubs.o")

run cc "${CFLAGS[@]}" "${SCRIPT_DIR}/gen-health-vectors.c" "${OBJECTS[@]}" -o "${WORK}/gen-health-vectors" "${LIBS[@]}"
run cc "${CFLAGS[@]}" "${SCRIPT_DIR}/gen-link-vectors.c" "${OBJECTS[@]}" -o "${WORK}/gen-link-vectors" "${LIBS[@]}"

# the evaluation loop: C's alert instances, alert log, variable lookup, silencers and per-host pass, with what the
# daemon gives them stubbed (health-loop-stubs.c)
# and the alert log's SQL: C's sqlite_health.c and sqlite_functions.c with the build's SQLite, for the scenarios
# that run over a real database file (health-sql-stubs.c)
LOOP_SOURCES=(
    health/health_log.c health/rrdcalc.c health/health_variable.c health/rrdvar.c health/health_silencers.c
    web/api/v1/api_v1_badge/web_buffer_svg.c
    database/sqlite/sqlite_health.c database/sqlite/sqlite_functions.c
    database/contexts/api_v2_contexts_alert_config.c
)
[[ -f "${BUILD}/libsqlite3.a" ]] || die "no ${BUILD}/libsqlite3.a"
for source in "${LOOP_SOURCES[@]}"; do
    [[ -f "${SRC}/src/${source}" ]] || die "missing ${SRC}/src/${source}"
    object="${WORK}/$(basename -- "${source}" .c).o"
    defines=()
    # an entry's transition id is a random UUID: health_log.c gets the stubs' counted one instead; and the kill of
    # a freed entry's running notification is the stubs'
    [[ "${source}" == health/health_log.c ]] && defines=(
        -Dos_uuid_generate_random=oracle_uuid_generate_random -Dspawn_popen_kill=oracle_spawn_popen_kill
    )
    # the stubs of these five record the call and, with a real database, call C's own under its new name; the
    # transition id of a REMOVED row injected at a restart is counted out too
    [[ "${source}" == database/sqlite/sqlite_health.c ]] && defines=(
        -Dos_uuid_generate_random=oracle_uuid_generate_random
        -Dsql_health_alarm_log_save=c_sql_health_alarm_log_save
        -Dsql_health_alarm_log_load=c_sql_health_alarm_log_load
        -Dsql_get_alarm_id=c_sql_get_alarm_id -Dsql_alert_store_config=c_sql_alert_store_config
        -Dsql_health_get_last_executed_event=c_sql_health_get_last_executed_event
    )
    compile="${SRC}/src/${source}"
    # the badge's helpers are static: its file is compiled as a copy with the splice appended (its own includes are
    # relative to its directory)
    if [[ "${source}" == web/api/v1/api_v1_badge/web_buffer_svg.c ]]; then
        compile="${WORK}/web_buffer_svg.c"
        cat -- "${SRC}/src/${source}" "${SCRIPT_DIR}/badge-splice.inc" >"${compile}"
        defines=(-iquote "${SRC}/src/web/api/v1/api_v1_badge")
    fi
    run cc "${CFLAGS[@]}" "${defines[@]}" -c "${compile}" -o "${object}"
    LOOP_OBJECTS+=("${object}")
done

# C's schema of the metadata database: the array database_config[] of sqlite_metadata.c, as it stands there
METADATA="${SRC}/src/database/sqlite/sqlite_metadata.c"
[[ -f "${METADATA}" ]] || die "missing ${METADATA}"
awk '
    BEGIN { print "#include <stddef.h>" }
    /^const char \*database_config\[\] = \{$/ { inside = 1 }
    inside { print }
    inside && /^\};$/ { done = 1; exit }
    END { if (!done) exit 1 }
' "${METADATA}" >"${WORK}/meta-schema.c" || die "the schema of ${METADATA} moved"
run cc "${CFLAGS[@]}" -c "${WORK}/meta-schema.c" -o "${WORK}/meta-schema.o"
run cc "${CFLAGS[@]}" -c "${SCRIPT_DIR}/health-sql-stubs.c" -o "${WORK}/health-sql-stubs.o"
LOOP_OBJECTS+=("${WORK}/meta-schema.o" "${WORK}/health-sql-stubs.o")

# the pass is static: its file is compiled as a copy with the splice appended. C's notification code is compiled as
# it stands, over the stubs' spawn, wait, kill and pid of a command, and their monotonic clock
NOTIFICATIONS="${SRC}/src/health/health_notifications.c"
EVENT_LOOP="${SRC}/src/health/health_event_loop.c"
[[ -f "${NOTIFICATIONS}" && -f "${EVENT_LOOP}" ]] || die "missing ${NOTIFICATIONS} or ${EVENT_LOOP}"
cat -- "${EVENT_LOOP}" "${SCRIPT_DIR}/health-loop-splice.inc" >"${WORK}/health_event_loop.c"

run cc "${CFLAGS[@]}" -Dspawn_popen_run=oracle_spawn_popen_run -Dspawn_popen_timedwait=oracle_spawn_popen_timedwait \
    -Dspawn_popen_kill=oracle_spawn_popen_kill -Dspawn_popen_pid=oracle_spawn_popen_pid \
    -Dnow_monotonic_usec=oracle_now_monotonic_usec -c "${NOTIFICATIONS}" -o "${WORK}/health_notifications.o"
run cc "${CFLAGS[@]}" -iquote "${SRC}/src/health" -c "${WORK}/health_event_loop.c" -o "${WORK}/health_event_loop.o"
run cc "${CFLAGS[@]}" -DHEALTH_ORACLE_LOOP -c "${SCRIPT_DIR}/health-oracle-stubs.c" -o "${WORK}/health-oracle-stubs-loop.o"
run cc "${CFLAGS[@]}" -c "${SCRIPT_DIR}/health-loop-stubs.c" -o "${WORK}/health-loop-stubs.o"
LOOP_OBJECTS+=(
    "${WORK}/health_notifications.o" "${WORK}/health_event_loop.o" "${WORK}/health-oracle-stubs-loop.o"
    "${WORK}/health-loop-stubs.o"
)
run cc "${CFLAGS[@]}" "${SCRIPT_DIR}/gen-loop-vectors.c" "${LOOP_OBJECTS[@]}" -o "${WORK}/gen-loop-vectors" \
    "${BUILD}/libsqlite3.a" "${LIBS[@]}"

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

# the loop: the tables, then one process per scenario, each appending its rows to its family's file
for family in loop queue sql notify silencers dyncfg child reload badge; do
    [[ -d "${CRATE_DIR}/tests/corpus/${family}" ]] || die "missing ${CRATE_DIR}/tests/corpus/${family}"
done
(cd -- "${CRATE_DIR}" && run "${WORK}/gen-loop-vectors" tables tests/vectors >"${WORK}/loop.log" 2>&1) \
    || die "the loop generator's tables failed: $(tail -n 5 "${WORK}/loop.log")"
(cd -- "${CRATE_DIR}" && LC_ALL=C TZ=UTC run "${WORK}/gen-loop-vectors" decide tests/vectors "${WORK}/decide.records" \
    >"${WORK}/loop.log" 2>&1) || die "the loop generator's decision table failed: $(tail -n 5 "${WORK}/loop.log")"
(cd -- "${CRATE_DIR}" && LC_ALL=C TZ=UTC run "${WORK}/gen-loop-vectors" silencers tests/vectors \
    "${WORK}/silencers.records" >"${WORK}/loop.log" 2>&1) \
    || die "the loop generator's silencers tables failed: $(tail -n 5 "${WORK}/loop.log")"
(cd -- "${CRATE_DIR}" && LC_ALL=C TZ=UTC run "${WORK}/gen-loop-vectors" dyncfg tests/vectors \
    "${WORK}/dyncfg.records" >"${WORK}/loop.log" 2>&1) \
    || die "the loop generator's DynCfg tables failed: $(tail -n 5 "${WORK}/loop.log")"
(cd -- "${CRATE_DIR}" && LC_ALL=C TZ=UTC run "${WORK}/gen-loop-vectors" badge tests/vectors >"${WORK}/loop.log" 2>&1) \
    || die "the loop generator's badge tables failed: $(tail -n 5 "${WORK}/loop.log")"
(
    cd -- "${CRATE_DIR}"
    # C prints a record's notification time as a local date
    export LC_ALL=C TZ=UTC
    for family in loop queue sql notify silencers dyncfg child reload badge; do
        vectors="tests/vectors/${family}.tsv"
        {
            printf '# generated by tests/oracle/gen-loop-vectors.c from the C implementation; do not edit\n'
            printf '# columns: scenario, step, kind, then the fields of that kind (at the top of the program)\n'
        } >"${vectors}"
        for scenario in "tests/corpus/${family}"/*.scn; do
            # C's records go to the third argument; what the program itself has to say goes to its stdout
            run "${WORK}/gen-loop-vectors" scenario "${vectors}" "${scenario}" "${WORK}/loop.records" \
                >"${WORK}/loop.log" || die "the loop generator failed on ${scenario}: $(tail -n 5 "${WORK}/loop.log")"
        done
    done
)

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
