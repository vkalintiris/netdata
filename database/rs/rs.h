#ifndef DATABASE_RS_H
#define DATABASE_RS_H

// #include "rnd/adapter/adapter.h"

typedef struct odb odb_t;
typedef unsigned long oid_t;

odb_t *odb_new();
void odb_destroy(odb_t *odb);

void odb_start(odb_t *odb);
oid_t odb_add(odb_t *odb, const char *sid);
void odb_remove(odb_t *odb, oid_t oid);

void odb_create_host(odb_t *odb, const char *hostname, const char *guid);

#endif /* DATABASE_RS_H */
