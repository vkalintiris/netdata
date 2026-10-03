// SPDX-License-Identifier: GPL-3.0-or-later
//
// Golden vectors of `bearer_get_token`'s payload (netdata-agent-daemon, `builtins/bearer_get_token.rs`): each input
// run through the production json_parse_function_payload_or_error() with the Function's member readers
// (function-bearer_get_token.c bearer_parse_json_payload(), copied: it is static there). An input is the payload as
// the Function receives it: a parent's FUNCTION_PAYLOAD lines, each followed by a newline; a few end without one.
//
// Output lines: `<input hex> <code> <hex>`: on an error C's whole reply body, else the request it read as
// `<claim> <guid> <node> <role> <access hex> <account> <client name hex, or - for none>`.

#include "libnetdata/libnetdata.h"

struct bearer_token_request {
    nd_uuid_t claim_id;
    nd_uuid_t machine_guid;
    nd_uuid_t node_id;
    HTTP_USER_ROLE user_role;
    HTTP_ACCESS access;
    nd_uuid_t cloud_account_id;
    STRING *client_name;
};

static bool bearer_parse_json_payload(json_object *jobj, void *data, BUFFER *error) {
    const char *path = "";
    struct bearer_token_request *rq = data;
    JSONC_PARSE_TXT2UUID_OR_ERROR_AND_RETURN(jobj, path, "claim_id", rq->claim_id, error, JSONC_REQUIRED);
    JSONC_PARSE_TXT2UUID_OR_ERROR_AND_RETURN(jobj, path, "machine_guid", rq->machine_guid, error, JSONC_REQUIRED);
    JSONC_PARSE_TXT2UUID_OR_ERROR_AND_RETURN(jobj, path, "node_id", rq->node_id, error, JSONC_REQUIRED);
    JSONC_PARSE_TXT2ENUM_OR_ERROR_AND_RETURN(jobj, path, "user_role", http_user_role2id, rq->user_role, error, JSONC_REQUIRED);
    JSONC_PARSE_ARRAY_OF_TXT2BITMAP_OR_ERROR_AND_RETURN(jobj, path, "access", http_access2id_one, rq->access, error, JSONC_REQUIRED);
    JSONC_PARSE_TXT2UUID_OR_ERROR_AND_RETURN(jobj, path, "cloud_account_id", rq->cloud_account_id, error, JSONC_REQUIRED);
    JSONC_PARSE_TXT2STRING_OR_ERROR_AND_RETURN(jobj, path, "client_name", rq->client_name, error, JSONC_REQUIRED);
    return true;
}

static FILE *out;

static void hex(const char *s, size_t len) {
    if(!len)
        fputc('-', out);
    for(size_t i = 0; i < len; i++)
        fprintf(out, "%02x", (unsigned char)s[i]);
}

static void run(const char *data, size_t len) {
    BUFFER *payload = buffer_create(0, NULL);
    buffer_contents_replace(payload, data, len);
    BUFFER *wb = buffer_create(0, NULL);
    struct bearer_token_request rq = { 0 };
    int code = 0;
    json_object *jobj =
        json_parse_function_payload_or_error(wb, len ? payload : NULL, &code, bearer_parse_json_payload, &rq);

    hex(data, len);
    fprintf(out, " %d ", code);
    if(jobj && code == HTTP_RESP_OK) {
        char claim[UUID_STR_LEN], guid[UUID_STR_LEN], node[UUID_STR_LEN], account[UUID_STR_LEN];
        uuid_unparse_lower(rq.claim_id, claim);
        uuid_unparse_lower(rq.machine_guid, guid);
        uuid_unparse_lower(rq.node_id, node);
        uuid_unparse_lower(rq.cloud_account_id, account);
        char line[1024];
        int n = snprintf(line, sizeof(line), "%s %s %s %s %llx %s ", claim, guid, node, http_id2user_role(rq.user_role),
                         (unsigned long long)rq.access, account);
        hex(line, n);
        if(rq.client_name)
            hex(string2str(rq.client_name), string_strlen(rq.client_name));
        else
            hex("-", 1);
        json_object_put(jobj);
    }
    else
        hex(buffer_tostring(wb), buffer_strlen(wb));
    fputc('\n', out);

    string_freez(rq.client_name);
    buffer_free(wb);
    buffer_free(payload);
}

