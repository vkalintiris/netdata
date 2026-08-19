#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Compatibility shim for the `custom` notification method.
#
# `custom_sender()` is a shell function users write inside
# health_alarm_notify.conf. The native notifier cannot execute shell, so it runs
# this shim instead: the shim re-reads the configuration to pick the function up,
# restores the helpers the function is documented to have available, and calls it.
#
# Every notification variable arrives in the environment already, under the same
# names the function has always used (${host}, ${status}, ${alarm}, ...), so an
# existing custom_sender() body needs no changes.
#
# Usage: custom-sender.sh "<space-separated recipients>"
#
# For a portable sender that needs no shell at all, set CUSTOM_SENDER_COMMAND in
# health_alarm_notify.conf instead. It is run with the recipients as its first
# argument and the same variables in its environment, on every platform.

#shellcheck source=/dev/null disable=SC2034,SC2154

to="${1}"

# The notifier ran under LC_ALL=C, which is what makes urlencode() below encode bytes
# rather than code points.
export LC_ALL=C

# ---------------------------------------------------------------------------
# helpers the function may call

# Log through the notifier: everything on stderr is relayed to the Agent's log.
info()    { printf '%s\n' "${*}" >&2; }
warning() { printf 'WARNING: %s\n' "${*}" >&2; }
error()   { printf 'ERROR: %s\n' "${*}" >&2; }
debug()   { [ "${NETDATA_ALARM_NOTIFY_DEBUG-0}" = "1" ] && printf 'DEBUG: %s\n' "${*}" >&2; return 0; }
fatal()   { error "${*}"; exit 1; }

# Percent-encode ${1}, setting REPLY and echoing it - both call styles are used.
urlencode() {
  local string="${1}" strlen encoded pos c o
  strlen=${#string}
  for ((pos = 0; pos < strlen; pos++)); do
    c=${string:pos:1}
    case "${c}" in
      [-_.~a-zA-Z0-9]) o="${c}" ;;
      *) printf -v o '%%%02x' "'${c}" ;;
    esac
    encoded+="${o}"
  done
  REPLY="${encoded}"
  echo "${REPLY}"
}

# Human-readable duration, same output as the notifier's own formatter.
duration4human() {
  local s="${1}" d=0 h=0 m=0 ds="day" hs="hour" ms="minute" ss="second" ret
  d=$((s / 86400)); s=$((s - (d * 86400)))
  h=$((s / 3600));  s=$((s - (h * 3600)))
  m=$((s / 60));    s=$((s - (m * 60)))
  if [ ${d} -gt 0 ]; then
    [ ${m} -ge 30 ] && h=$((h + 1))
    [ ${d} -gt 1 ] && ds="days"; [ ${h} -gt 1 ] && hs="hours"
    if [ ${h} -gt 0 ]; then ret="${d} ${ds} and ${h} ${hs}"; else ret="${d} ${ds}"; fi
  elif [ ${h} -gt 0 ]; then
    [ ${s} -ge 30 ] && m=$((m + 1))
    [ ${h} -gt 1 ] && hs="hours"; [ ${m} -gt 1 ] && ms="minutes"
    if [ ${m} -gt 0 ]; then ret="${h} ${hs} and ${m} ${ms}"; else ret="${h} ${hs}"; fi
  elif [ ${m} -gt 0 ]; then
    [ ${m} -gt 1 ] && ms="minutes"; [ ${s} -gt 1 ] && ss="seconds"
    if [ ${s} -gt 0 ]; then ret="${m} ${ms} and ${s} ${ss}"; else ret="${m} ${ms}"; fi
  else
    [ ${s} -gt 1 ] && ss="seconds"; ret="${s} ${ss}"
  fi
  REPLY="${ret}"
  echo "${REPLY}"
}

# HTTP helper: prints the response status code, as it always did.
#
# `curl` is resolved after the configuration is sourced, because the shipped
# health_alarm_notify.conf contains `curl=""` - the script also resolved it after
# sourcing, and doing it before would leave every custom_sender() without an HTTP
# client while still reporting success.
docurl() {
  if [ -z "${curl}" ]; then
    error "cannot find curl; custom_sender() cannot make HTTP requests"
    return 1
  fi
  # shellcheck disable=SC2086
  ${curl} ${curl_options} --write-out "%{http_code}" --output /dev/null --silent --show-error "${@}"
}

# ---------------------------------------------------------------------------
# pick up the user's custom_sender()

# A stub, so a configuration without the function still exits cleanly.
custom_sender() {
  info "custom notification mechanism is not configured; not sending to '${to}'"
  return 1
}

: "${NETDATA_STOCK_CONFIG_DIR:=/usr/lib/netdata/conf.d}"
: "${NETDATA_USER_CONFIG_DIR:=/etc/netdata}"

for CONFIG in \
  "${NETDATA_STOCK_CONFIG_DIR}/health_alarm_notify.conf" \
  "${NETDATA_USER_CONFIG_DIR}/health_alarm_notify.conf"
do
  [ -f "${CONFIG}" ] || continue
  source "${CONFIG}" || error "failed to load config file '${CONFIG}'."
done

# Now that the configuration has had its say, resolve what it did not set.
curl="${curl:-$(command -v curl 2>/dev/null)}"
curl_options="${curl_options:-}"

[ -z "${to}" ] && exit 1

custom_sender "${to}"
