#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Script to test alarm notifications for netdata

dir="$(dirname "${0}")"

# A build with a Rust toolchain ships the native dispatcher; one without it ships the
# shell implementation. Exactly one of the two is installed.
for notifier in "${dir}/alarm-notify" "${dir}/alarm-notify.exe" "${dir}/alarm-notify.sh"; do
  if [ -x "${notifier}" ]; then
    "${notifier}" test "${1}"
    exit $?
  fi
done

echo >&2 "Cannot find an alert notification program in '${dir}'."
exit 1
