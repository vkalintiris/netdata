// SPDX-License-Identifier: GPL-3.0-or-later

#include "ml-private.h"

static int
get_num_dims(RRDSET *set)
{
    int num_dims = 0;

    RRDDIM *dim;
    rrddim_foreach_read(dim, set) {
        num_dims++;
    }

    return num_dims;
}

static void
dict_add_dim(RRDDIM *dim)
{
    assert(dim && dim->id && dim->rrdset);

    char id_buf[ML_UNIT_MAX_ID];
    snprintfz(id_buf, ML_UNIT_MAX_ID, "%s.%s", dim->rrdset->id, dim->id);

    ml_unit_t *unit = dictionary_get(ml_cfg.train_dict, id_buf);
    if (unit)
        return;

    unit = callocz(1, sizeof(ml_unit_t));
    unit->dim = dim;
    unit->km_ref = kmeans_new(2);
    unit->num_dims = get_num_dims(dim->rrdset);

    netdata_rwlock_init(&unit->rwlock);

    dictionary_set(ml_cfg.train_dict, id_buf, unit, sizeof(unit));
}

static void
dict_delete_dim(RRDDIM *dim)
{
    assert(dim && dim->id && dim->rrdset);

    char id_buf[ML_UNIT_MAX_ID];
    snprintfz(id_buf, ML_UNIT_MAX_ID, "%s.%s", dim->rrdset->id, dim->id);

    ml_unit_t *unit = dictionary_get(ml_cfg.train_dict, id_buf);
    if (!unit)
        return;

    info("Removing dim %s", id_buf);

    kmeans_delete(unit->km_ref);
    freez(unit);

    dictionary_del(ml_cfg.train_dict, id_buf);
}

static void
dict_add_set(RRDSET *set)
{
    assert(set);

    ml_unit_t *unit = dictionary_get(ml_cfg.train_dict, set->id);
    if (unit)
        return;

    unit = callocz(1, sizeof(ml_unit_t));
    unit->set = set;
    unit->km_ref = kmeans_new(2);
    unit->num_dims = get_num_dims(set);

    netdata_rwlock_init(&unit->rwlock);

    dictionary_set(ml_cfg.train_dict, set->id, unit, sizeof(unit));
}

static void
dict_delete_set(RRDSET *set)
{
    assert(set);

    ml_unit_t *unit = dictionary_get(ml_cfg.train_dict, set->id);
    if (!unit)
        return;

    info("Removing set %s", set->id);

    kmeans_delete(unit->km_ref);
    freez(unit);

    dictionary_del(ml_cfg.train_dict, set->id);
}

ml_unit_t *
ml_dict_get_unit_dim(DICTIONARY *dict, RRDDIM *dim)
{
    assert(dim && dim->id && dim->rrdset);

    char id_buf[ML_UNIT_MAX_ID];
    snprintfz(id_buf, ML_UNIT_MAX_ID, "%s.%s", dim->rrdset->id, dim->id);

    return dictionary_get(dict, id_buf);
}

ml_unit_t *
ml_dict_get_unit_set(DICTIONARY *dict, RRDSET *set)
{
    assert(set);
    return dictionary_get(dict, set->id);
}

// Callback used to create a shallow key/val copy in another dict.
static int
dict_copy_unit_cb(char *key, void *value, void *data)
{
    DICTIONARY *dict = data;
    ml_unit_t *unit = value;

    // TODO: assert: return value == NULL
    dictionary_set(dict, key, unit, sizeof(unit));
    return 0;
}

// Callback used to train the units in the train dict.
static int
dict_train_unit_cb(void *entry, void *data)
{
    (void) data;

    ml_unit_t *unit = entry;

    // FIXME: We should lock the unit for the entire training step.
    netdata_rwlock_wrlock(&unit->rwlock);
    ml_unit_train(entry);
    netdata_rwlock_unlock(&unit->rwlock);

    return 0;
}

// Callback used to predict the units in the prediction dict.
static int
dict_predict_unit_cb(void *entry, void *data)
{
    (void) data;

    ml_unit_t *unit = entry;

    // We train the dim units that belong to the same set at once. No need
    // to do anything if this dim unit has been already processed.
    if (unit->dim && unit->predicted)
        goto SKIP_PREDICTION;

    // Use a try-lock to avoid blocking until the entire training thread
    // finishes training all charts/dims.
    if (!netdata_rwlock_tryrdlock(&unit->rwlock)) {
        ml_unit_predict(entry);
        netdata_rwlock_unlock(&unit->rwlock);
    }

SKIP_PREDICTION:
    // Mark it back to false for the next iteration of the prediction thread.
    unit->predicted = false;
    return 0;
}

// Callback used to update anomaly score charts.
static int
dict_ml_charts_unit_cb(void *entry, void *data)
{
    (void) data;

    ml_unit_t *unit = entry;

    if (unit->dim && unit->ml_chart_updated)
        goto SKIP_ML_CHART_UPDATE;

    // No need to lock. We are just reading stuff that are owned by the
    // prediction thread.
    ml_chart_update_unit(entry);

SKIP_ML_CHART_UPDATE:
    // Mark it back to false for the next iteration of the prediction thread.
    unit->ml_chart_updated = false;
    return 0;
}

