#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Shared by the C oracles of the netdata-agent crates: small programs compiled against a CMake build of the C
# agent (liblibnetdata.a, config.h) with that build's flags, so they run the production code paths and produce
# the expected outputs the Rust tests compare against.
#
# Source it, then call c_oracle_env: it validates NETDATA_SRC / NETDATA_BUILD and sets SRC, BUILD, CFLAGS and
# LIBS for `cc "${CFLAGS[@]}" prog.c -o prog "${LIBS[@]}"`.

RED=$'\033[0;31m'
YELLOW=$'\033[1;33m'
GRAY=$'\033[0;90m'
NC=$'\033[0m'

run() {
    printf >&2 "%s%s >%s " "${GRAY}" "$(pwd)" "${NC}"
    printf >&2 "%s" "${YELLOW}"
    printf >&2 "%q " "$@"
    printf >&2 "%s\n" "${NC}"
    if ! "$@"; then
        local exit_code=$?
        printf >&2 "%s[ERROR]%s command failed with exit code %s: %s\n" "${RED}" "${NC}" "${exit_code}" "$*"
        return "${exit_code}"
    fi
}

die() {
    printf >&2 "%s[ERROR]%s %s\n" "${RED}" "${NC}" "$*"
    exit 1
}

c_oracle_env() {
    : "${NETDATA_SRC:?set NETDATA_SRC to a netdata source tree}"
    SRC=$(realpath -e -- "${NETDATA_SRC}") || die "NETDATA_SRC does not exist: ${NETDATA_SRC}"
    BUILD=$(realpath -e -- "${NETDATA_BUILD:-${SRC}/build}") || die "no build directory"
    [[ -f "${SRC}/src/libnetdata/libnetdata.h" ]] || die "not a netdata source tree: ${SRC}"
    [[ -f "${BUILD}/liblibnetdata.a" && -f "${BUILD}/config.h" ]] || die "not a netdata build directory: ${BUILD}"

    CFLAGS=(
        -D_GNU_SOURCE -O2 -g -DNDEBUG -std=gnu11 -flto=auto -fexceptions -fno-omit-frame-pointer
        -I/usr/include/uuid -I/usr/include/json-c
        -I"${BUILD}" -I"${BUILD}/sqlite-output" -I"${BUILD}/libbacktrace-install/include"
        -I"${SRC}/src" -I"${SRC}/src/libnetdata/netipc/include"
        -I"${SRC}/src/libnetdata/libjudy/vendored" -I"${SRC}/src/libnetdata/libjudy/vendored/JudyCommon"
    )
    LIBS=(
        "${BUILD}/liblibnetdata.a" "${BUILD}/libnetipc.a" "${BUILD}/libbacktrace-install/lib/libbacktrace.a"
        "${BUILD}/libjudy.a" -lm -lrt -lsystemd -lpthread -ljson-c -lyaml -lz -llz4 -lxxhash -lzstd
        -lbrotlidec -lbrotlienc -lbrotlicommon -luuid -luv -ldl -lssl -lcrypto -lcurl
    )
}
