// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef NETDATA_UUID_H
#define NETDATA_UUID_H

// for compatibility with libuuid
typedef unsigned char nd_uuid_t[16];

// for quickly managing it as 2x 64-bit numbers
typedef struct _uuid {
    union {
        nd_uuid_t uuid;
        struct {
            uint64_t hig64;
            uint64_t low64;
        } parts;
    };
} ND_UUID;
ND_UUID UUID_generate_from_hash(const void *payload, size_t payload_len);

#define UUIDeq(a, b) ((a).parts.hig64 == (b).parts.hig64 && (a).parts.low64 == (b).parts.low64)

static inline ND_UUID uuid2UUID(uuid_t uu1) {
    ND_UUID *ret = (ND_UUID *)uu1;
    return *ret;
}

#ifndef UUID_STR_LEN
// CentOS 7 has older version that doesn't define this
// same goes for MacOS
#define UUID_STR_LEN	37
#endif

#define UUID_COMPACT_STR_LEN 33

void uuid_unparse_lower_compact(const nd_uuid_t uuid, char *out);
int uuid_parse_compact(const char *in, nd_uuid_t uuid);

int uuid_parse_flexi(const char *in, nd_uuid_t uuid);
#define uuid_parse(in, uuid) uuid_parse_flexi(in, uuid)

static inline int hex_char_to_int(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1; // Invalid hexadecimal character
}

static inline void nd_uuid_clear(nd_uuid_t uu) {
    memset(uu, 0, sizeof(nd_uuid_t));
}

// Netdata does not need to sort UUIDs lexicographically and this kind
// of sorting does not need to be portable between little/big endian.
// So, any kind of sorting will work, as long as it compares UUIDs.
// The fastest possible, is good enough.
static inline int nd_uuid_compare(const nd_uuid_t uu1, const nd_uuid_t uu2) {
    // IMPORTANT:
    // uu1 or uu2 may not be aligned to word boundaries on this call,
    // so casting this to a struct may give SIGBUS on some architectures.
    return memcmp(uu1, uu2, sizeof(nd_uuid_t));
}

static inline void nd_uuid_copy(nd_uuid_t dst, const nd_uuid_t src) {
    memcpy(dst, src, sizeof(nd_uuid_t));
}

static inline bool nd_uuid_eq(const nd_uuid_t uu1, const nd_uuid_t uu2) {
    return nd_uuid_compare(uu1, uu2) == 0;
}

static inline int nd_uuid_is_null(const nd_uuid_t uu) {
    return nd_uuid_compare(uu, UUID_ZERO.uuid) == 0;
}

void nd_uuid_unparse_lower(const nd_uuid_t uuid, char *out);
void nd_uuid_unparse_upper(const nd_uuid_t uuid, char *out);

#define uuid_is_null(uu) nd_uuid_is_null(uu)
#define uuid_clear(uu) nd_uuid_clear(uu)
#define uuid_compare(uu1, uu2) nd_uuid_compare(uu1, uu2)
#define uuid_copy(dst, src) nd_uuid_copy(dst, src)
#define uuid_eq(uu1, uu2) nd_uuid_eq(uu1, uu2)

#define uuid_generate(out) os_uuid_generate(out)
#define uuid_generate_random(out) os_uuid_generate_random(out)
#define uuid_generate_time(out) os_uuid_generate_time(out)

#define uuid_unparse(uu, out) nd_uuid_unparse_lower(uu, out)
#define uuid_unparse_lower(uu, out) nd_uuid_unparse_lower(uu, out)
#define uuid_unparse_upper(uu, out) nd_uuid_unparse_upper(uu, out)

#endif //NETDATA_UUID_H
