// SPDX-License-Identifier: GPL-3.0-or-later
//
// Golden vector generator for the netdata-agent-health crate's matching of rules against hosts and charts.
//
// Linked like gen-health-vectors (the same sources and stubs, see gen-health-vectors.sh), it reads the scenario file
// given as its second argument and writes, into the directory given as the first:
//
//   labels.tsv   set <label set> then <name> <value> per label: a label set as C holds it, in name order;
//                match <label set> <pattern text> <0 or 1>: pattern_array_label_match() of the text compiled as a
//                      rule's `host labels` or `chart labels` value is (trim_and_add_key_to_values())
//   link.tsv     <scenario> <host> <chart id> link <rule name> <index in its chain> <is_template> <hash>: one row per
//                      rule C would link to the chart, in C's order (health_prototype_alerts_for_rrdset_incrementally():
//                      every rule that passed `enabled alarms`, the host's labels, `on` and the chart's labels; the
//                      alert store then keeps the first of a name);
//                <scenario> <host> <chart id> end <number of links>: after each chart
//
// The scenario file (tests/corpus/match/scenarios.txt) holds a directive per line:
//   labelset <name> <label>=<value>|<label>=<value>|...   defines a label set ("-" alone for an empty one)
//   pattern <text>                                        one match row per label set defined so far
//   scenario <name>                                       empties the prototype store; `enabled alarms` is "*" again
//   enabled <text>                                        [health] enabled alarms
//   load <stock: 0 or 1> <path>                           reads a health.d file into the store
//   host <name> <label set, or nolabels>                  the host of the charts that follow
//   chart <id> <name> <context> <label set, or nolabels>  applies every stored rule to this chart
// Paths are relative to the crate's directory, where the program runs.
//
// Field encoding: see health-oracle.h.

#include "health-oracle.h"

// health_prototypes.c has no header for it; rrdlabels.c declares it the same way for its own test
struct pattern_array *trim_and_add_key_to_values(struct pattern_array *pa, const char *key, STRING *input);

struct label_set {
    char *name;
    RRDLABELS *labels;
};

static struct label_set *sets;
static size_t sets_used, sets_size;

struct pair {
    char *name;
    char *value;
};

struct pairs {
    struct pair *list;
    size_t used, size;
};

static int collect_label(const char *name, const char *value, RRDLABEL_SRC ls, void *data) {
    (void)ls;
    struct pairs *pairs = data;
    if(pairs->used == pairs->size) {
        pairs->size = pairs->size ? pairs->size * 2 : 16;
        pairs->list = reallocz(pairs->list, pairs->size * sizeof(*pairs->list));
    }
    pairs->list[pairs->used].name = strdupz(name);
    pairs->list[pairs->used].value = strdupz(value);
    pairs->used++;
    return 1;
}

static int by_name(const void *a, const void *b) {
    return strcmp(((const struct pair *)a)->name, ((const struct pair *)b)->name);
}

static RRDLABELS *find_set(const char *name, size_t line_no) {
    if(strcmp(name, "nolabels") == 0)
        return NULL;
    for(size_t i = 0; i < sets_used; i++)
        if(strcmp(sets[i].name, name) == 0)
            return sets[i].labels;
    fprintf(stderr, "scenario line %zu: unknown label set '%s'\n", line_no, name);
    exit(1);
}

// "labelset <name> <label>=<value>|..." : the set, added as a collector's or a configuration's labels are
static void define_set(FILE *out, char *rest, size_t line_no) {
    char *name = strsep(&rest, " ");
    if(!name || !*name || !rest) {
        fprintf(stderr, "scenario line %zu: labelset needs a name and its labels\n", line_no);
        exit(1);
    }

    RRDLABELS *labels = rrdlabels_create();
    if(strcmp(rest, "-") != 0) {
        char *label;
        while((label = strsep(&rest, "|"))) {
            char *value = strchr(label, '=');
            if(!value) {
                fprintf(stderr, "scenario line %zu: a label without '='\n", line_no);
                exit(1);
            }
            *value++ = '\0';
            rrdlabels_add(labels, label, value, RRDLABEL_SRC_CONFIG);
        }
    }

    if(sets_used == sets_size) {
        sets_size = sets_size ? sets_size * 2 : 64;
        sets = reallocz(sets, sets_size * sizeof(*sets));
    }
    sets[sets_used].name = strdupz(name);
    sets[sets_used].labels = labels;
    sets_used++;

    struct pairs pairs = { 0 };
    rrdlabels_walkthrough_read(labels, collect_label, &pairs);
    if(pairs.used)
        qsort(pairs.list, pairs.used, sizeof(*pairs.list), by_name);

    fputs("set\t", out);
    oracle_esc(out, name);
    for(size_t i = 0; i < pairs.used; i++) {
        fputc('\t', out);
        oracle_esc(out, pairs.list[i].name);
        fputc('\t', out);
        oracle_esc(out, pairs.list[i].value);
        freez(pairs.list[i].name);
        freez(pairs.list[i].value);
    }
    fputc('\n', out);
    freez(pairs.list);
}

static void match_pattern(FILE *out, const char *text) {
    for(size_t i = 0; i < sets_used; i++) {
        STRING *input = string_strdupz(text);
        struct pattern_array *pa = trim_and_add_key_to_values(NULL, NULL, input);
        bool matched = pattern_array_label_match(pa, sets[i].labels, '=', NULL);
        pattern_array_free(pa);
        string_freez(input);

        fputs("match\t", out);
        oracle_esc(out, sets[i].name);
        fputc('\t', out);
        oracle_esc(out, text);
        fprintf(out, "\t%d\n", matched ? 1 : 0);
    }
}

