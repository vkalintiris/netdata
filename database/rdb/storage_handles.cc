#include "database/rdb/StorageInstance.h"
#include "database/rdb/protos/rdbv.pb.h"
#include "database/rrd.h"
#include "rdb-private.h"
#include <cstdint>
#include <limits>

namespace pb = google::protobuf;

using rocksdb::Status;
using rocksdb::Iterator;
using rocksdb::ReadOptions;

static std::atomic<uint32_t> MaxGroupID = 0;
static std::atomic<uint32_t> MaxMetricID = 0;

/*===---------------------------------------------------------------------===*/
/* Groups                                                                    */
/*===---------------------------------------------------------------------===*/

STORAGE_METRICS_GROUP *rdb_metrics_group_get(STORAGE_INSTANCE *si, uuid_t *uuid)
{
    UNUSED(si);
    UNUSED(uuid);

    global_statistics_metrics_group_get();
    return reinterpret_cast<STORAGE_METRICS_GROUP *>(++MaxGroupID);
}

void rdb_metrics_group_release(STORAGE_INSTANCE *si, STORAGE_METRICS_GROUP *smg)
{
    UNUSED(si);
    UNUSED(smg);

    global_statistics_metrics_group_release();
}

/*===---------------------------------------------------------------------===*/
/* Metrics                                                                   */
/*===---------------------------------------------------------------------===*/

STORAGE_METRIC_HANDLE *rdb_metric_create(STORAGE_INSTANCE *si, STORAGE_METRICS_GROUP *smg, RRDDIM *rd)
{
    global_statistics_metric_get_or_create();

    UNUSED(si);

    STORAGE_METRIC_HANDLE *smh = rdb_metric_get(si, &rd->metric_uuid);
    if (smh)
        return smh;

    uint32_t GID = static_cast<uint32_t>(reinterpret_cast<uintptr_t>(smg));
    uint32_t MID = ++MaxMetricID;

    Status S = SI->setUUIDtoIDs(rd->metric_uuid, GID, MID);
    if (!S.ok())
        return nullptr;

    return reinterpret_cast<STORAGE_METRIC_HANDLE *>(new rdb::MetricHandle(GID, MID));
}

STORAGE_METRIC_HANDLE *rdb_metric_get(STORAGE_INSTANCE *si, uuid_t *uuid)
{
    UNUSED(si);

    global_statistics_metric_get();

    std::optional<rdb::MetricHandle> OMH = SI->getMetricHandleFromUUID(*uuid);
    if (!OMH.has_value())
        return nullptr;

    return reinterpret_cast<STORAGE_METRIC_HANDLE *>(new rdb::MetricHandle(std::move(OMH.value())));
}

STORAGE_METRIC_HANDLE *rdb_metric_dup(STORAGE_METRIC_HANDLE *smh)
{
    global_statistics_metric_dup();

    rdb::MetricHandle *MH = reinterpret_cast<rdb::MetricHandle *>(smh);
    return reinterpret_cast<STORAGE_METRIC_HANDLE *>(new rdb::MetricHandle(*MH));
}

void rdb_metric_release(STORAGE_METRIC_HANDLE *smh)
{
    global_statistics_metric_release();

    rdb::MetricHandle *MH = reinterpret_cast<rdb::MetricHandle *>(smh);
    delete MH;
}

bool rdb_metric_retention_by_uuid(STORAGE_INSTANCE *si, uuid_t *uuid, time_t *first_entry_s, time_t *last_entry_s)
{
    global_statistics_metric_retention_by_uuid();

    UNUSED(si);
    UNUSED(uuid);
    UNUSED(first_entry_s);
    UNUSED(last_entry_s);

    fatal("%s() - Not implemented yet.", __func__);

    return false;
}

time_t rdb_metric_oldest_time(STORAGE_METRIC_HANDLE *SMH, STORAGE_COLLECT_HANDLE *SCH)
{
    global_statistics_metric_oldest_time();

    time_t After = 0;

    if (SMH)
    {
        rdb::MetricHandle *MH = reinterpret_cast<rdb::MetricHandle *>(SMH);
        std::optional<uint32_t> OB = MH->after();
        if (OB.has_value())
            After = OB.value();
    }

    if (!After && SCH)
    {
        rdb_collect_handle *RCH = reinterpret_cast<rdb_collect_handle *>(SCH);
        After = RCH->ch.after() / USEC_PER_SEC;
    }

    return After;
}

