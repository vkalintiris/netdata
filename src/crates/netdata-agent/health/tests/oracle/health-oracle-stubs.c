// SPDX-License-Identifier: GPL-3.0-or-later
//
// Link stubs so the real health configuration path (src/health/health_config.c, health_prototypes.c,
// health_dyncfg.c, compiled from the reference tree) runs outside the netdata daemon: the reader, the sub-parsers,
// the prototype store and the hash need no host, no database and no DynCfg.
//
//   - health_globals: the initializer of src/health/health.c:6-30; the programs set the default exec and recipient;
//   - health_user_config_dir / health_stock_config_dir: only health_reload_prototypes() calls them (not run here);
//   - localhost, rrdhost_root_index, rrdcalc_*, rrdlabels_match_simple_pattern_parsed, service_running: linking and
//     matching (health_prototypes.c:525-647), reached only with hosts;
//   - dyncfg_add / dyncfg_del / dyncfg_status: the DynCfg nodes of health_dyncfg.c, registered only on reload;
//   - aclk_send_alert_configuration / alert_hash_has_transitioned: the Cloud push of a DYNCFG rule's hash
//     (health_prototypes.c:404-410);
//   - rrd_alert_match_cleanup / rrd_alert_config_cleanup: copies of src/health/rrdcalc.c:800-837;
//   - sql_alert_store_config: the dumper. C calls it once per accepted rule, right after the rule's hash is made and
//     before the default exec and recipient are filled (health_prototypes.c:396-402, :477-483); the real one binds
//     the alert_hash row (src/database/sqlite/sqlite_health.c). Here it writes the rule as a vector row.

#include "health-oracle.h"

struct health_plugin_globals health_globals = {
    .initialization = {
        .spinlock = SPINLOCK_INITIALIZER,
        .done = false,
    },
    .config = {
        .enabled = true,
        .stock_enabled = true,
        .use_summary_for_notifications = true,

        .health_log_entries_max = HEALTH_LOG_ENTRIES_DEFAULT,
        .health_log_retention_s = HEALTH_LOG_RETENTION_DEFAULT,

        .default_warn_repeat_every = 0,
        .default_crit_repeat_every = 0,

        .run_at_least_every_seconds = 10,
        .postpone_alarms_during_hibernation_for_seconds = 60,
        .notification_execution_timeout_seconds = 120,
    },
    .prototypes = {
        .dict = NULL,
        .registering = false,
    }
};

RRDHOST *localhost = NULL;
DICTIONARY *rrdhost_root_index = NULL;
__thread bool is_health_thread = false;
struct dictionary_stats dictionary_stats_category_rrdhealth = { .name = "health" };

const char *health_user_config_dir(void) {
    return "/oracle/etc/health.d";
}

const char *health_stock_config_dir(void) {
    return "/oracle/lib/health.d";
}

bool service_running(SERVICE_TYPE service) {
    (void)service;
    return true;
}

bool rrdcalc_add_from_prototype(RRDHOST *host, RRDSET *st, RRD_ALERT_PROTOTYPE *ap) {
    (void)host;
    (void)st;
    (void)ap;
    return false;
}

void rrdcalc_delete_all(RRDHOST *host) {
    (void)host;
}

void rrdcalc_unlink_and_delete(RRDHOST *host, RRDCALC *rc, bool having_ll_wrlock) {
    (void)host;
    (void)rc;
    (void)having_ll_wrlock;
}

void rrdcalc_unlink_and_delete_all_rrdset_alerts(RRDSET *st) {
    (void)st;
}

SIMPLE_PATTERN_RESULT rrdlabels_match_simple_pattern_parsed(RRDLABELS *labels, SIMPLE_PATTERN *pattern, char equal, size_t *searches) {
    (void)labels;
    (void)pattern;
    (void)equal;
    (void)searches;
    return SP_NOT_MATCHED;
}

bool dyncfg_add(const struct dyncfg_add_inline_spec *spec) {
    (void)spec;
    return true;
}

void dyncfg_del(RRDHOST *host, const char *id) {
    (void)host;
    (void)id;
}

void dyncfg_status(RRDHOST *host, const char *id, DYNCFG_STATUS status) {
    (void)host;
    (void)id;
    (void)status;
}

void aclk_send_alert_configuration(char *config_hash) {
    (void)config_hash;
}

bool alert_hash_has_transitioned(nd_uuid_t *hash_id) {
    (void)hash_id;
    return true;
}

void rrd_alert_match_cleanup(struct rrd_alert_match *am) {
    if(am->is_template)
        string_freez(am->on.context);
    else
        string_freez(am->on.chart);

    string_freez(am->host_labels);
    pattern_array_free(am->host_labels_pattern);

    string_freez(am->chart_labels);
    pattern_array_free(am->chart_labels_pattern);

    memset(am, 0, sizeof(*am));
}

