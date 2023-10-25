#ifndef RDB_PRIVATE_H
#define RDB_PRIVATE_H

#include "rdb-common.h"
#include "Key.h"
#include "Page.h"
#include "CollectionPage.h"
#include "StorageInstance.h"
#include "CollectionHandle.h"
#include "FlushedQueryHandle.h"

#include <cstdint>

class MetricHandle
{
public:
    MetricHandle(uint32_t gid, uint32_t mid) : gid(gid), mid(mid) {}

    uint32_t groupID() const {
        return gid;
    }

    uint32_t metricID() const {
        return mid;
    }

private:
    uint32_t gid;
    uint32_t mid;
};

#endif /* RDB_PRIVATE_H */
