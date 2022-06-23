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

void replication_thread_start(RRDHOST *RH);
void replication_thread_stop(RRDHOST *RH);

void replication_receiver_connect(RRDHOST *RH, char *Buf, size_t Len);
void replication_sender_connect(RRDHOST *RH, const char *Buf, size_t Len);

bool replication_receiver_fill_gap(RRDHOST *RH, const char *Buf);
void replication_receiver_drop_gap(RRDHOST *RH, time_t After, time_t Before);

const char *replication_logs(RRDHOST *RH);

#ifdef __cplusplus
};
#endif

#endif /* REPLICATION_H */
