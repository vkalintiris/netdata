# SPDX-License-Identifier: GPL-3.0-or-later

set(CLAIM_PLUGIN_FILES claim.c claim.h)
target_sources(netdata PRIVATE ${CLAIM_PLUGIN_FILES})