void
ml_dict_train(void)
{
    RRDSET *set;
    RRDDIM *dim;

    struct timeval begin_tv, end_tv;

    now_monotonic_high_precision_timeval(&begin_tv);

    /*
     * Insert/Update the entries in the training dict
    */

    rrdhost_rdlock(localhost);

    rrdset_foreach_read(set, localhost) {
        rrdset_rdlock(set);

        // Skip anomaly score charts we have created.
        if (!strncmp(ML_CHART_PREFIX, set->id, strlen(ML_CHART_PREFIX)))
            goto UNLOCK_SET;

        // Skip charts we are not interested in.
        if (simple_pattern_matches(ml_cfg.skip_charts, set->name))
            goto UNLOCK_SET;

        bool is_obsolete = rrdset_flag_check(set, RRDSET_FLAG_ARCHIVED) ||
                           rrdset_flag_check(set, RRDSET_FLAG_OBSOLETE);

        // Delete/Add set.
        if (!simple_pattern_matches(ml_cfg.train_per_dim, set->name)) {
            is_obsolete ? dict_delete_set(set) : dict_add_set(set);
            goto UNLOCK_SET;
        }

        // Delete all dims if the set is obsolete.
        if (is_obsolete) {
            rrddim_foreach_read(dim, set) { dict_delete_dim(dim); }
            goto UNLOCK_SET;
        }

        // Remove obsolete dims, add missing ones.
        rrddim_foreach_read(dim, set) {
            is_obsolete = rrddim_flag_check(dim, RRDDIM_FLAG_ARCHIVED) ||
                          rrddim_flag_check(dim, RRDDIM_FLAG_OBSOLETE);
            is_obsolete ? dict_delete_dim(dim) : dict_add_dim(dim);
        }

UNLOCK_SET:
        rrdset_unlock(set);
    }

    rrdhost_unlock(localhost);

    now_monotonic_high_precision_timeval(&end_tv);

    info("[%Ld usec] TR - inserts: %llu, deletes: %llu, searches: %llu, entries: %llu",
         dt_usec(&end_tv, &begin_tv),
         ml_cfg.train_dict->stats->inserts,
         ml_cfg.train_dict->stats->deletes,
         ml_cfg.train_dict->stats->searches,
         ml_cfg.train_dict->stats->entries);

    now_monotonic_high_precision_timeval(&begin_tv);

    /*
     * Create a shallow dict copy for the prediction thread.
    */

    netdata_rwlock_wrlock(&ml_cfg.predict_dict_rwlock);

    if (ml_cfg.predict_dict)
        dictionary_destroy(ml_cfg.predict_dict);

    int dict_flags = DICTIONARY_FLAG_SINGLE_THREADED |
                     DICTIONARY_FLAG_NAME_LINK_DONT_CLONE |
                     DICTIONARY_FLAG_VALUE_LINK_DONT_CLONE |
                     DICTIONARY_FLAG_WITH_STATISTICS;

    ml_cfg.predict_dict = dictionary_create(dict_flags);
    dictionary_get_all_name_value(ml_cfg.train_dict, dict_copy_unit_cb, ml_cfg.predict_dict);

    assert(ml_cfg.train_dict->stats->entries == ml_cfg.predict_dict->stats->entries);

    netdata_rwlock_unlock(&ml_cfg.predict_dict_rwlock);

    now_monotonic_high_precision_timeval(&end_tv);

    info("[%Ld usec] PR - inserts: %llu, deletes: %llu, searches: %llu, entries: %llu",
         dt_usec(&end_tv, &begin_tv),
         ml_cfg.predict_dict->stats->inserts,
         ml_cfg.predict_dict->stats->deletes,
         ml_cfg.predict_dict->stats->searches,
         ml_cfg.predict_dict->stats->entries);

    /*
     * Train each entry in the training dict.
    */

    dictionary_get_all(ml_cfg.train_dict, dict_train_unit_cb, NULL);
}

void
ml_dict_predict(void) {
    struct timeval begin_tv, end_tv;

    now_monotonic_high_precision_timeval(&begin_tv);

    info("Running prediction dict");

    netdata_rwlock_rdlock(&ml_cfg.predict_dict_rwlock);

    if (ml_cfg.predict_dict) {
        // Update anomaly score
        dictionary_get_all(ml_cfg.predict_dict, dict_predict_unit_cb, NULL);

        // Update ml charts
        dictionary_get_all(ml_cfg.predict_dict, dict_ml_charts_unit_cb, NULL);
    }

    netdata_rwlock_unlock(&ml_cfg.predict_dict_rwlock);

    now_monotonic_high_precision_timeval(&end_tv);

    info("[%Ld usec] Prediction loop", dt_usec(&end_tv, &begin_tv));
}
