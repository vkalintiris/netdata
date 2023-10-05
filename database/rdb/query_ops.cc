#include "rdb-private.h"
#include "si.h"

using rocksdb::Slice;
using rocksdb::Iterator;
using rocksdb::ReadOptions;

using namespace rocksdb;

// TODO: this will iterate _ALL_ keys.
time_t rdb_global_first_time_s(STORAGE_INSTANCE *si) {
    UNUSED(si);

    netdata_log_error("Expensive operation: %s()", __func__);

    char scratch[12];

    uint32_t gid = 0;
    uint32_t mid = 0;
    uint32_t pit = 0;
    
    const Slice StartK = rdb_collection_key_serialize(scratch, gid, mid, pit);

    Iterator *it = SI->RDB->NewIterator(ReadOptions());
    uint32_t first_pit = ~0u;
    for (it->Seek(StartK); it->Valid(); it->Next()) {
        const Slice &K = it->key();
        rdb_collection_key_deserialize(K, gid, mid, pit);
        netdata_log_error("gid=%u, mid=%u, pit=%u", gid, mid, pit);
        first_pit = std::min(first_pit, pit);
    }

    return first_pit;
}

uint64_t rdb_disk_space_used(STORAGE_INSTANCE *si) {
    UNUSED(si);
    
    std::array<Range, 1> ranges;
    std::array<uint64_t, 1> sizes;
    SizeApproximationOptions Opts;

    Opts.include_memtables = false;
    Opts.files_size_error_margin = 0.1;

    char StartBuf[12];
    const Slice &StartK = rdb_collection_key_serialize(StartBuf, 0, 0, 0);

    char LimitBuf[12];
    const Slice &LimitK = rdb_collection_key_serialize(LimitBuf,
        std::numeric_limits<uint32_t>::max(),
        std::numeric_limits<uint32_t>::max(),
        std::numeric_limits<uint32_t>::max()
    );

    ranges[0].start = StartK;
    ranges[0].limit = LimitK;

    Status S = SI->RDB->GetApproximateSizes(Opts, SI->RDB->DefaultColumnFamily(), ranges.data(), ranges.size(), sizes.data());
    if (!S.ok()) {
        netdata_log_error("Could not get approximate size for default CF: %s", S.ToString().c_str());
        return 0;
    }

    return sizes[0];
}
