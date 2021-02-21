#ifndef CGO_DATABASE_H
#define CGO_DATABASE_H

typedef struct rrdhost* RRDHOSTP;
typedef struct rrdset* RRDSETP;

extern RRDHOSTP localhost;

void rrdhostp_rdlock(RRDHOSTP host);
void rrdhostp_unlock(RRDHOSTP host);

const char *rrdhostp_hostname(RRDHOSTP host);
RRDSETP rrdhostp_root_set(RRDHOSTP host);

RRDSETP rrdsetp_next_set(RRDSETP set);
const char *rrdsetp_name(RRDSETP set);

#endif /* CGO_DATABASE_H */
