#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Regenerates the dbengine codec vectors in tests/vectors/dbengine from the C implementation:
#   gorilla-writer.json, gorilla-page.json, gorilla-disk-load.json, raw-pages.json  (gen-gorilla.cc)
#   pgd-tests-run.json  (src/database/engine/page_test.cc compiled verbatim against fake-gtest/)
#
# Usage: NETDATA_SRC=/path/to/netdata [NETDATA_BUILD=/path/to/netdata/build] [VECTORS_WORK=/path/to/workdir] gen.sh
#
# It compiles src/database/engine/page.c from NETDATA_SRC with the flags of the shared C-oracle helper, links it with
# oracle-stubs.c and NETDATA_BUILD/liblibnetdata.a (which holds gorilla.cc and storage_number.c), and runs the
# programs. It never runs a netdata binary. VECTORS_WORK keeps the build outputs; without it a temporary directory
# is used and removed.

set -euo pipefail

GEN_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
OUT_DIR=$(cd -- "${GEN_DIR}/../../vectors/dbengine" && pwd)
REPO_ROOT=$(git -C "${GEN_DIR}" rev-parse --show-toplevel)
ORACLE_LIB="${REPO_ROOT}/src/crates/netdata-agent/c-oracle/lib.sh"
[[ -f "${ORACLE_LIB}" ]] || { printf >&2 'ERROR: C-oracle helper not found: %q\n' "${ORACLE_LIB}"; exit 1; }

# shellcheck source=/dev/null
source "${ORACLE_LIB}"
c_oracle_env

if [[ -n "${VECTORS_WORK:-}" ]]; then
    mkdir -p -- "${VECTORS_WORK}"
    WORK=$(realpath -e -- "${VECTORS_WORK}")
else
    WORK=$(mktemp -d "${TMPDIR:-/tmp}/dbengine-vectors.XXXXXX")
    trap 'rm -rf -- "${WORK}"' EXIT
fi

CXXFLAGS=("${CFLAGS[@]/-std=gnu11/-std=gnu++17}")
PAGE_TEST="${SRC}/src/database/engine/page_test.cc"
TEST_FLAGS=(-DHAVE_GTEST -I"${GEN_DIR}/fake-gtest" -I"${GEN_DIR}" -I"${SRC}/src/database/engine"
            -include page-test-compat.h -Wno-stringop-overflow)

# ---- common objects
run cc "${CFLAGS[@]}" -c "${SRC}/src/database/engine/page.c" -o "${WORK}/page.o"
run cc "${CFLAGS[@]}" -c "${GEN_DIR}/oracle-stubs.c" -o "${WORK}/oracle-stubs.o"

# ---- codec vectors
run c++ "${CXXFLAGS[@]}" "${GEN_DIR}/gen-gorilla.cc" "${WORK}/page.o" "${WORK}/oracle-stubs.o" \
    -o "${WORK}/gen-gorilla" "${LIBS[@]}"
# the rejected-chain cases log "invalid gorilla disk page chain." on stderr by design
run "${WORK}/gen-gorilla" "${OUT_DIR}" 2> "${WORK}/gen-gorilla.stderr"

# ---- page_test.cc, four builds
#   as_written: the file verbatim (slots_for_page(1024 * 1024), std::random_device seeds)
#   slots10240: slots_for_page(1024 * 1024) -> slots_for_page(10240), mt19937 seeded with 5489. 1024 * 1024 does not
#               fit struct pgd's uint16_t slots (page.c:35); 10240 is the smallest multiple of 1024 for which the
#               Roundtrip loops (page_test.cc:355-365, i * 1024 for i < 10) all run and terminate
#   u16max:     as slots10240 but slots_for_page(65535), the largest value that survives the uint16_t
#   array32:    slots10240 plus page_type = PAGE_METRICS (the ARRAY_32BIT branches of the switch statements)
grep -q 'slots_for_page(1024 \* 1024)' "${PAGE_TEST}" || die "page_test.cc changed: slots_for_page(1024 * 1024) not found"
grep -q 'std::mt19937 gen(rand_dev());' "${PAGE_TEST}" || die "page_test.cc changed: std::mt19937 gen(rand_dev()) not found"
grep -q 'static uint8_t page_type = PAGE_GORILLA_METRICS;' "${PAGE_TEST}" || die "page_test.cc changed: page_type"

