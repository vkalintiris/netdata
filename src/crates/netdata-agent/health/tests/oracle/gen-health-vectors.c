// SPDX-License-Identifier: GPL-3.0-or-later
//
// Golden vector generator for the netdata-agent-health crate's configuration path.
//
// Linked with the health configuration sources of the reference tree and health-oracle-stubs.c (see
// gen-health-vectors.sh), it runs C's health_readfile() over a corpus of health.d files and writes, into the
// directory given as the first argument:
//
//   rules.tsv    for each corpus item, in order:
//                  <item> rule ...   one row per rule C accepted, written when C hashes and stores it, before the
//                                    default exec and recipient are filled (the stubbed sql_alert_store_config());
//                  <item> file <path> <stock> <return value>   one row per file read, after its rules;
//                  <item> entry <name> <index in the chain> <prototype enabled> <rule enabled> <exec> <recipient>
//                                    <hash>   the prototype store after the item, in its order
//   records.tsv  <item> <level> <errno> <message>: every record C logged while reading the item, in order; the
//                errno and the message as C's logfmt printed them (without their quotes), "-" for no errno
//
// The second argument is the list of items, one per line: <stock: 0 or 1> <path>. A path is a file, or a directory
// whose files are read in the order given by the lines that follow it with the same item (see the script). Paths
// are relative to the crate's directory, where the program runs and where `cargo test` runs: a path is part of each
// rule's source and of the records.
//
// Field encoding: see health-oracle.h.

#include "health-oracle.h"

static FILE *records;
static char capture_path[4096];
static int saved_stderr = -1;

static void capture_begin(void) {
    fflush(stderr);
    saved_stderr = dup(STDERR_FILENO);
    int fd = open(capture_path, O_CREAT | O_TRUNC | O_WRONLY, 0600);
    if(saved_stderr == -1 || fd == -1 || dup2(fd, STDERR_FILENO) == -1) {
        perror("capture");
        exit(1);
    }
    close(fd);
}

// the value of a logfmt field written as <name>="...": NULL when the line has none
static char *quoted_field(char *line, const char *name, bool to_line_end) {
    char *start = strstr(line, name);
    if(!start)
        return NULL;
    start += strlen(name);

    char *end;
    if(to_line_end) {
        end = strrchr(start, '"');
        if(!end || end < start)
            return NULL;
    }
    else {
        end = start;
        while(*end && !(*end == '"' && end[-1] != '\\'))
            end++;
        if(!*end)
            return NULL;
    }
    *end = '\0';
    return start;
}

static void capture_end(const char *item) {
    fflush(stderr);
    dup2(saved_stderr, STDERR_FILENO);
    close(saved_stderr);

    FILE *fp = fopen(capture_path, "r");
    if(!fp) {
        perror(capture_path);
        exit(1);
    }

    char *line = NULL;
    size_t size = 0;
    ssize_t len;
    while((len = getline(&line, &size, fp)) > 0) {
        if(line[len - 1] == '\n')
            line[len - 1] = '\0';
        if(strncmp(line, "time=", 5) != 0)
            continue;

        // the message is last on the line; cut it off before looking for the fields in front of it
        char *msg = strstr(line, " msg=\"");
        if(!msg) {
            fprintf(stderr, "oracle: a record without a message in %s: %s\n", item, line);
            exit(1);
        }
        *msg = '\0';
        msg = quoted_field(msg + 1, "msg=\"", true);

        char *level = strstr(line, " level=");
        char *errno_text = quoted_field(line, " errno=\"", false);
        if(!level || !msg) {
            fprintf(stderr, "oracle: a record this program cannot read in %s\n", item);
            exit(1);
        }
        level += strlen(" level=");
        level[strcspn(level, " ")] = '\0';

        oracle_esc(records, item);
        fputc('\t', records);
        oracle_esc(records, level);
        fputc('\t', records);
        oracle_esc(records, errno_text ? errno_text : "-");
        fputc('\t', records);
        oracle_esc(records, msg);
        fputc('\n', records);
    }
    free(line);
    fclose(fp);
}

static void read_file(const char *item, const char *path, bool stock) {
    int rc = health_readfile(path, NULL, stock);
    oracle_esc(oracle_rules, item);
    fputs("\tfile\t", oracle_rules);
    oracle_esc(oracle_rules, path);
    fprintf(oracle_rules, "\t%d\t%d\n", stock ? 1 : 0, rc);
}