static FILE *out_open(const char *dir, const char *name, const char *columns) {
    char path[4096];
    snprintf(path, sizeof(path), "%s/%s", dir, name);
    FILE *f = fopen(path, "w");
    if(!f) {
        fprintf(stderr, "cannot create %s\n", path);
        exit(1);
    }
    fprintf(f, "# generated by tests/oracle/gen-link-vectors.c from the C implementation; do not edit\n");
    fprintf(f, "# columns: %s\n", columns);
    return f;
}

static void set_enabled_alerts(const char *text) {
    simple_pattern_free(health_globals.config.enabled_alerts);
    health_globals.config.enabled_alerts = simple_pattern_create(text, NULL, SIMPLE_PATTERN_EXACT, true);
}

// the prefix a link row starts with, built once per chart
static char *link_prefix(const char *scenario, const char *host, const char *chart) {
    char *text = NULL;
    size_t size = 0;
    FILE *f = open_memstream(&text, &size);
    oracle_esc(f, scenario);
    fputc('\t', f);
    oracle_esc(f, host);
    fputc('\t', f);
    oracle_esc(f, chart);
    fclose(f);
    return text;
}

int main(int argc, char **argv) {
    if(argc != 3) {
        fprintf(stderr, "usage: %s <output directory> <scenario file>\n", argv[0]);
        return 1;
    }

    string_init();
    time_grouping_init();
    rrdlabels_aral_init(false);
    health_init_prototypes();
    health_globals.config.default_exec = string_strdupz("/oracle/plugins.d/alarm-notify.sh");
    health_globals.config.default_recipient = string_strdupz("root");
    set_enabled_alerts("*");

    // C's own expectations of the matcher hold on this build
    int errors = rrdlabels_unittest();
    if(errors) {
        fprintf(stderr, "rrdlabels_unittest() found %d errors\n", errors);
        return 1;
    }

    FILE *labels_out = out_open(argv[1], "labels.tsv",
                                "set, label set, then name and value per label (in name order) | match, label set, "
                                "pattern text, result");
    oracle_links = out_open(argv[1], "link.tsv",
                            "scenario, host, chart id, then: link, rule name, index in its chain, is_template, hash | "
                            "end, number of links");

    FILE *input = fopen(argv[2], "r");
    if(!input) {
        perror(argv[2]);
        return 1;
    }

    static RRDHOST host;
    char scenario[1024] = "";
    char host_name[1024] = "";
    bool have_host = false;

    char *line = NULL;
    size_t size = 0, line_no = 0;
    ssize_t len;
    while((len = getline(&line, &size, input)) > 0) {
        line_no++;
        if(line[len - 1] == '\n')
            line[len - 1] = '\0';
        if(!*line || *line == '#')
            continue;

        char *rest = line;
        char *directive = strsep(&rest, " ");
        if(!rest)
            rest = "";

        if(strcmp(directive, "labelset") == 0)
            define_set(labels_out, rest, line_no);
        else if(strcmp(directive, "pattern") == 0)
            match_pattern(labels_out, rest);
        else if(strcmp(directive, "scenario") == 0) {
            snprintf(scenario, sizeof(scenario), "%s", rest);
            dictionary_flush(health_globals.prototypes.dict);
            set_enabled_alerts("*");
            have_host = false;
        }
        else if(strcmp(directive, "enabled") == 0)
            set_enabled_alerts(rest);
        else if(strcmp(directive, "load") == 0) {
            if(!*scenario || !(rest[0] == '0' || rest[0] == '1') || rest[1] != ' ') {
                fprintf(stderr, "scenario line %zu: load <0|1> <path> inside a scenario\n", line_no);
                return 1;
            }
            if(health_readfile(rest + 2, NULL, rest[0] == '1') != 1) {
                fprintf(stderr, "scenario line %zu: cannot read %s\n", line_no, rest + 2);
                return 1;
            }
        }
        else if(strcmp(directive, "host") == 0) {
            char *name = strsep(&rest, " ");
            if(!*scenario || !name || !rest) {
                fprintf(stderr, "scenario line %zu: host <name> <label set> inside a scenario\n", line_no);
                return 1;
            }
            snprintf(host_name, sizeof(host_name), "%s", name);
            memset(&host, 0, sizeof(host));
            host.rrdlabels = find_set(rest, line_no);
            have_host = true;
        }
        else if(strcmp(directive, "chart") == 0) {
            char *id = strsep(&rest, " ");
            char *name = rest ? strsep(&rest, " ") : NULL;
            char *context = rest ? strsep(&rest, " ") : NULL;
            if(!have_host || !id || !name || !context || !rest) {
                fprintf(stderr, "scenario line %zu: chart <id> <name> <context> <label set> after a host\n", line_no);
                return 1;
            }

            // what the matcher reads of a chart (health_prototype_matches_rrdset()) and of its host
            RRDSET st = { 0 };
            st.id = string_strdupz(id);
            st.name = string_strdupz(name);
            st.context = string_strdupz(context);
            st.rrdlabels = find_set(rest, line_no);
            st.rrdhost = &host;

            char *prefix = link_prefix(scenario, host_name, id);
            oracle_link_prefix = prefix;
            oracle_link_count = 0;
            health_prototype_alerts_for_rrdset_incrementally(&st);
            fprintf(oracle_links, "%s\tend\t%zu\n", prefix, oracle_link_count);
            free(prefix);

            string_freez(st.id);
            string_freez(st.name);
            string_freez(st.context);
        }
        else {
            fprintf(stderr, "scenario line %zu: unknown directive '%s'\n", line_no, directive);
            return 1;
        }
    }
    free(line);
    fclose(input);
    fclose(labels_out);
    fclose(oracle_links);
    return 0;
}