time_t rdb_metric_latest_time(STORAGE_METRIC_HANDLE *SMH, STORAGE_COLLECT_HANDLE *SCH)
{
    global_statistics_metric_latest_time();

    time_t Before = 0;

    if (SCH)
    {
        rdb_collect_handle *RCH = reinterpret_cast<rdb_collect_handle *>(SCH);
        Before = RCH->ch.before() / USEC_PER_SEC;
    }

    if (SMH && !Before)
    {
        rdb::MetricHandle *MH = reinterpret_cast<rdb::MetricHandle *>(SMH);
        std::optional<uint32_t> OA = MH->before();
        if (OA.has_value())
            Before = OA.value();
    }

    return Before;
}

/*===---------------------------------------------------------------------===*/
/* Collection handles                                                        */
/*===---------------------------------------------------------------------===*/
thread_local pb::Arena *ThreadArena = nullptr;

STORAGE_COLLECT_HANDLE *rdb_store_metric_init(STORAGE_METRIC_HANDLE *smh,
                                              uint32_t update_every)
{
    global_statistics_store_metric_init();
    global_statistics_rdb_collection_handles_incr();

    using namespace rdb;

    if (!ThreadArena) {
        pb::ArenaOptions AO;
        AO.start_block_size = 1024 * 1024;
        AO.max_block_size = AO.start_block_size;
        ThreadArena = new pb::Arena(AO);
    }

    MetricHandle *MH = reinterpret_cast<MetricHandle *>(smh);

    rdb::PageOptions PO = rdb::PageOptions();
    PO.initial_slots = (MH->mid() % PO.capacity) + 1;

    auto CH = rdb::CollectionHandle::create(*ThreadArena, PO, MH->gid(), MH->mid());
    if (!CH.has_value())
        fatal("Could not create collection handle");

    CH->setUpdateEvery(*MH, update_every * USEC_PER_SEC);

    rdb_collect_handle *rch = new rdb_collect_handle(CH.value());
    return reinterpret_cast<STORAGE_COLLECT_HANDLE *>(rch);
}

void rdb_store_metric_next(STORAGE_METRIC_HANDLE *smh, STORAGE_COLLECT_HANDLE *sch,
                           usec_t point_in_time_ut, NETDATA_DOUBLE n,
                           NETDATA_DOUBLE min_value, NETDATA_DOUBLE max_value,
                           uint16_t count, uint16_t anomaly_count, SN_FLAGS flags)
{
    using namespace rdb;
    global_statistics_store_metric_next();

    UNUSED(smh);

    MetricHandle *MH = reinterpret_cast<MetricHandle *>(smh);
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);


    STORAGE_POINT SP = {
        .min = min_value,
        .max = max_value,
        .sum = n,

        .start_time_s = 0,
        .end_time_s = 0,

        .count = count,
        .anomaly_count = anomaly_count,

        .flags = flags,
    };

    rch->ch.store_next(*MH, point_in_time_ut, SP);
}

void rdb_store_metric_flush(STORAGE_METRIC_HANDLE *smh, STORAGE_COLLECT_HANDLE *sch)
{
    using namespace rdb;
    global_statistics_store_metric_flush();

    UNUSED(smh);

    MetricHandle *MH = reinterpret_cast<MetricHandle *>(smh);
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);

    rch->ch.flush(*MH);
}

void rdb_store_metric_change_collection_frequency(STORAGE_METRIC_HANDLE *smh, STORAGE_COLLECT_HANDLE *sch, int update_every_s)
{
    using namespace rdb;
    global_statistics_store_metric_change_collection_frequency();

    UNUSED(smh);

    MetricHandle *MH = reinterpret_cast<MetricHandle *>(smh);
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);

    rch->ch.setUpdateEvery(*MH, update_every_s * USEC_PER_SEC);
}

int rdb_store_metric_finalize(STORAGE_METRIC_HANDLE *smh, STORAGE_COLLECT_HANDLE *sch)
{
    using namespace rdb;
    global_statistics_store_metric_finalize();
    global_statistics_rdb_collection_handles_decr();

    UNUSED(smh);

    MetricHandle *MH = reinterpret_cast<MetricHandle *>(smh);
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);

    rch->ch.flush(*MH);
    delete rch;
    return 0;
}

