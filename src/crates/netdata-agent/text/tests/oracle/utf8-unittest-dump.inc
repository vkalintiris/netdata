// Inserted by gen-vectors.sh before run_sanitize_test() of a copy of
// src/libnetdata/sanitizers/utf8-sanitizer-unittest.c: prints every
// sanitize_test_t case (with its hand-written expectations) as a Rust table row.
// NULL strings are written as empty ones: text_sanitize() treats both alike.

static void dump_bytes(const char *s) {
    fputs("b\"", stdout);
    for(const unsigned char *p = (const unsigned char *)(s ? s : ""); *p; p++) {
        if(*p >= 0x20 && *p <= 0x7e && *p != '"' && *p != '\\')
            fputc(*p, stdout);
        else
            fprintf(stdout, "\\x%02x", *p);
    }
    fputc('"', stdout);
}

static void dump_case(const sanitize_test_t *t) {
    const char *map = "Unknown";
    if(t->char_map == identity_char_map) map = "Identity";
    else if(t->char_map == test_rrd_char_map) map = "TestRrd";
    else if(t->char_map == rrd_string_allowed_chars) map = "RrdString";

    fputs("    Case { name: \"", stdout);
    fputs(t->name, stdout);
    fputs("\", input: ", stdout);
    dump_bytes((const char *)t->input);
    fprintf(stdout, ", dst_size: %zu, map: Map::%s, utf: %s, empty: ", t->dst_size, map, t->utf ? "true" : "false");
    dump_bytes(t->empty);
    fputs(", expected: ", stdout);
    dump_bytes(t->expected_output);
    fprintf(stdout, ", len: %zu, mblen: %zu },\n", t->expected_len, t->expected_mblen);
}

