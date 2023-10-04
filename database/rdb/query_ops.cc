#include "rdb-private.h"
#include "si.h"

using rocksdb::Slice;
using rocksdb::Iterator;
using rocksdb::ReadOptions;

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
