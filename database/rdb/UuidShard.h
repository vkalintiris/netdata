#ifndef RDB_UUID_SHARD_H
#define RDB_UUID_SHARD_H

#include "libnetdata/libnetdata.h"
#include "libnetdata/xxhash.h"

#include <atomic>
#include <mutex>
#include <vector>
#include <unordered_map>

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

template<typename T>
class UuidShard {
public:
    UuidShard(size_t shards = 0) {
        mutexes = std::vector<std::mutex>(shards);
        maps = std::vector<std::unordered_map<UUID, T *>>(shards);
    }

    T *create(const uuid_t &uuid) {
        T *v = new T();
        uuid_copy(v->uuid, uuid);
        v->id = ++max_reserved_id;
        v->rc = 1;

        size_t i = shard(uuid);
        {
            std::lock_guard<std::mutex> L(mutexes[i]);
            maps[i][UUID{ .inner = uuid }] = v;
        }

        return v; 
    }

    T *add_or_create(const uuid_t &uuid) {
        T *v = nullptr;

        size_t i = shard(uuid);
        {
            std::lock_guard<std::mutex> L(mutexes[i]);
            auto it = maps[i].find(UUID{ .inner = uuid });
            if (it != maps[i].cend()) {
                v= it->second;
                v->rc++;
            }
        }

        if (v)
            return v;
        else
            return create(uuid);
    }

    void acquire(T *v) {
        size_t i = shard(v->uuid);
        {
            std::lock_guard<std::mutex> L(mutexes[i]);
            v->rc++;
        }
    }

    T *acquire(const uuid_t &uuid) {
        size_t i = shard(uuid);
        {
            std::lock_guard<std::mutex> L(mutexes[i]);
            auto it = maps[i].find(UUID{ .inner = uuid });
            if (it == maps[i].cend())
                return nullptr;

            T *v = it->second;
            v->rc++;
            return v;
        }
    }

    T *acquire(uuid_t *uuid) {
        return acquire(*uuid);
    }

    void release(T *v) {
        size_t i = shard(v->uuid);
        {
            std::lock_guard<std::mutex> L(mutexes[i]);

            if (--v->rc == 0)
                delete v;
        }
    }

private:
    size_t shard(const uuid_t &uuid) {
        size_t h = std::hash<UUID>{}(UUID{ .inner = &uuid[0] });
        return h % maps.size();
    }

private:
    std::vector<std::mutex> mutexes;
    std::vector<std::unordered_map<UUID, T *>> maps;

    std::atomic<uint32_t> max_reserved_id;
};

#endif /* RDB_UUID_SHARD_H */