void rrd_alert_config_cleanup(struct rrd_alert_config *ac) {
    string_freez(ac->name);

    string_freez(ac->exec);
    string_freez(ac->recipient);

    string_freez(ac->classification);
    string_freez(ac->component);
    string_freez(ac->type);

    string_freez(ac->source);
    string_freez(ac->units);
    string_freez(ac->summary);
    string_freez(ac->info);

    string_freez(ac->dimensions);

    expression_free(ac->calculation);
    expression_free(ac->warning);
    expression_free(ac->critical);

    memset(ac, 0, sizeof(*ac));
}

// ------------------------------------------------------------------------------------------------
// the dumper

FILE *oracle_rules = NULL;
const char *oracle_item = "";

void oracle_esc_bytes(FILE *f, const void *data, size_t len) {
    const unsigned char *s = data;
    for(size_t i = 0; i < len; i++) {
        unsigned char c = s[i];
        if(c == '\\')
            fputs("\\\\", f);
        else if(c >= 0x20 && c <= 0x7e && c != '#')
            fputc(c, f);
        else
            fprintf(f, "\\x%02x", c);
    }
}

void oracle_esc(FILE *f, const char *s) {
    if(!s)
        fputs("\\x00", f);
    else
        oracle_esc_bytes(f, s, strlen(s));
}

// a STRING: NULL when unset (string2str() would print it as "")
static void field_string(FILE *f, STRING *s) {
    fputc('\t', f);
    oracle_esc(f, s ? string2str(s) : NULL);
}

// an expression: its source text, then what it was parsed as; NULL for both when absent
static void field_expression(FILE *f, EVAL_EXPRESSION *e) {
    fputc('\t', f);
    oracle_esc(f, e ? expression_source(e) : NULL);
    fputc('\t', f);
    oracle_esc(f, e ? expression_parsed_as(e) : NULL);
}

void sql_alert_store_config(RRD_ALERT_PROTOTYPE *ap) {
    FILE *f = oracle_rules;
    if(!f)
        return;

    struct rrd_alert_match *am = &ap->match;
    struct rrd_alert_config *ac = &ap->config;

    oracle_esc(f, oracle_item);
    fputs("\trule", f);
    field_string(f, ac->name);
    fprintf(f, "\t%d\t%d", am->is_template ? 1 : 0, am->enabled ? 1 : 0);
    field_string(f, am->is_template ? am->on.context : am->on.chart);
    field_string(f, am->host_labels);
    field_string(f, am->chart_labels);
    field_string(f, ac->exec);
    field_string(f, ac->recipient);
    field_string(f, ac->classification);
    field_string(f, ac->component);
    field_string(f, ac->type);
    fprintf(f, "\t%d", (int)ac->source_type);
    field_string(f, ac->source);
    field_string(f, ac->units);
    field_string(f, ac->summary);
    field_string(f, ac->info);
    fprintf(f, "\t%d\t%u", ac->update_every, (unsigned)ac->alert_action_options);
    field_string(f, ac->dimensions);

    uint64_t value_bits;
    memcpy(&value_bits, &ac->time_group_value, sizeof(value_bits));
    fprintf(f, "\t%s\t%d\t%016llx\t%d\t%d\t%d\t%d\t%08x",
            time_grouping_id2txt(ac->time_group), (int)ac->time_group_condition, (unsigned long long)value_bits,
            (int)ac->dims_group, (int)ac->data_source, ac->before, ac->after, (unsigned)ac->options);

    field_expression(f, ac->calculation);
    field_expression(f, ac->warning);
    field_expression(f, ac->critical);

    uint32_t multiplier_bits;
    memcpy(&multiplier_bits, &ac->delay_multiplier, sizeof(multiplier_bits));
    fprintf(f, "\t%d\t%d\t%d\t%08x\t%d\t%u\t%u",
            ac->delay_up_duration, ac->delay_down_duration, ac->delay_max_duration, (unsigned)multiplier_bits,
            ac->has_custom_repeat_config ? 1 : 0, ac->warn_repeat_every, ac->crit_repeat_every);

    // the bytes that are hashed, from the production writer, and the hash C just stored in the rule
    CLEAN_BUFFER *wb = buffer_create(100, NULL);
    health_prototype_to_json(wb, ap, true);
    ND_UUID uuid = UUID_generate_from_hash(buffer_tostring(wb), buffer_strlen(wb));
    if(memcmp(uuid.uuid, ac->hash_id, sizeof(uuid.uuid)) != 0) {
        fprintf(stderr, "oracle: the rule's hash is not the hash of its JSON in %s\n", oracle_item);
        exit(1);
    }

    fputc('\t', f);
    for(size_t i = 0; i < sizeof(ac->hash_id); i++)
        fprintf(f, "%02x", (unsigned)ac->hash_id[i]);
    fputc('\t', f);
    oracle_esc_bytes(f, buffer_tostring(wb), buffer_strlen(wb));
    fputc('\n', f);
}
