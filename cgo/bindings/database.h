#ifndef CGO_DATABASE_H
#define CGO_DATABASE_H

#include <stdlib.h>

typedef struct rrdhost* RRDHOSTP;
typedef struct rrdset* RRDSETP;
typedef struct rrdresult* RRDRP;

extern RRDHOSTP localhost;

void rrdhostp_rdlock(RRDHOSTP host);
void rrdhostp_unlock(RRDHOSTP host);

const char *rrdhostp_hostname(RRDHOSTP host);
RRDSETP rrdhostp_root_set(RRDHOSTP host);

void rrdsetp_rdlock(RRDSETP set);
void rrdsetp_unlock(RRDSETP set);

RRDSETP rrdsetp_next_set(RRDSETP set);
const char *rrdsetp_name(RRDSETP set);
int rrdsetp_update_every(RRDSETP set);
int rrdsetp_num_dims(RRDSETP set);

long long cfg_get_number(const char *section, const char *name, long long value);

RRDRP rrdrp_get(RRDSETP set, int num_samples);

long rrdrp_num_rows(RRDRP res);

#endif /* CGO_DATABASE_H */
