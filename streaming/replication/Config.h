#ifndef REPLICATION_CONFIG_H
#define REPLICATION_CONFIG_H

namespace replication {

class Config {
public:
    bool EnableReplication;

    void readReplicationConfig();
};

extern Config Cfg;

} // namespace replication

#endif /* REPLICATION_CONFIG_H */
