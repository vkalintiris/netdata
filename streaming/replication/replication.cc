#include "libnetdata/perfetto/perfetto.h"
#include "replication-private.h"

#include <fstream>
#include <iostream>
#include <sstream>

std::unique_ptr<perfetto::TracingSession> TS;

PERFETTO_DEFINE_CATEGORIES(
    perfetto::Category("replication"));
PERFETTO_TRACK_EVENT_STATIC_STORAGE();

std::unique_ptr<perfetto::TracingSession> StartTracing() {
    perfetto::TraceConfig cfg;
    cfg.add_buffers()->set_size_kb(1024);
    auto* ds_cfg = cfg.add_data_sources()->mutable_config();
    ds_cfg->set_name("track_event");

    auto tracing_session = perfetto::Tracing::NewTrace();
    tracing_session->Setup(cfg);
    tracing_session->StartBlocking();
    return tracing_session;
}

void StopTracing(std::unique_ptr<perfetto::TracingSession> tracing_session) {
  // Make sure the last event is closed for this example.
  perfetto::TrackEvent::Flush();

  // Stop tracing and read the trace data.
  tracing_session->FlushBlocking();
  tracing_session->StopBlocking();

  // Write the trace into a file.
  std::vector<char> trace_data(tracing_session->ReadTraceBlocking());
  std::ofstream output;
  output.open("/tmp/replication.pftrace", std::ios::out | std::ios::binary);
  output.write(&trace_data[0], trace_data.size());
  output.close();

  sleep(1);
}

struct TimeRange {
    time_t After;
    time_t Before;

    friend bool operator==(const TimeRange &LHS, const TimeRange &RHS) {
        return (LHS.After == RHS.After) && (LHS.Before == RHS.Before);
    };

    friend bool operator!=(const TimeRange &LHS, const TimeRange &RHS) {
        return !(LHS == RHS);
    };
};

enum class EventType {
    Connected,
    Disconnected
};

struct Event {
    EventType Type;
    time_t Timestamp;
    TimeRange EntriesRange;
};

class Replicator {
public:
    Replicator(RRDHOST *RH) : RH(RH) {}

    void connected() {
        TRACE_EVENT("replication", "connected", "host", RH->hostname);

        Event ConnectionEvent = {
            EventType::Connected,
            now_realtime_sec(),
            { rrdhost_first_entry_t(RH), rrdhost_last_entry_t(RH) }
        };

        ConnectionEvents.push_back(ConnectionEvent);

        std::stringstream SS;
        dump(SS);
        error("%s", SS.str().c_str());
    }

    void disconnected() {
        TRACE_EVENT("replication", "disconnected", "host", RH->hostname);

        Event DisconnectionEvent = {
            EventType::Disconnected,
            now_realtime_sec(),
            { rrdhost_first_entry_t(RH), rrdhost_last_entry_t(RH) }
        };

        DisconnectionEvents.push_back(DisconnectionEvent);

        std::stringstream SS;
        dump(SS);
        error("%s", SS.str().c_str());
    }

    void dump(std::ostream &OS) {
        OS << "Hostname: " << RH->hostname << "\n";

        OS << "Connection Events:\n";
        for (const auto &CE : ConnectionEvents) {
            OS << "\tTimestamp: " << CE.Timestamp << "\n";
            OS << "\tAfter: " << CE.EntriesRange.After  << "\n";
            OS << "\tBefore: " << CE.EntriesRange.Before << "\n";
        }

        OS << "Disconnection Events:\n";
        for (const auto &DE : DisconnectionEvents) {
            OS << "\tTimestamp: " << DE.Timestamp << "\n";
            OS << "\tAfter: " << DE.EntriesRange.After  << "\n";
            OS << "\tBefore: " << DE.EntriesRange.Before << "\n";
        }

        OS << "\n";
    };

private:
    RRDHOST *RH;

    std::vector<Event> ConnectionEvents;
    std::vector<Event> DisconnectionEvents;
};


void replication_init(void) {
    perfetto::TracingInitArgs args;

    args.backends |= perfetto::kInProcessBackend;
    perfetto::Tracing::Initialize(args);
    perfetto::TrackEvent::Register();

    TS = StartTracing();
}

void replication_fini(void) {
    StopTracing(std::move(TS));
}

void replication_new(RRDHOST *RH) {
    Replicator *R = new Replicator(RH);
    RH->repl_handle = static_cast<replication_handle_t>(R);
}

void replication_delete(RRDHOST *RH) {
    Replicator *R = static_cast<Replicator *>(RH->repl_handle);
    delete R;
}

void replication_connected(RRDHOST *RH) {
    Replicator *R = new Replicator(RH);
    R->connected();
}

void replication_disconnected(RRDHOST *RH) {
    Replicator *R = new Replicator(RH);
    R->disconnected();
}
