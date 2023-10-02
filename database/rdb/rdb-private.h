#ifndef RDB_PRIVATE_H
#define RDB_PRIVATE_H

#include "rdb.h"

#include <atomic>
#include <mutex>
#include <unordered_map>
#include <map>
#include <vector>

struct rdb_metric_handle {
    uuid_t uuid;
    uint32_t id;
    uint32_t rc;
};

struct rdb_metrics {
    std::mutex mutex;
    std::vector<rdb_metric_handle *> values;
    uint32_t max_id;
};

struct rdb_metrics_group {
    uint32_t rc;
};

#endif /* RDB_PRIVATE_H */