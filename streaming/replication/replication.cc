#include "replication-private.h"

void replication_new(RRDHOST *RH) {
    RH->repl_handle = NULL;
}

void replication_delete(RRDHOST *RH) {
    UNUSED(RH);
}
