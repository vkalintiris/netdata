#ifndef REPLICATION_H
#define REPLICATION_H

#ifdef __cplusplus
extern "C" {
#endif

#include "daemon/common.h"

typedef void * replication_handle_t;

void replication_init(void);
void replication_fini(void);

void replication_new_host(RRDHOST *RH);
void replication_delete_host(RRDHOST *RH);

void replication_new_dimension(RRDHOST *RH, RRDDIM *RD);
void replication_delete_dimension(RRDHOST *RH, RRDDIM *RD);

void replication_connected(RRDHOST *RH);
void replication_disconnected(RRDHOST *RH);

bool replication_receiver_serialize_gaps(RRDHOST *RH, char *Buf, size_t Len);
bool replication_sender_deserialize_gaps(RRDHOST *RH, const char *Buf, size_t Len);

bool replication_receiver_fill_gap(RRDHOST *RH, const char *Buf);
void replication_receiver_drop_gap(RRDHOST *RH, time_t After, time_t Before);

void replication_thread_start(RRDHOST *RH);
void replication_thread_stop(RRDHOST *RH);


#ifdef __cplusplus
};
#endif

#endif /* REPLICATION_H */
