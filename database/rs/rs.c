#include "daemon/common.h"
#include "rs.h"
#include "rnd/adapter/adapter.h"

struct odb {
    ODB *db;
    netdata_mutex_t mutex;
};

odb_t *odb_new() {
  odb_t *odb = callocz(1, sizeof(odb_t));
  
  odb->db = rs_odb_new();
  netdata_mutex_init(&odb->mutex);
  return odb;
}

void odb_destroy(odb_t *odb) {
  rs_odb_destroy(odb->db);
  netdata_mutex_destroy(&odb->mutex);
  freez(odb);
}

void odb_start(odb_t *odb) {
  netdata_mutex_lock(&odb->mutex);
  rs_odb_start(odb->db);
  netdata_mutex_unlock(&odb->mutex);
}

oid_t odb_add(odb_t *odb, const char *sid) {
  netdata_mutex_lock(&odb->mutex);
  oid_t oid = rs_odb_add(odb->db, sid);
  netdata_mutex_unlock(&odb->mutex);
  return oid;
}

void odb_remove(odb_t *odb, oid_t oid) {
  netdata_mutex_lock(&odb->mutex);
  rs_odb_remove(odb->db, oid);
  netdata_mutex_unlock(&odb->mutex);
}

void odb_create_host(odb_t *odb, const char *hostname, const char *guid) {
  netdata_mutex_lock(&odb->mutex);
  rs_odb_create_host(odb->db, hostname, guid);
  netdata_mutex_unlock(&odb->mutex);
}