/*===---------------------------------------------------------------------===*/
/* Query ops                                                                 */
/*===---------------------------------------------------------------------===*/

struct rdb_query_handle
{
    rdb::MetricHandle *MH;
    pb::Arena Arena;

    uint32_t After;
    uint32_t Before;
    uint32_t Now;
    rdb::UniversalQuery UQ;

    rdb_query_handle(rdb::MetricHandle *MH, rdb::CollectionHandle *CH, uint32_t After, uint32_t Before) :
        MH(MH), Arena(), 
        After(After), Before(Before), Now(After),
        UQ(MH, CH, After, Before)
    {
    }

    inline STORAGE_POINT next()
    {
        STORAGE_POINT SP = UQ.next();
        Now = SP.start_time_s;
        return SP;
    }

    inline bool isFinished()
    {
        return (Now > Before) ? true : UQ.isFinished(Arena);
    }

    ~rdb_query_handle()
    {
        UQ.finalize();
        rdb_metric_release(reinterpret_cast<STORAGE_METRIC_HANDLE *>(MH));
    }
};

void rdb_load_metric_init(STORAGE_METRIC_HANDLE *smh,
                          STORAGE_COLLECT_HANDLE *sch,
                          struct storage_engine_query_handle *seqh,
                          time_t After,
                          time_t Before,
                          STORAGE_PRIORITY priority)
{
    using namespace rdb;
    global_statistics_load_metric_init();

    MetricHandle *MH = reinterpret_cast<MetricHandle *>(rdb_metric_dup(smh));
    rdb_collect_handle *rch = reinterpret_cast<rdb_collect_handle *>(sch);

    After = std::max(rdb_metric_oldest_time(smh, sch), After);
    Before = std::min(rdb_metric_latest_time(smh, nullptr), Before);

    rdb_query_handle *rqh = new rdb_query_handle(MH, &rch->ch, After, Before);

    seqh->start_time_s = After;
    seqh->end_time_s = Before;
    seqh->backend = STORAGE_ENGINE_BACKEND_RDB;
    seqh->priority = priority;
    seqh->handle = reinterpret_cast<STORAGE_QUERY_HANDLE *>(rqh);
}

STORAGE_POINT rdb_load_metric_next(struct storage_engine_query_handle *seqh)
{
    global_statistics_load_metric_next();

    rdb_query_handle *rqh = reinterpret_cast<rdb_query_handle *>(seqh->handle);
    return rqh->UQ.next();
}

int rdb_load_metric_is_finished(struct storage_engine_query_handle *seqh)
{
    global_statistics_load_metric_is_finished();

    rdb_query_handle *rqh = reinterpret_cast<rdb_query_handle *>(seqh->handle);
    return rqh->isFinished();
}

void rdb_load_metric_finalize(struct storage_engine_query_handle *seqh) {
    global_statistics_load_metric_finalize();

    rdb_query_handle *rqh = reinterpret_cast<rdb_query_handle *>(seqh->handle);
    delete rqh;
}

/*===---------------------------------------------------------------------===*/
/* Storage instance                                                          */
/*===---------------------------------------------------------------------===*/

time_t rdb_global_first_time_s(STORAGE_INSTANCE *si)
{
    global_statistics_global_first_time();

    UNUSED(si);

    Iterator *It = SI->getIteratorMD(ReadOptions());

    It->SeekToFirst();
    if (!It->Valid()) {
        // We probably haven't written anything yet. Should we consult the
        // collection handles?
        return 0;
    }

    rdb::Key K(It->key());

    delete It;
    return static_cast<time_t>(K.pit());
}

uint64_t rdb_disk_space_used(STORAGE_INSTANCE *si)
{
    global_statistics_disk_space_used();

    UNUSED(si);

    std::array<rocksdb::Range, 1> ranges;
    std::array<uint64_t, 1> sizes;
    rocksdb::SizeApproximationOptions Opts;

    Opts.include_memtables = false;
    Opts.files_size_error_margin = 0.1;

    ranges[0].start = rdb::Key::min().slice();
    ranges[0].limit = rdb::Key::max().slice();

    Status S = SI->RDB->GetApproximateSizes(Opts, SI->RDB->DefaultColumnFamily(), ranges.data(), ranges.size(), sizes.data());
    if (!S.ok()) {
        netdata_log_error("Could not get approximate size for default CF: %s", S.ToString().c_str());
        return 0;
    }

    return sizes[0];
}
