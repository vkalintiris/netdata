#ifndef UUID_UTILS_H
#define UUID_UTILS_H

#include "libnetdata/libnetdata.h"
#include "libnetdata/xxhash.h"
#include <functional>

struct UUID {
    const unsigned char *inner;

    bool operator==(const UUID &other) const {
        return uuid_compare(inner, other.inner) == 0;
    }
};

namespace std {
    template<> struct hash<UUID> {
        auto operator()(const UUID &uuid) const -> size_t {
            // I suspect we can just pick 4-bytes from the uuid
            return XXH32(uuid.inner, 16, 0);
        }
    };
}

#endif /* UUID_UTILS_H */
