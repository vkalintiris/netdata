#ifndef RDB_STORAGE_INSTANCE_H
#define RDB_STORAGE_INSTANCE_H

#include "database/rdb/Intervals.h"
#include "rdb-common.h"

#include "Key.h"
#include "MetricHandle.h"

struct rdb_collect_handle;

struct rdb_metric_handle
{
    uuid_t uuid;
    uint32_t id;
    uint32_t rc;

    uint32_t rmg;
    rdb_collect_handle *rch;

    rdb_metric_handle() :
        uuid{}, id{0}, rc{0}, rmg{0}, rch{nullptr}
    { }
};

class CustomCF : public rocksdb::CompactionFilter
{
public:
    rocksdb::CompactionFilter::Decision FilterBlobByKey(int Level, const rocksdb::Slice &SK, std::string *NewValue, std::string *SkipUntil) const override
    {
        UNUSED(Level);
        UNUSED(NewValue);
        UNUSED(SkipUntil);

        rdb::Key K = SK;
        uint32_t After = K.pit();

        uint32_t Epoch = 0x000000FF;
        uint32_t Diff = After - Epoch;
        uint32_t Cutoff = 24 * 3600;

        if (Diff >= Cutoff) {
            // netdata_log_error("Dropping key: %s", K.toString(true).c_str());
            return rocksdb::CompactionFilter::Decision::kRemove;
        }
        else
            return rocksdb::CompactionFilter::Decision::kKeep;
    }

    const char* Name() const override
    {
        return "CustomCF";
    }
};

namespace rdb {

class StorageInstance
{
public:
    [[nodiscard]] inline Status setIntervalManager(uint32_t GID, uint32_t MID, const IntervalManager<1024> &IM)
    {
        std::array<char, 4096> AR;
        auto OV = IM.serialize(AR);
        if (!OV) {
            return Status::InvalidArgument(Status::kNoSpace);
        }

        MetricKey MK(GID, MID);
        return putIM(MK.slice(), OV.value());
    }

    [[nodiscard]] inline std::optional<IntervalManager<1024>> getIntervalManager(uint32_t GID, uint32_t MID) const
    {
        using namespace rocksdb;

        MetricKey MK(GID, MID);
        PinnableSlice PV;

        Status S = RDB->Get(rocksdb::ReadOptions(), CFHs[2], MK.slice(), &PV);
        if (!S.ok())
        {
            // TODO: The Status check is very generic. We should check for
            // some subcode that denotes a missing metric key.
            return std::nullopt;
        }

        // TODO: verify copy-elision
        return IntervalManager<1024>::deserialize(PV);
    }

    [[nodiscard]] inline Status setUUIDtoIDs(const uuid_t &uuid, uint32_t GID, uint32_t MID)
    {
        using namespace rocksdb;

        Slice K(reinterpret_cast<const char *>(uuid), sizeof(uuid_t));
        MetricKey MK(GID, MID);

        Status S = putMH(K, MK.slice());
        if (!S.ok())
            return S;

        IntervalManager<1024> IM;
        return setIntervalManager(GID, MID, IM);
    }

    [[nodiscard]] inline std::optional<MetricHandle> getMetricHandleFromUUID(const uuid_t &uuid) const
    {
        using namespace rocksdb;

        Slice K(reinterpret_cast<const char *>(uuid), sizeof(uuid_t));
        PinnableSlice PV;

        Status S = RDB->Get(rocksdb::ReadOptions(), CFHs[2], K, &PV);
        if (!S.ok())
        {
            // TODO: The Status check is very generic. We should check for
            // some subcode that denotes a missing key.
            return std::nullopt;
        }

        MetricKey MK(PV);
        std::optional<IntervalManager<1024>> IM = getIntervalManager(MK.gid(), MK.mid());
        if (!IM.has_value())
            return std::nullopt;

        return MetricHandle(MK.gid(), MK.mid(), std::move(IM.value()));
    }

private:
    static rocksdb::ColumnFamilyOptions levelStyleOpts(size_t MemtableBudget)
    {
        using namespace rocksdb;

        ColumnFamilyOptions CFO = ColumnFamilyOptions();

        CFO.write_buffer_size = static_cast<size_t>(MemtableBudget / 4);

        // merge two memtables when flushing to L0
        CFO.min_write_buffer_number_to_merge = 2;

        // this means we'll use 50% extra memory in the worst case, but will reduce
        // write stalls.
        CFO.max_write_buffer_number = 6;

        // start flushing L0->L1 as soon as possible. each file on level0 is
        // (memtable_memory_budget / 2). This will flush level 0 when it's bigger than
        // memtable_memory_budget.
        CFO.level0_file_num_compaction_trigger = 2;

        // doesn't really matter much, but we don't want to create too many files
        CFO.target_file_size_base = MemtableBudget/ 8;

        // make L1 size equal to L0 size, so that L0 -> L1 compactions are fast
        CFO.max_bytes_for_level_base = MemtableBudget;

        // level style compaction
        CFO.compaction_style = kCompactionStyleLevel;

        // only compress levels >= 2
        CFO.compression_per_level.resize(CFO.num_levels);
        for (int i = 0; i < CFO.num_levels; ++i)
        {
            if (i == 0)
            {
                CFO.compression_per_level[i] = kLZ4Compression;
            }
            else
            {
                CFO.compression_per_level[i] = rocksdb::kZSTD;
            }
        }

        return CFO;
    }

