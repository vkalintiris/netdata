#include "rdb-private.h"
#include "si.h"

using namespace rocksdb;

// TODO: this will iterate _ALL_ keys.
time_t rdb_global_first_time_s(STORAGE_INSTANCE *si) {
    UNUSED(si);

    char scratch[12];

    uint32_t gid = 0;
    uint32_t mid = 0;
    uint32_t pit = 0;
    
    const Slice StartK = rdb_collection_key_serialize(scratch, gid, mid, pit);

    Iterator *it = RDB->NewIterator(ReadOptions());
    uint32_t first_pit = ~0u;
    for (it->Seek(StartK); it->Valid(); it->Next()) {
        const Slice &K = it->key();
        rdb_collection_key_deserialize(K, gid, mid, pit);
        netdata_log_error("gid=%u, mid=%u, pit=%u", gid, mid, pit);
        first_pit = std::min(first_pit, pit);
    }

    return first_pit;
}

time_t rdb_metric_oldest_time(STORAGE_METRIC_HANDLE *smh) {
    rdb_metric_handle *rmh = reinterpret_cast<rdb_metric_handle *>(smh);

    char scratch[12];

    uint32_t gid = rmh->gid;
    uint32_t mid = rmh->id;
    uint32_t pit = 0;
    
    const Slice StartK = rdb_collection_key_serialize(scratch, gid, mid, pit);

    Iterator *it = RDB->NewIterator(ReadOptions());
    for (it->Seek(StartK); it->Valid(); it->Next()) {
        const Slice &K = it->key();

        rdb_collection_key_deserialize(K, gid, mid, pit);
        return pit;
    }

    return 0;
}
