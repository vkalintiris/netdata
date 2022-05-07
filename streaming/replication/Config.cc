#include "replication-private.h"

using namespace replication;

Config replication::Cfg;

void Config::readReplicationConfig(void) {
    const char *ConfigSectionReplication = CONFIG_SECTION_REPLICATION;

    bool EnableReplication = config_get_boolean(ConfigSectionReplication, "enabled", true);

    Cfg.EnableReplication = EnableReplication;
}
