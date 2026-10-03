#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Regenerates the golden vectors of netdata-agent-eval from the C implementation:
#   - tests/vectors/{bindings,corpus,hardcode}.tsv        from gen-eval-vectors.c
#   - tests/vectors/c_unittest{,_hardcode}.tsv            from C's own unit tests (eval-unittest.c and
#     eval-unittest-hardcoding.c), which must pass, with eval-unittest-dump.inc spliced into copies of them
#
# Usage:
#   NETDATA_SRC=/path/to/netdata [NETDATA_BUILD=/path/to/netdata/build] tests/oracle/gen-eval-vectors.sh
#
# NETDATA_BUILD must hold a CMake build of NETDATA_SRC (liblibnetdata.a, config.h). The programs are compiled with
# the flags of that build (gcc, -O2, LTO) so they run the production code paths.

set -euo pipefail

# shellcheck source=../../../c-oracle/lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/../../../c-oracle/lib.sh"

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
CRATE_DIR=$(cd -- "${SCRIPT_DIR}/../.." && pwd)
[[ -f "${CRATE_DIR}/Cargo.toml" && -d "${CRATE_DIR}/tests/vectors" ]] || die "unexpected crate layout at ${CRATE_DIR}"

c_oracle_env

WORK=$(mktemp -d "${TMPDIR:-/tmp}/netdata-agent-eval-oracle.XXXXXX")
trap 'rm -rf -- "${WORK}"' EXIT

# the parse failures of the corpus are logged by C to stderr; they are not vectors
run cc "${CFLAGS[@]}" "${SCRIPT_DIR}/gen-eval-vectors.c" -o "${WORK}/gen-eval-vectors" "${LIBS[@]}"
run "${WORK}/gen-eval-vectors" "${CRATE_DIR}/tests/vectors" 2>"${WORK}/gen.log" \
    || die "the generator failed: $(tail -n 5 "${WORK}/gen.log")"

# C's unit vectors, dumped while C's own tests run
EVAL_DIR="${SRC}/src/libnetdata/eval"
[[ -f "${EVAL_DIR}/eval-unittest.c" && -f "${EVAL_DIR}/eval-unittest-hardcoding.c" ]] || die "missing C unit tests in ${EVAL_DIR}"
cat "${EVAL_DIR}/eval-unittest.c" "${SCRIPT_DIR}/eval-unittest-dump.inc" >"${WORK}/eval-unittest.c"
awk '
    /^int eval_hardcode_unittest\(void\) \{$/ {
        print "void dump_hardcode_case(size_t index, const char *name, const char *expression, const char *variable,"
        print "                        NETDATA_DOUBLE value, const char *expected_source, NETDATA_DOUBLE expected_result,"
        print "                        int expected_error);"
        print
        declared = 1
        next
    }
    /^        HardcodeTestCase \*tc = &test_cases\[i\];$/ {
        print
        print "        dump_hardcode_case(i, tc->name, tc->expression, tc->variable, tc->hardcode_value,"
        print "                           tc->expected_source, tc->expected_result, tc->expected_error);"
        called = 1
        next
    }
    { print }
    END { if (!declared || !called) exit 1 }
' "${EVAL_DIR}/eval-unittest-hardcoding.c" >"${WORK}/eval-unittest-hardcoding.c" \
    || die "eval_hardcode_unittest() or its loop not found in ${EVAL_DIR}/eval-unittest-hardcoding.c"

run cc "${CFLAGS[@]}" -iquote "${EVAL_DIR}" "${WORK}/eval-unittest.c" "${WORK}/eval-unittest-hardcoding.c" \
    -o "${WORK}/eval-unittest" "${LIBS[@]}"
run "${WORK}/eval-unittest" "${CRATE_DIR}/tests/vectors" >"${WORK}/unittest.out" 2>"${WORK}/unittest.log" \
    || die "the C unit tests failed: $(tail -n 5 "${WORK}/unittest.out")"

# C's own counts, which the Rust tests assert again on the dumped rows
grep -E '^(Total tests|Failed): |^Hardcode variable test results: ' "${WORK}/unittest.out" >&2

printf >&2 "vectors written to %s\n" "${CRATE_DIR}/tests/vectors"
