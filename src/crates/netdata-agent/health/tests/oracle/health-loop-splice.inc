// SPDX-License-Identifier: GPL-3.0-or-later
//
// Appended to a copy of src/health/health_event_loop.c, so that the oracle can call the file's static per-host
// pass. Nothing of C's text is changed: the copy is the file followed by these lines.

void oracle_health_event_loop_for_host(RRDHOST *host, bool apply_hibernation_delay, time_t now, time_t *next_run, ONEWAYALLOC *owa) {
    health_event_loop_for_host(host, apply_hibernation_delay, now, next_run, owa);
}
