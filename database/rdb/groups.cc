#include "rdb-private.h"
#include "uuid_shard.h"

static class UuidShard<rdb_metrics_group> GroupsRegistry(24);

STORAGE_METRICS_GROUP *rdb_metrics_group_get(STORAGE_INSTANCE *si, uuid_t *uuid) {
    UNUSED(si);

    rdb_metrics_group *rmg = GroupsRegistry.create(*uuid);
    return reinterpret_cast<STORAGE_METRICS_GROUP *>(rmg);
}

void rdb_metrics_group_release(STORAGE_INSTANCE *si, STORAGE_METRICS_GROUP *smg) {
    UNUSED(si);

    rdb_metrics_group *rmg = reinterpret_cast<rdb_metrics_group *>(smg);
    GroupsRegistry.release(rmg);
}
