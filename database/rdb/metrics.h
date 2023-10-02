#ifndef RDB_METRICS_H
#define RDB_METRICS_H

#include "rdb-private.h"

class Metrics {
public:
    Metrics(size_t shards) {
        mutexes = std::vector<std::mutex>(shards);
        maps = std::vector<std::unordered_map<UUID, rdb_metric_handle *>>(shards);
    }

    rdb_metric_handle *create(const uuid_t &uuid) {
        rdb_metric_handle *rmh = new rdb_metric_handle();
        uuid_copy(rmh->uuid, uuid);
        rmh->id = ++max_reserved_id;
        rmh->rc = 1;

        size_t i = shard(uuid);
        {
            std::lock_guard<std::mutex> L(mutexes[i]);
            maps[i][UUID{ .inner = uuid }] = rmh;
        }

        return rmh; 
    }

    rdb_metric_handle *add_or_create(const uuid_t &uuid) {
        rdb_metric_handle *rmh = nullptr;

        size_t i = shard(uuid);
        {
            std::lock_guard<std::mutex> L(mutexes[i]);
            auto it = maps[i].find(UUID{ .inner = uuid });
            if (it != maps[i].cend()) {
                rmh = it->second;
                rmh->rc++;
            }
        }

        if (rmh)
            return rmh;
        else
            return create(uuid);
    }

    void acquire(rdb_metric_handle *rmh) {
        size_t i = shard(rmh->uuid);
        {
            std::lock_guard<std::mutex> L(mutexes[i]);
            rmh->rc++;
        }
    }

    rdb_metric_handle *acquire(const uuid_t &uuid) {
        size_t i = shard(uuid);
        {
            std::lock_guard<std::mutex> L(mutexes[i]);
            auto *rmh = maps[i][UUID{ .inner = uuid }];
            rmh->rc++;
            return rmh;
        }
    }

    rdb_metric_handle *acquire(uuid_t *uuid) {
        return acquire(*uuid);
    }

    void release(rdb_metric_handle *rmh) {
        size_t i = shard(rmh->uuid);
        {
            std::lock_guard<std::mutex> L(mutexes[i]);

            if (--rmh->rc == 0)
                delete rmh;
        }
    }

private:
    size_t shard(const uuid_t &uuid) {
        size_t h = std::hash<UUID>{}(UUID{ .inner = &uuid[0] });
        return h % maps.size();
        return 10;
    }

private:
    std::vector<std::mutex> mutexes;
    std::vector<std::unordered_map<UUID, rdb_metric_handle *>> maps;

    std::atomic<uint32_t> max_reserved_id;
};

#endif /* RDB_METRICS_H */
