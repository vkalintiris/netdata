// SPDX-License-Identifier: GPL-3.0-or-later
//
// Runs one inicfg script against the C implementation and prints what every step returns, so the Rust port can be
// compared against it (tests/oracle.rs replays the same scripts). Usage: inicfg-oracle SCRIPT FIXTURES_DIR
//
// A script has one step per line, fields separated by TAB; empty lines and lines starting with '#' are skipped.
// "\N" as a default means NULL. Doubles are printed as their IEEE-754 bits so both sides compare exactly.

#include "libnetdata/libnetdata.h"

#define MAX_FIELDS 8

// Internal to libnetdata (inicfg_internals.h) but linked in.
void inicfg_section_destroy_non_loaded(struct config *root, const char *section);
void inicfg_section_option_destroy_non_loaded(struct config *root, const char *section, const char *name);

static const char *arg(char **f, int i) {
    return strcmp(f[i], "\\N") == 0 ? NULL : f[i];
}

static void print_double(double v) {
    uint64_t bits;
    memcpy(&bits, &v, sizeof(bits));
    printf("%016" PRIx64 "\n", bits);
}

static double parse_double_bits(const char *s) {
    uint64_t bits = strtoull(s, NULL, 16);
    double v;
    memcpy(&v, &bits, sizeof(v));
    return v;
}

