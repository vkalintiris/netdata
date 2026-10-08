#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Regenerates tests/vectors/jsonc_doc.tsv of netdata-agent-text from json-c itself: what the system's json-c (the
# library the C agent links) makes of each text of tests/oracle/jsonc-doc-inputs.txt with the calls
# /api/v3/settings uses. Needs a C compiler and json-c's development files (pkg-config json-c).
#
# Usage: tests/oracle/gen-jsonc-doc.sh

set -euo pipefail

# shellcheck source=../../../c-oracle/lib.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/../../../c-oracle/lib.sh"

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
CRATE_DIR=$(cd -- "${SCRIPT_DIR}/../.." && pwd)
[[ -f "${CRATE_DIR}/Cargo.toml" && -d "${CRATE_DIR}/tests/vectors" ]] || die "unexpected crate layout at ${CRATE_DIR}"

pkg-config --exists json-c || die "json-c's development files are not installed (pkg-config json-c)"
read -r -a JSONC_CFLAGS <<< "$(pkg-config --cflags json-c)"
read -r -a JSONC_LIBS <<< "$(pkg-config --libs json-c)"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/netdata-agent-text-jsonc-doc.XXXXXX")
trap 'rm -rf -- "${WORK}"' EXIT

run cc -O2 -Wall -Wextra "${JSONC_CFLAGS[@]}" "${SCRIPT_DIR}/gen-jsonc-doc.c" -o "${WORK}/gen-jsonc-doc" "${JSONC_LIBS[@]}"
run "${WORK}/gen-jsonc-doc" "${SCRIPT_DIR}/jsonc-doc-inputs.txt" "${CRATE_DIR}/tests/vectors/jsonc_doc.tsv"
