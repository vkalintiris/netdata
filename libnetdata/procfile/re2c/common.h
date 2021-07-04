#ifndef COMMON_H
#define COMMON_H

#include "libnetdata/libnetdata.h"

#if 0
inline uint32_t str2uint32_t(const char *s) {
    uint32_t n = 0;
    char c;
    for(c = *s; c >= '0' && c <= '9' ; c = *(++s)) {
        n *= 10;
        n += c - '0';
    }
    return n;
}

inline uint64_t str2uint64_t(const char *s) {
    uint64_t n = 0;
    char c;
    for(c = *s; c >= '0' && c <= '9' ; c = *(++s)) {
        n *= 10;
        n += c - '0';
    }
    return n;
}
#endif

#endif /* COMMON_H */
