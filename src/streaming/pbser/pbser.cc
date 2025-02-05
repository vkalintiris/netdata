#include <google/protobuf/io/coded_stream.h>
#include <google/protobuf/io/zero_copy_stream_impl.h>
#include "src/streaming/pbser/proto/netdata/v1/netdata.pb.h"
#include "pbser.h"

namespace pb = google::protobuf;
namespace nd = netdata;

static struct {
    POPEN_INSTANCE *pi = nullptr;
    int fd = -1;
    SPINLOCK lock = SPINLOCK_INITIALIZER;
} globals;

typedef struct pbser_context {
    SPINLOCK lock;
    nd::Host *host;
    std::atomic<uint32_t> max_chart_id;
    time_t last_entry_s;
    pb::Arena arena;
} pbser_context_t;

void pbser_rrdhost_init(RRDHOST *rh)
{
    spinlock_lock(&globals.lock);
    if (!globals.pi) {
        globals.pi = spawn_popen_run("/home/vk/repos/nd/master/src/otel/target/release/main");

        globals.fd = spawn_popen_write_fd(globals.pi);
        if (globals.fd < 0) {
            fatal("spawn_pope_write_fd: %d", globals.fd);
        }
    }
    spinlock_unlock(&globals.lock);

    pbser_context_t *ctx = new pbser_context_t();
    spinlock_init(&ctx->lock);
    ctx->host = pb::Arena::CreateMessage<nd::Host>(&ctx->arena);
    ctx->host->set_hostname(rrdhost_hostname(rh));
    ctx->max_chart_id = 0;
    ctx->last_entry_s = 0;

    rh->pbser_context = ctx;
}

void pbser_rrdhost_fini(RRDHOST *rh)
{
    delete rh->pbser_context;
}

void pbser_rrdhost_new_chart_id(RRDHOST *rh, RRDSET *rs) {
    pbser_context *ctx = rh->pbser_context;
    rs->pbser_id = ++ctx->max_chart_id;
}

void pbser_chart_update_start(RRDSET *rs) {
    RRDHOST *rh = rs->rrdhost;
    pbser_context *ctx = rh->pbser_context;

    spinlock_lock(&ctx->lock);

    if (rrdset_flag_check(rs, RRDSET_FLAG_NEEDS_PBSER_DEFINITION)) {
        nd::ChartDefinition *chart_definition = ctx->host->add_chart_definition();
        chart_definition->set_id(rs->pbser_id);
        chart_definition->set_name(rrdset_id(rs));
        chart_definition->set_family(rrdset_family(rs));
        chart_definition->set_context(rrdset_context(rs));
        chart_definition->set_units(rrdset_units(rs));
        chart_definition->set_update_every(rs->update_every);

        void *ptr;
        rrddim_foreach_read(ptr, rs) {
            RRDDIM *rd = (RRDDIM *) ptr;

            nd::DimensionDefinition *dimension_definition = chart_definition->add_dimension_definition();
            dimension_definition->set_name(rrddim_id(rd));
        }
        rrddim_foreach_done(ptr);

        // Equivalent to: rrdset_flag_clear(rs, RRDSET_FLAG_NEEDS_PBSER_DEFINITION);
        uint32_t *flags = (uint32_t *) &rs->flags;
        __atomic_and_fetch(flags, ~ static_cast<uint32_t>(RRDSET_FLAG_NEEDS_PBSER_DEFINITION), __ATOMIC_RELEASE);
    }

    nd::ChartCollection *chart_collection = ctx->host->add_chart_collection();
    chart_collection->set_id(rs->pbser_id);
}

void pbser_chart_update_metric(RRDDIM *rd, usec_t point_end_time_ut, NETDATA_DOUBLE value) {
    RRDHOST *rh = rd->rrdset->rrdhost;
    pbser_context *ctx = rh->pbser_context;

    size_t index = ctx->host->chart_collection_size() - 1;
    nd::ChartCollection &chart_collection = ctx->host->mutable_chart_collection()->at(index);

    nd::DimensionCollection *dimension_collection = chart_collection.add_dimension_collection();
    dimension_collection->set_time(point_end_time_ut);
    dimension_collection->set_value(value);
}

void pbser_chart_update_end(RRDSET *rs)
{
    RRDHOST *rh = rs->rrdhost;
    pbser_context *ctx = rh->pbser_context;

    if (ctx->last_entry_s == 0) {
        ctx->last_entry_s = rrdset_last_entry_s(rs);
    }

    // Flush message
    if (rrdset_last_entry_s(rs) > ctx->last_entry_s) {
        uint32_t size = ctx->host->ByteSizeLong();

        auto start_time = std::chrono::high_resolution_clock::now();
        spinlock_lock(&globals.lock);

        int n = write(globals.fd, &size, sizeof(uint32_t));
        if (n != sizeof(uint32_t)) {
            fatal("Failed to write message size (n=%d)", n);
        }

        if (!ctx->host->SerializeToFileDescriptor(globals.fd)) {
            fatal("WTFFFFFFFFFFFFFFFFF?");
        }

        spinlock_unlock(&globals.lock);
        auto end_time = std::chrono::high_resolution_clock::now();
        auto duration = std::chrono::duration_cast<std::chrono::microseconds>(end_time - start_time);

        netdata_log_error("Protobuf serialization wall_time_us=%ld", duration.count());

        ctx->arena.Reset();
        ctx->host = pb::Arena::CreateMessage<nd::Host>(&ctx->arena);

        ctx->last_entry_s = rrdset_last_entry_s(rs);
    }

    spinlock_unlock(&ctx->lock);
}