    static rocksdb::ColumnFamilyOptions metricDataCFO(const rocksdb::CompactionStyle CS)
    {
        using namespace rocksdb;
        constexpr uint64_t MiB = 1024 * 1024;

        switch (CS)
        {
            case kCompactionStyleLevel:
            {
                ColumnFamilyOptions CFO = levelStyleOpts(256 * MiB);

                CFO.enable_blob_files = true;
                CFO.min_blob_size = 64;
                CFO.blob_compression_type = kZSTD;

                // CFO.compaction_filter = new CustomCF();

                return CFO;
            }
            default:
                fatal("Unsupported compaction style");
        }
    }

    static rocksdb::ColumnFamilyOptions metricHandleCFO(const rocksdb::CompactionStyle CS)
    {
        using namespace rocksdb;

        UNUSED(CS);

        ColumnFamilyOptions CFO;
        // CFO.merge_operator =
        return CFO;
    }

public:
    StorageInstance() : RDB(nullptr) { }

    rocksdb::Status open(rocksdb::Options Opts, const char *Path)
    {
        using namespace rocksdb;

        Opts.error_if_exists = false;
        Opts.create_if_missing = true;
        Opts.create_missing_column_families = true;

        ColumnFamilyOptions defaultCFO{};
        ColumnFamilyDescriptor defaultCFD(kDefaultColumnFamilyName, defaultCFO);

        ColumnFamilyOptions mdCFO = metricDataCFO(kCompactionStyleLevel);
        ColumnFamilyDescriptor mdCFD("md", mdCFO);

        ColumnFamilyOptions mhCFO{};
        ColumnFamilyDescriptor mhCFD("mh", mhCFO);

        ColumnFamilyOptions imCFO{};
        ColumnFamilyDescriptor imCFD("im", imCFO);

        std::vector<ColumnFamilyDescriptor> CFDs = { defaultCFD, mdCFD, mhCFD, imCFD };

        return rocksdb::DB::Open(Opts, Path, CFDs, &CFHs, &RDB);
    }

    [[nodiscard]] inline rocksdb::Status putMD(const rocksdb::Slice &K, const rocksdb::Slice &V)
    {
        rocksdb::WriteOptions WO;
        WO.disableWAL = true;
        WO.sync = false;
        return RDB->Put(WO, CFHs[1], K, V);
    }

    [[nodiscard]] inline rocksdb::Status deleteMD(const rocksdb::Slice &K)
    {
        rocksdb::WriteOptions WO;
        WO.disableWAL = true;
        WO.sync = false;
        return RDB->Delete(WO, CFHs[1], K);
    }

    rocksdb::Status putMH(const rocksdb::Slice &K, const rocksdb::Slice &V)
    {
        rocksdb::WriteOptions WO;
        WO.disableWAL = true;
        WO.sync = false;
        return RDB->Put(WO, CFHs[2], K, V);
    }

    rocksdb::Status putIM(const rocksdb::Slice &K, const rocksdb::Slice &V)
    {
        rocksdb::WriteOptions WO;
        WO.disableWAL = true;
        WO.sync = false;
        return RDB->Put(WO, CFHs[3], K, V);
    }

    rocksdb::Iterator *getIteratorMD(const rocksdb::ReadOptions &RO)
    {
        return RDB->NewIterator(RO, CFHs[1]);
    }

    void close()
    {
        using namespace rocksdb;

        FlushOptions FO;
        FO.allow_write_stall = true;
        FO.wait = true;

        RDB->Flush(FO);
        RDB->SyncWAL();

        RDB->Close();
        delete RDB;

        RDB = nullptr;
    }

    pb::Arena *getThreadArena()
    {
        pid_t tid = gettid();

        {
            std::lock_guard<std::mutex> L(ArenasMutex);

            auto It = Arenas.find(tid);
            if (It == Arenas.cend())
            {
                pb::ArenaOptions AO;
                AO.start_block_size = 1024 * 1024;
                AO.max_block_size = AO.start_block_size;

                pb::Arena *A = new pb::Arena(AO);

                Arenas[tid] = A;
                return A;
            } else {
                return It->second;
            }
        }
    }

public:
    rocksdb::DB *RDB;

    std::vector<rocksdb::ColumnFamilyHandle *> CFHs;

    std::mutex ArenasMutex;
    std::unordered_map<pid_t, pb::Arena *> Arenas;
};

} // namespace rdb

extern rdb::StorageInstance *SI;
extern std::atomic<size_t> NumFlushedPages;

#endif /* RDB_STORAGE_INSTANCE_H */
