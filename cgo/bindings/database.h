#ifndef CGO_DATABASE_H
#define CGO_DATABASE_H

typedef struct rrdhost* RRDHOSTP;

extern RRDHOSTP localhost;

const char *rrdhostp_hostname(RRDHOSTP host);

#endif /* CGO_DATABASE_H */
