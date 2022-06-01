#include "replication-private.h"

using namespace replication;

/*
 * C API
 */

void replication_init(void) {
    Cfg.readReplicationConfig();
}

void replication_fini(void) {
}