static void run_str(const char *s) {
    run(s, strlen(s));
}

#define CLAIM "5a1e0000-0000-4000-8000-0000000000cc"
#define GUID "5a1e0000-0000-4000-8000-0000000000aa"
#define NODE "5a1e0000-0000-4000-8000-0000000000bb"
#define ACCOUNT "5a1e0000-0000-4000-8000-0000000000e3"

// a whole request with the members after node_id given as `rest`
static void request(const char *claim, const char *rest) {
    char s[4096];
    snprintf(s, sizeof(s), "{\"claim_id\":%s,\"machine_guid\":\"" GUID "\",\"node_id\":\"" NODE "\",%s}\n", claim, rest);
    run_str(s);
}

static void nested(size_t depth, const char *inner, bool close) {
    char s[1024] = "";
    for(size_t i = 0; i < depth; i++)
        strcat(s, "[");
    strcat(s, inner);
    for(size_t i = 0; close && i < depth; i++)
        strcat(s, "]");
    strcat(s, "\n");
    run_str(s);
}

int main(int argc, char **argv) {
    if(argc != 2) {
        fprintf(stderr, "usage: %s <output file>\n", argv[0]);
        return 1;
    }
    out = fopen(argv[1], "w");
    if(!out) {
        perror(argv[1]);
        return 1;
    }

    // no payload, and json-c's own texts
    run("", 0);
    const char *tokener[] = {
        " ", " \n", "{", "{\n", "{\"a\":\"x", "{\"a\":\"x\n", "[\n", "[1,\n", "{\"a\"\n", "{\"a\":\n", "{\"a\":tr\n",
        "x\n", "\xff\n", "]\n", "}\n", ":\n", ",\n", "{\"a\":}\n", "[,]\n", "{,}\n", "{1:2}\n", "{a:1}\n",
        "{\"a\":tru}\n", "{\"a\":tx}\n", "{\"a\":fals}\n", "{\"a\":nul}\n", "{\"a\":nx}\n", "{\"a\":na}\n",
        "{\"a\":True}\n", "{\"a\":Nul}\n", "{\"a\":Tx}\n",
        "{\"a\" 1}\n", "{\"a\",1}\n", "[1 2]\n", "[1:2]\n", "{\"a\":1 \"b\":2}\n", "{\"a\":1]\n",
        "{\"a\":\"\x01\"}\n", "{\"a\":\"\x1f\"}\n", "{\"a\":\"\t\"}\n", "{\"a\":\"\\u12\"}\n", "{\"a\":\"\\q\"}\n",
        "{\"a\":\"\\ud800\"}\n", "{\"a\":\"\\ud800x\"}\n", "{\"a\":\"\\udc00\"}\n", "{\"a\":\"\\u0000\"}\n",
        "{\"a\":\"\xff\"}\n", "{\"a\":\"\xc3\"}\n",
        "{\"a\":-}\n", "{\"a\":-x}\n", "{\"a\":+1}\n", "{\"a\":.5}\n", "{\"a\":1.}\n", "{\"a\":1.5e+}\n", "{\"a\":01}\n",
        "{\"a\":1e}\n", "{\"a\":1e999}\n", "{\"a\":99999999999999999999999}\n", "{\"a\":NaN}\n", "{\"a\":Infinity}\n",
        "{\"a\":-Infinity}\n", "{\"a\":nan}\n",
        "{'a':1}\n", "/*c*/{}\n", "//c\n{}\n", "{\"a\":1}garbage\n", "{\"a\":1}\n{\n", "null\n", "\"str\"\n", "[1]\n",
        "123\n", "123x\n", "truex\n", "{\"a\":1,}\n", "[1,]\n", "{\n\"claim_id\":\n\"x\"\n}\n",
        // a value the text ends in: json-c reads one byte past numbers and literals
        "123", "-1", "1.5", "true", "false", "null", "\"s\"", "{}", "[]",
    };
    for(size_t i = 0; i < sizeof(tokener) / sizeof(*tokener); i++)
        run_str(tokener[i]);

    // json-c's depth: a value at level 33 (the root is 1)
    nested(32, "", true);
    nested(33, "", true);
    nested(31, "1", true);
    nested(32, "1", true);
    nested(33, "", false);
    nested(32, "x", false);
    nested(31, "{\"a\":1}", true);
    nested(31, "{\"a\":[]}", true);
    nested(31, "{\"a\":[1]}", true);
    nested(31, "{\"a\":", false);
    nested(130, "", true);

    // json-c's depth checked while parsing: a repeated key's earlier, deeper value still counts
    run_str("{\"x\":[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[[]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]]],\"x\":1}\n");
    // a raw NUL: json-c stops there as at the end of the data
    const struct { const char *s; size_t len; } nul[] = {
        { "\0{}\n", 4 }, { "{\"a\":1,\0}\n", 10 }, { "123\0\n", 5 }, { "{\"a\":1}\0x\n", 10 },
        { "{\"a\":\"x\0\"}\n", 11 }, { "{\"a\":tr\0\n", 9 }, { "\0", 1 },
    };
    for(size_t i = 0; i < sizeof(nul) / sizeof(*nul); i++)
        run(nul[i].s, nul[i].len);
    // comments and literals json-c reads past
    const char *extensions[] = {
        "{\"a\":1 /x}\n", "{\"a\":/x}\n", "{\"a\":1 /*c*/}\n", "{\"a\":1 /", "{\"a\":True\n", "{\"a\":NULL]\n",
        "{\"a\":[1,]\n",
    };
    for(size_t i = 0; i < sizeof(extensions) / sizeof(*extensions); i++)
        run_str(extensions[i]);

    // the member readers' texts, in C's order
    const char *members[] = {
        "{}\n",
        "{\"claim_id\":null}\n",
        "{\"claim_id\":5}\n",
        "{\"claim_id\":\"not-a-uuid\"}\n",
        "{\"claim_id\":{}}\n",
        "{\"claim_id\":\"" CLAIM "\",\"machine_guid\":\"" GUID "\"}\n",
    };
    for(size_t i = 0; i < sizeof(members) / sizeof(*members); i++)
        run_str(members[i]);
    request("\"" CLAIM "\"", "\"user_role\":5");
    request("\"" CLAIM "\"", "\"user_role\":null");
    request("\"" CLAIM "\"", "\"user_role\":\"admin\"");
    request("\"" CLAIM "\"", "\"user_role\":\"admin\",\"access\":\"x\"");
    request("\"" CLAIM "\"", "\"user_role\":\"admin\",\"access\":[1]");
    request("\"" CLAIM "\"", "\"user_role\":\"admin\",\"access\":[\"signed-in\",null]");
    request("\"" CLAIM "\"", "\"user_role\":\"admin\",\"access\":[\"none\"],\"client_name\":{}");
    request("\"" CLAIM "\"", "\"user_role\":\"admin\",\"access\":[\"signed-in\",\"bogus\"]");
    request("\"" CLAIM "\"", "\"user_role\":\"admin\",\"access\":[\"signed-in\"],\"cloud_account_id\":\"" ACCOUNT "\"");
    request("\"" CLAIM "\"",
            "\"user_role\":\"admin\",\"access\":[\"signed-in\"],\"cloud_account_id\":\"" ACCOUNT "\",\"client_name\":[]");
    // twelve unknown names: the text cut to 255 bytes; then a cut inside a multi-byte character
    request("\"" CLAIM "\"",
            "\"user_role\":\"admin\",\"access\":[\"unknown-a\",\"unknown-b\",\"unknown-c\",\"unknown-d\",\"unknown-e\","
            "\"unknown-f\",\"unknown-g\",\"unknown-h\",\"unknown-i\",\"unknown-j\",\"unknown-k\",\"unknown-l\"]");
    for(size_t pad = 0; pad < 4; pad++) {
        char rest[1024] = "\"user_role\":\"admin\",\"access\":[\"";
        strncat(rest, "xxxx", pad);
        for(size_t i = 0; i < 120; i++)
            strcat(rest, "\xce\xbb");
        strcat(rest, "\"]");
        request("\"" CLAIM "\"", rest);
    }

    // what the readers accept
    const char *whole = "\"user_role\":\"admin\",\"access\":[\"signed-in\",\"same-space\",\"anonymous-data\",\"sensitive-data\"],"
                        "\"cloud_account_id\":\"" ACCOUNT "\",\"client_name\":\"parity-cloud\"";
    request("\"" CLAIM "\"", whole);
    request("\"5A1E0000-0000-4000-8000-0000000000CC\"", whole);
    request("\"5a1e00000000400080000000000000cc\"", whole);
    request("\"" CLAIM "junk\"", whole);
    request("\"5a1e0000-00004000-8000-0000000000cc\"", whole);
    request("null", whole);
    const char *rests[] = {
        "\"user_role\":\"king\",\"access\":[],\"cloud_account_id\":null,\"client_name\":\"c\"",
        "\"user_role\":\"\",\"access\":[],\"cloud_account_id\":null,\"client_name\":\"\"",
        "\"user_role\":\"any\",\"access\":[\"view-config\",\"edit-config\"],\"cloud_account_id\":null,\"client_name\":null",
        "\"user_role\":\"members\",\"access\":[\"signed-in\",\"signed-in\"],\"cloud_account_id\":null,\"client_name\":5",
        "\"user_role\":\"member\",\"access\":[],\"cloud_account_id\":null,\"client_name\":-5",
        "\"user_role\":\"member\",\"access\":[],\"cloud_account_id\":null,\"client_name\":1.5",
        "\"user_role\":\"member\",\"access\":[],\"cloud_account_id\":null,\"client_name\":true",
        "\"user_role\":\"member\",\"access\":[],\"cloud_account_id\":null,\"client_name\":\"a\\u0000b\"",
        "\"user_role\":\"member\",\"access\":[],\"cloud_account_id\":null,"
        "\"client_name\":\"0123456789012345678901234567890123456789012345678901234567890123456789\"",
        "\"user_role\":\"Admin\",\"access\":[\"Signed-In\"],\"cloud_account_id\":null,\"client_name\":\"c\",\"extra\":1",
        "\"user_role\":\"admin\",\"user_role\":\"observer\",\"access\":[],\"cloud_account_id\":null,\"client_name\":\"c\"",
    };
    for(size_t i = 0; i < sizeof(rests) / sizeof(*rests); i++)
        request("\"" CLAIM "\"", rests[i]);
    // json-c cuts an object's key at a NUL: a later member of the cut name replaces the earlier one
    char rest[1024];
    snprintf(rest, sizeof(rest), "%s,\"claim_id\\u0000\":\"x\"", whole);
    request("\"" CLAIM "\"", rest);
    request("\"" CLAIM "\"",
            "\"user_role\":\"admin\",\"user_role\\u0000x\":\"observer\",\"access\":[],\"cloud_account_id\":null,"
            "\"client_name\":\"c\"");

    if(fclose(out) != 0) {
        perror(argv[1]);
        return 1;
    }
    return 0;
}
