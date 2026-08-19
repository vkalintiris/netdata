#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later

# OBSOLETE - REPLACED WITH the alert notification dispatcher itself.

dir="$(dirname "${0}")"

for notifier in "${dir}/alarm-notify" "${dir}/alarm-notify.exe" "${dir}/alarm-notify.sh"; do
  if [ -x "${notifier}" ]; then
    exec "${notifier}" "${@}"
  fi
done

echo >&2 "Cannot find an alert notification program in '${dir}'."
exit 1