int main(int argc, char **argv) {
    if(argc != 3) {
        fprintf(stderr, "usage: %s SCRIPT FIXTURES_DIR\n", argv[0]);
        return 1;
    }
    FILE *fp = fopen(argv[1], "r");
    if(!fp) {
        perror(argv[1]);
        return 1;
    }

    struct config cfg = APPCONFIG_INITIALIZER;
    char line[65536];
    while(fgets(line, sizeof(line), fp)) {
        line[strcspn(line, "\n")] = '\0';
        if(!line[0] || line[0] == '#')
            continue;

        char *f[MAX_FIELDS] = { 0 };
        int n = 0;
        for(char *p = line, *tok; n < MAX_FIELDS && (tok = strsep(&p, "\t")); )
            f[n++] = tok;
        for(int i = n; i < MAX_FIELDS; i++)
            f[i] = "";

        const char *op = f[0];
        printf("%s", op);
        for(int i = 1; i < n; i++)
            printf("\t%s", f[i]);
        printf(" -> ");

        if(!strcmp(op, "load")) {
            char path[FILENAME_MAX];
            snprintf(path, sizeof(path), "%s/%s", argv[2], f[1]);
            printf("%d\n", inicfg_load(&cfg, path, atoi(f[2]), arg(f, 3)));
        }
        else if(!strcmp(op, "get")) {
            const char *v = inicfg_get(&cfg, f[1], f[2], arg(f, 3));
            printf("%s\n", v ? v : "(null)");
        }
        else if(!strcmp(op, "set"))
            printf("%s\n", inicfg_set(&cfg, f[1], f[2], f[3]));
        else if(!strcmp(op, "get_filename")) {
            const char *v = inicfg_get_filename(&cfg, f[1], f[2], arg(f, 3));
            printf("%s\n", v ? v : "(null)");
        }
        else if(!strcmp(op, "get_path")) {
            const char *v = inicfg_get_path(&cfg, f[1], f[2], arg(f, 3));
            printf("%s\n", v ? v : "(null)");
        }
        else if(!strcmp(op, "get_boolean"))
            printf("%d\n", inicfg_get_boolean(&cfg, f[1], f[2], atoi(f[3])));
        else if(!strcmp(op, "get_boolean_ondemand"))
            printf("%d\n", inicfg_get_boolean_ondemand(&cfg, f[1], f[2], atoi(f[3])));
        else if(!strcmp(op, "set_boolean"))
            printf("%d\n", inicfg_set_boolean(&cfg, f[1], f[2], atoi(f[3])));
        else if(!strcmp(op, "get_number"))
            printf("%lld\n", inicfg_get_number(&cfg, f[1], f[2], strtoll(f[3], NULL, 10)));
        else if(!strcmp(op, "get_number_range"))
            printf("%lld\n", inicfg_get_number_range(&cfg, f[1], f[2], strtoll(f[3], NULL, 10),
                                                     strtoll(f[4], NULL, 10), strtoll(f[5], NULL, 10)));
        else if(!strcmp(op, "set_number"))
            printf("%lld\n", inicfg_set_number(&cfg, f[1], f[2], strtoll(f[3], NULL, 10)));
        else if(!strcmp(op, "get_double"))
            print_double(inicfg_get_double(&cfg, f[1], f[2], parse_double_bits(f[3])));
        else if(!strcmp(op, "set_double"))
            print_double(inicfg_set_double(&cfg, f[1], f[2], parse_double_bits(f[3])));
        else if(!strcmp(op, "get_duration_seconds"))
            printf("%lld\n", (long long)inicfg_get_duration_seconds(&cfg, f[1], f[2], strtoll(f[3], NULL, 10)));
        else if(!strcmp(op, "set_duration_seconds"))
            printf("%lld\n", (long long)inicfg_set_duration_seconds(&cfg, f[1], f[2], strtoll(f[3], NULL, 10)));
        else if(!strcmp(op, "get_duration_ms"))
            printf("%llu\n", (unsigned long long)inicfg_get_duration_ms(&cfg, f[1], f[2], strtoull(f[3], NULL, 10)));
        else if(!strcmp(op, "set_duration_ms"))
            printf("%llu\n", (unsigned long long)inicfg_set_duration_ms(&cfg, f[1], f[2], strtoull(f[3], NULL, 10)));
        else if(!strcmp(op, "get_duration_days_to_seconds"))
            printf("%lld\n", (long long)inicfg_get_duration_days_to_seconds(&cfg, f[1], f[2], (unsigned)strtoul(f[3], NULL, 10)));
        else if(!strcmp(op, "get_size_bytes"))
            printf("%" PRIu64 "\n", inicfg_get_size_bytes(&cfg, f[1], f[2], strtoull(f[3], NULL, 10)));
        else if(!strcmp(op, "set_size_bytes"))
            printf("%" PRIu64 "\n", inicfg_set_size_bytes(&cfg, f[1], f[2], strtoull(f[3], NULL, 10)));
        else if(!strcmp(op, "get_size_mb"))
            printf("%" PRIu64 "\n", inicfg_get_size_mb(&cfg, f[1], f[2], strtoull(f[3], NULL, 10)));
        else if(!strcmp(op, "set_size_mb"))
            printf("%" PRIu64 "\n", inicfg_set_size_mb(&cfg, f[1], f[2], strtoull(f[3], NULL, 10)));
        else if(!strcmp(op, "exists"))
            printf("%d\n", inicfg_exists(&cfg, f[1], f[2]));
        else if(!strcmp(op, "set_default_raw_value")) {
            inicfg_set_default_raw_value(&cfg, f[1], f[2], f[3]);
            printf("ok\n");
        }
        else if(!strcmp(op, "move"))
            printf("%d\n", inicfg_move(&cfg, f[1], f[2], f[3], f[4]) == 0);
        else if(!strcmp(op, "move_everywhere"))
            printf("%d\n", inicfg_move_everywhere(&cfg, f[1], f[2]) == 0);
        else if(!strcmp(op, "destroy_section")) {
            inicfg_section_destroy_non_loaded(&cfg, f[1]);
            printf("ok\n");
        }
        else if(!strcmp(op, "destroy_option")) {
            inicfg_section_option_destroy_non_loaded(&cfg, f[1], f[2]);
            printf("ok\n");
        }
        else if(!strcmp(op, "stream_needs_dbengine"))
            printf("%d\n", stream_conf_needs_dbengine(&cfg));
        else if(!strcmp(op, "stream_has_api"))
            printf("%d\n", stream_conf_has_api_enabled(&cfg));
        else if(!strcmp(op, "generate")) {
            BUFFER *wb = buffer_create(0, NULL);
            inicfg_generate(&cfg, wb, atoi(f[1]), atoi(f[2]));
            printf("\n%s<<<END\n", buffer_tostring(wb));
            buffer_free(wb);
        }
        else {
            fprintf(stderr, "unknown step '%s'\n", op);
            return 1;
        }
    }
    fclose(fp);
    return 0;
}
