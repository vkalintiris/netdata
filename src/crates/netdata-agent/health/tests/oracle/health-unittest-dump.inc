// Inserted by gen-health-vectors.sh into a copy of src/health/health-config-unittest.c, right after its includes:
// each function the configuration path ports gets a wrapper and a macro of its name, so every call C's unit test
// makes leaves a row with its inputs and what C returned, whatever table the call came from. A main() appended to
// the copy runs C's test, which must pass, and adds the expectations of its one file-scope table.
//
// c_unittest.tsv rows (the first field names the case, the second the kind):
//   <input> lookup <return> <time_group> <condition> <value bits> <after> <before> <update_every> <options>
//           <dimensions>                 health_parse_db_lookup() on a zeroed config whose value is NaN
//   <input> expect <should succeed> <time_group> <condition> <value bits> <after> <before>
//                                        the same table's hand-written expectations (C checks them only on success)
//   <input> delay <up> <down> <max> <multiplier bits> | <return> <up> <down> <max> <multiplier bits>
//                                        health_parse_delay(): the four values before, then after
//   <delay> multiplier <multiplier bits> <maximum> <result>     health_delay_apply_multiplier()
//   <name> add <rules> <is_template> <on> <update_every> <after> <calc> <warn> <crit> <multiplier bits> | <return>
//           <message>                    health_prototype_add() of a hand-built prototype (its first rule's fields)
//   <content> readfile <return> <names in the store, each with its chain length, joined with 0x1f>
// Field encoding: see health-oracle.h (NULL strings are "\x00").

#include "health-oracle.h"

static FILE *dump_file;

static unsigned dump_float_bits(float f) {
    uint32_t u;
    memcpy(&u, &f, sizeof(u));
    return (unsigned)u;
}

static unsigned long long dump_double_bits(NETDATA_DOUBLE d) {
    uint64_t u;
    memcpy(&u, &d, sizeof(u));
    return (unsigned long long)u;
}

static void dump_string(STRING *s) {
    fputc('\t', dump_file);
    oracle_esc(dump_file, s ? string2str(s) : NULL);
}

static void dump_expression(EVAL_EXPRESSION *e) {
    fputc('\t', dump_file);
    oracle_esc(dump_file, e ? expression_source(e) : NULL);
}

static int dump_db_lookup(size_t line, const char *filename, char *string, struct rrd_alert_config *ac) {
    char *input = strdupz(string); // the parser writes into its input
    int rc = health_parse_db_lookup(line, filename, string, ac);
    oracle_esc(dump_file, input);
    fprintf(dump_file, "\tlookup\t%d\t%s\t%d\t%016llx\t%d\t%d\t%d\t%08x", rc, time_grouping_id2txt(ac->time_group),
            (int)ac->time_group_condition, dump_double_bits(ac->time_group_value), ac->after, ac->before,
            ac->update_every, (unsigned)ac->options);
    dump_string(ac->dimensions);
    fputc('\n', dump_file);
    freez(input);
    return rc;
}

static int dump_delay(size_t line, const char *filename, char *string, int *up, int *down, int *max, float *multiplier) {
    char *input = strdupz(string);
    oracle_esc(dump_file, input);
    fprintf(dump_file, "\tdelay\t%d\t%d\t%d\t%08x", *up, *down, *max, dump_float_bits(*multiplier));
    int rc = health_parse_delay(line, filename, string, up, down, max, multiplier);
    fprintf(dump_file, "\t|\t%d\t%d\t%d\t%d\t%08x\n", rc, *up, *down, *max, dump_float_bits(*multiplier));
    freez(input);
    return rc;
}

static int dump_apply_multiplier(int delay, float multiplier, int maximum) {
    int result = health_delay_apply_multiplier(delay, multiplier, maximum);
    fprintf(dump_file, "%d\tmultiplier\t%08x\t%d\t%d\n", delay, dump_float_bits(multiplier), maximum, result);
    return result;
}

static bool dump_prototype_add(RRD_ALERT_PROTOTYPE *ap, const char **msg) {
    size_t rules = 0;
    for(RRD_ALERT_PROTOTYPE *t = ap; t; t = t->_internal.next)
        rules++;

    oracle_esc(dump_file, ap->config.name ? string2str(ap->config.name) : NULL);
    fprintf(dump_file, "\tadd\t%zu\t%d", rules, ap->match.is_template ? 1 : 0);
    dump_string(ap->match.is_template ? ap->match.on.context : ap->match.on.chart);
    fprintf(dump_file, "\t%d\t%d", ap->config.update_every, ap->config.after);
    dump_expression(ap->config.calculation);
    dump_expression(ap->config.warning);
    dump_expression(ap->config.critical);
    fprintf(dump_file, "\t%08x", dump_float_bits(ap->config.delay_multiplier));

    const char *reason = NULL;
    bool rc = health_prototype_add(ap, &reason);
    if(msg)
        *msg = reason;
    fprintf(dump_file, "\t|\t%d\t", rc ? 1 : 0);
    oracle_esc(dump_file, reason);
    fputc('\n', dump_file);
    return rc;
}

static int dump_readfile(const char *filename, void *data, bool stock_config) {
    // the file's bytes are the case: the Rust test writes them to a file of its own
    FILE *fp = fopen(filename, "r");
    if(fp) {
        char buffer[4096];
        size_t len;
        while((len = fread(buffer, 1, sizeof(buffer), fp)) > 0)
            oracle_esc_bytes(dump_file, buffer, len);
        fclose(fp);
    }

    int rc = health_readfile(filename, data, stock_config);
    fprintf(dump_file, "\treadfile\t%d\t", rc);

    size_t entries = 0;
    RRD_ALERT_PROTOTYPE *ap;
    dfe_start_read(health_globals.prototypes.dict, ap) {
        size_t rules = 0;
        for(RRD_ALERT_PROTOTYPE *t = ap; t; t = t->_internal.next)
            rules++;
        if(entries++)
            fputs("\\x1f", dump_file);
        oracle_esc(dump_file, ap_dfe.name);
        fprintf(dump_file, "=%zu", rules);
    }
    dfe_done(ap);
    fputc('\n', dump_file);
    return rc;
}

#define health_parse_db_lookup(line, filename, string, ac) dump_db_lookup(line, filename, string, ac)
#define health_parse_delay(line, filename, string, up, down, max, multiplier) \
    dump_delay(line, filename, string, up, down, max, multiplier)
#define health_delay_apply_multiplier(delay, multiplier, maximum) dump_apply_multiplier(delay, multiplier, maximum)
#define health_prototype_add(ap, msg) dump_prototype_add(ap, msg)
#define health_readfile(filename, data, stock_config) dump_readfile(filename, data, stock_config)
