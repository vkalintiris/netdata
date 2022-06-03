#include "replication-private.h"

using namespace replication;

Config replication::Cfg;

template <typename T>
static T clamp(const T& Value, const T& Min, const T& Max) {
    return std::max(Min, std::min(Value, Max));
}

void Config::readReplicationConfig(void) {
    const char *ConfigSectionReplication = CONFIG_SECTION_REPLICATION;

    /*
     * Enable/Disable replication
     */
    bool EnableReplication = config_get_boolean(ConfigSectionReplication, "enabled", true);

    /*
     * Backfill this many seconds on first connection of a child.
     */
    time_t SecondsToReplicateOnFirstConnection =
        config_get_number(ConfigSectionReplication, "seconds to replicate on first connection", 3600 * 24 * 4);

#if 0
    SecondsToReplicateOnFirstConnection = clamp<time_t>(SecondsToReplicateOnFirstConnection, 0, 2 * 24 * 3600);
#endif

    /*
     * Send at most this amount of <timestamp, storage_number>s for a single dim.
     */
    size_t MaxEntriesPerGapData  =
        config_get_number(ConfigSectionReplication, "max entries for each dimension gap data", 1024);

#if 0
    MaxEntriesPerGapData = clamp<size_t>(MaxEntriesPerGapData, 60, 1000);
#endif

    /*
     * Max number of gaps that we want parents to track for a child.
     */
    size_t MaxNumGapsToReplicate =
        config_get_number(ConfigSectionReplication, "max num gaps to replicate", 512);

#if 0
    MaxNumGapsToReplicate = clamp<size_t>(MaxNumGapsToReplicate, 1, 100);
#endif

    /*
     * Max number of queries that we should perform per second
     */
    size_t MaxQueriesPerSecond =
        config_get_number(ConfigSectionReplication, "max queries per second", 128);

#if 0
    MaxQueriesPerSecond = clamp<size_t>(MaxQueriesPerSecond, 5, 500);
#endif

    Cfg.EnableReplication = EnableReplication;
    Cfg.SecondsToReplicateOnFirstConnection = SecondsToReplicateOnFirstConnection;
    Cfg.MaxEntriesPerGapData = MaxEntriesPerGapData;
    Cfg.MaxNumGapsToReplicate = MaxNumGapsToReplicate;
    Cfg.MaxQueriesPerSecond = MaxQueriesPerSecond;
}