cp -- "${PAGE_TEST}" "${WORK}/page_test.as_written.cc"
for n in 10240 65535; do
    sed -e "s/slots_for_page(1024 \\* 1024)/slots_for_page(${n})/" \
        -e 's/std::mt19937 gen(rand_dev());/std::mt19937 gen(5489u); (void)rand_dev;/' \
        "${PAGE_TEST}" > "${WORK}/page_test.slots${n}.cc"
done
mv -- "${WORK}/page_test.slots65535.cc" "${WORK}/page_test.u16max.cc"
sed -e 's/static uint8_t page_type = PAGE_GORILLA_METRICS;/static uint8_t page_type = PAGE_METRICS;/' \
    "${WORK}/page_test.slots10240.cc" > "${WORK}/page_test.array32.cc"

run c++ "${CXXFLAGS[@]}" -c "${GEN_DIR}/pgd-tests-main.cc" -o "${WORK}/pgd-tests-main.o"
for v in as_written slots10240 u16max array32; do
    run c++ "${CXXFLAGS[@]}" "${TEST_FLAGS[@]}" -c "${WORK}/page_test.${v}.cc" -o "${WORK}/page_test.${v}.o"
    run c++ "${CXXFLAGS[@]}" -Wno-stringop-overflow "${WORK}/page_test.${v}.o" "${WORK}/pgd-tests-main.o" "${WORK}/page.o" \
        "${WORK}/oracle-stubs.o" -o "${WORK}/pgd-tests.${v}" "${LIBS[@]}"
done

# a test that fails is data here, not an error
run_tests() { # variant filter
    FAKE_GTEST_FILTER="$2" FAKE_GTEST_TIMEOUT=120 "${WORK}/pgd-tests.$1" > "${WORK}/pgd-tests.$1.jsonl" || true
}
run_tests as_written ""
run_tests slots10240 ""
run_tests u16max ""
# the array32 build runs only the tests with PAGE_METRICS branches. Excluded: CopyToExtent and the three
# RejectCorrupt*/StopCorrupt* tests (they index gorilla buffer offsets in an ARRAY_32BIT page: out-of-bounds stack
# accesses) and Roundtrip (with 1024 slots the loop at page_test.cc:365 starts at i * 1024 > slots for i >= 2 and runs
# until size_t wraps; its iteration count at the timeout is not reproducible)
run_tests array32 "PGD.EmptyOrNull,PGD.Create,PGD.CursorFullPage,PGD.CursorHalfPage,PGD.MemoryFootprint,PGD.DiskFootprint"

variant_json() { # name transform
    printf '{"variant":"%s","source_transform":"%s","tests":[' "$1" "$2"
    paste -sd, "${WORK}/pgd-tests.$1.jsonl"
    printf ']}'
}
{
    printf '{"generator":"gen/gen.sh + gen/fake-gtest (page_test.cc compiled with -DHAVE_GTEST against the real page.c); do not edit",'
    printf '"reference":"netdata/netdata @ 1e97a0fc9e","source":"src/database/engine/page_test.cc",'
    printf '"compat":"gen/page-test-compat.h maps PAGE_METRICS/PAGE_GORILLA_METRICS and pgc_destroy(cache)",\n"variants":[\n'
    variant_json as_written "none (verbatim)"
    printf ',\n'
    variant_json slots10240 "slots_for_page(1024 * 1024) -> slots_for_page(10240); std::mt19937 gen(rand_dev()) -> std::mt19937 gen(5489u)"
    printf ',\n'
    variant_json u16max "slots_for_page(1024 * 1024) -> slots_for_page(65535); std::mt19937 gen(rand_dev()) -> std::mt19937 gen(5489u)"
    printf ',\n'
    variant_json array32 "slots10240 plus page_type = PAGE_METRICS (RRDENG_PAGE_TYPE_ARRAY_32BIT); tests filtered"
    printf '\n]}\n'
} > "${OUT_DIR}/pgd-tests-run.json"

printf >&2 "vectors written to %s\n" "${OUT_DIR}"