static void dump_store(const char *item) {
    RRD_ALERT_PROTOTYPE *ap;
    dfe_start_read(health_globals.prototypes.dict, ap) {
        size_t index = 0;
        for(RRD_ALERT_PROTOTYPE *t = ap; t; t = t->_internal.next, index++) {
            oracle_esc(oracle_rules, item);
            fputs("\tentry\t", oracle_rules);
            oracle_esc(oracle_rules, ap_dfe.name);
            fprintf(oracle_rules, "\t%zu\t%d\t%d\t", index, ap->_internal.enabled ? 1 : 0, t->match.enabled ? 1 : 0);
            oracle_esc(oracle_rules, t->config.exec ? string2str(t->config.exec) : NULL);
            fputc('\t', oracle_rules);
            oracle_esc(oracle_rules, t->config.recipient ? string2str(t->config.recipient) : NULL);
            fputc('\t', oracle_rules);
            for(size_t i = 0; i < sizeof(t->config.hash_id); i++)
                fprintf(oracle_rules, "%02x", (unsigned)t->config.hash_id[i]);
            fputc('\n', oracle_rules);
        }
    }
    dfe_done(ap);
}

static FILE *out_open(const char *dir, const char *name, const char *columns) {
    char path[4096];
    snprintf(path, sizeof(path), "%s/%s", dir, name);
    FILE *f = fopen(path, "w");
    if(!f) {
        fprintf(stderr, "cannot create %s\n", path);
        exit(1);
    }
    fprintf(f, "# generated by tests/oracle/gen-health-vectors.c from the C implementation; do not edit\n");
    fprintf(f, "# columns: %s\n", columns);
    return f;
}

int main(int argc, char **argv) {
    if(argc != 3) {
        fprintf(stderr, "usage: %s <output directory> <item list>\n", argv[0]);
        return 1;
    }

    // what C's own unit test initializes before it reads a file (health-config-unittest.c), and the two defaults
    // the daemon takes from [health]
    string_init();
    time_grouping_init();
    health_init_prototypes();
    health_globals.config.default_exec = string_strdupz("/oracle/plugins.d/alarm-notify.sh");
    health_globals.config.default_recipient = string_strdupz("root");

    oracle_rules = out_open(argv[1], "rules.tsv",
                            "item, then by kind: rule, name, is_template, enabled, on, host_labels, chart_labels, exec, "
                            "recipient, classification, component, type, source_type, source, units, summary, info, "
                            "update_every, action options, dimensions, time_group, time_group_condition, "
                            "time_group_value bits, dims_group, data_source, before, after, options, calc source, calc "
                            "parsed_as, warn source, warn parsed_as, crit source, crit parsed_as, delay up, delay down, "
                            "delay max, delay multiplier bits (float), custom repeat, warn repeat, crit repeat, hash, the "
                            "hashed JSON | file, path, stock, return value | entry, name, index in the chain, prototype "
                            "enabled, rule enabled, exec, recipient, hash (NULL strings are \\x00)");
    records = out_open(argv[1], "records.tsv", "item, level, errno (- for none), message (as logfmt printed them)");
    snprintf(capture_path, sizeof(capture_path), "%s/.stderr-capture", argv[1]);

    FILE *list = fopen(argv[2], "r");
    if(!list) {
        perror(argv[2]);
        return 1;
    }

    // an item is one line "<stock> <path>", or a line "group <name>" followed by its files as "+<stock> <path>"
    char line[8192];
    char item[8192] = "";
    bool open_item = false;
    while(fgets(line, sizeof(line), list)) {
        line[strcspn(line, "\n")] = '\0';
        if(!*line)
            continue;

        bool member = line[0] == '+';
        if(!member) {
            if(open_item) {
                dump_store(item);
                capture_end(item);
            }
            open_item = true;
            dictionary_flush(health_globals.prototypes.dict);
            if(strncmp(line, "group ", 6) == 0) {
                snprintf(item, sizeof(item), "%s", line + 6);
                oracle_item = item;
                capture_begin();
                continue;
            }
            snprintf(item, sizeof(item), "%s", line + 2);
            oracle_item = item;
            capture_begin();
        }

        const char *spec = member ? line + 1 : line;
        read_file(item, spec + 2, spec[0] == '1');
    }
    if(open_item) {
        dump_store(item);
        capture_end(item);
    }

    fclose(list);
    fclose(records);
    fclose(oracle_rules);
    unlink(capture_path);
    return 0;
}
