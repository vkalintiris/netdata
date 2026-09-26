#!/usr/bin/env bash
# Regenerates the SQLite files of this crate the way packaging/cmake/Modules/NetdataSQLite.cmake does, and checks
# them against SHA256SUMS. Needs network access, a C compiler, make and tclsh (SQLite's configure uses them).
set -euo pipefail

SQLITE_VERSION_NUMBER="3530400"
SQLITE_VERSION_YEAR="2026"
SQLITE_ZIP_SHA256="d18fa15aec74d8c17e1463f861095adc01b5ad190256acb4f91d22f0368d232b"

RED='\033[0;31m'
YELLOW='\033[1;33m'
GRAY='\033[0;90m'
NC='\033[0m'

run() {
  printf >&2 "${GRAY}$(pwd) >${NC} "
  printf >&2 "${YELLOW}"
  printf >&2 "%q " "$@"
  printf >&2 "${NC}\n"
  if ! "$@"; then
    local exit_code=$?
    echo -e >&2 "${RED}[ERROR]${NC} Command failed with exit code ${exit_code}: ${YELLOW}$1${NC}"
    echo -e >&2 "${RED}        Full command:${NC} $*"
    echo -e >&2 "${RED}        Working dir:${NC} $(pwd)"
    return $exit_code
  fi
}

safe_dir() {
  local p
  p=$(realpath -e -- "${1:-}") && [[ -d "$p" && ${#p} -ge 10 && "$p" =~ ^/[^/]+/[^/]+/[^/]+(/.*)?$ ]] || {
    printf >&2 'ERROR: unsafe directory: %q\n' "${1:-}"
    exit 1
  }
  printf '%s\n' "$p"
}

CRATE=$(safe_dir "$(dirname -- "${BASH_SOURCE[0]}")") || exit 1
WORK=$(mktemp -d "${TMPDIR:-/tmp}/sqlite-regen.XXXXXX")
WORK=$(safe_dir "$WORK") || exit 1
trap 'rm -rf -- "$WORK"' EXIT

ZIP="sqlite-src-${SQLITE_VERSION_NUMBER}.zip"
cd "$WORK"
run curl -fsSLo "$ZIP" "https://www.sqlite.org/${SQLITE_VERSION_YEAR}/${ZIP}"
echo "${SQLITE_ZIP_SHA256}  ${ZIP}" | run sha256sum -c -
run unzip -q "$ZIP"
SRC=$(safe_dir "$WORK/sqlite-src-${SQLITE_VERSION_NUMBER}") || exit 1
run mkdir build
cd build
run "$SRC/configure" --enable-update-limit
run env MAKEFLAGS= make sqlite3.c sqlite3.h
run cp sqlite3.c sqlite3.h "$SRC/ext/recover/sqlite3recover.c" "$SRC/ext/recover/sqlite3recover.h" \
  "$SRC/ext/recover/dbdata.c" "$CRATE/sqlite3/"
cd "$CRATE/sqlite3"
run sha256sum -c ../SHA256SUMS
