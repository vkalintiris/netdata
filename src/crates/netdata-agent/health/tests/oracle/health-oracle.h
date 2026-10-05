// SPDX-License-Identifier: GPL-3.0-or-later
//
// Shared by the health configuration oracle's programs and its stubs: where the dumper writes.

#ifndef HEALTH_ORACLE_H
#define HEALTH_ORACLE_H

#include "health/health_internals.h"

// the vector file the stubbed sql_alert_store_config() writes a row to, and the corpus item being read
extern FILE *oracle_rules;
extern const char *oracle_item;

// the vector file the stubbed rrdcalc_add_from_prototype() writes a row to, the fields its rows start with (already
// escaped), and how many rows it wrote
extern FILE *oracle_links;
extern const char *oracle_link_prefix;
extern size_t oracle_link_count;

// Field encoding (netdata-agent-text's, decoded by its tests/common/mod.rs): bytes 0x20..0x7e except '\' and '#'
// are written as-is, '\' as "\\", every other byte as "\xHH". A NULL string is "\x00", which no C string can hold.
void oracle_esc(FILE *f, const char *s);
void oracle_esc_bytes(FILE *f, const void *data, size_t len);

// a rule's fields as a `rule` row of rules.tsv has them after its kind, up to the hash: each led by its tab
void oracle_rule_fields(FILE *f, RRD_ALERT_PROTOTYPE *ap);

#endif
