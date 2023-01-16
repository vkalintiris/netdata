// SPDX-License-Identifier: GPL-3.0-or-later

#include "ad_charts.h"

void nml_update_dimensions_chart(RRDHOST *rh, const nml_machine_learning_stats_t &mls) {
    /*
     * Machine learning status
    */
    {
        static thread_local RRDSET *rs = NULL;

        static thread_local RRDDIM *rd_enabled = NULL;
        static thread_local RRDDIM *rd_disabled_ue = NULL;
        static thread_local RRDDIM *rd_disabled_sp = NULL;

        if (!rs) {
            char id_buf[1024];
            char name_buf[1024];

            snprintfz(id_buf, 1024, "machine_learning_status_on_%s", localhost->machine_guid);
            snprintfz(name_buf, 1024, "machine_learning_status_on_%s", rrdhost_hostname(localhost));

            rs = rrdset_create(
                    rh,
                    "netdata", // type
                    id_buf,
                    name_buf, // name
                    NETDATA_ML_CHART_FAMILY, // family
                    "netdata.machine_learning_status", // ctx
                    "Machine learning status", // title
                    "dimensions", // units
                    NETDATA_ML_PLUGIN, // plugin
                    NETDATA_ML_MODULE_TRAINING, // module
                    NETDATA_ML_CHART_PRIO_MACHINE_LEARNING_STATUS, // priority
                    rh->rrd_update_every, // update_every
                    RRDSET_TYPE_LINE // chart_type
            );
            rrdset_flag_set(rs , RRDSET_FLAG_ANOMALY_DETECTION);

            rd_enabled = rrddim_add(rs, "enabled", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
            rd_disabled_ue = rrddim_add(rs, "disabled-ue", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
            rd_disabled_sp = rrddim_add(rs, "disabled-sp", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
        }

        rrddim_set_by_pointer(rs, rd_enabled, mls.num_machine_learning_status_enabled);
        rrddim_set_by_pointer(rs, rd_disabled_ue, mls.num_machine_learning_status_disabled_ue);
        rrddim_set_by_pointer(rs, rd_disabled_sp, mls.num_machine_learning_status_disabled_sp);

        rrdset_done(rs);
    }

    /*
     * Metric type
    */
    {
        static thread_local RRDSET *rs = NULL;

        static thread_local RRDDIM *rd_constant = NULL;
        static thread_local RRDDIM *rd_variable = NULL;

        if (!rs) {
            char id_buf[1024];
            char name_buf[1024];

            snprintfz(id_buf, 1024, "metric_types_on_%s", localhost->machine_guid);
            snprintfz(name_buf, 1024, "metric_types_on_%s", rrdhost_hostname(localhost));

            rs = rrdset_create(
                    rh,
                    "netdata", // type
                    id_buf, // id
                    name_buf, // name
                    NETDATA_ML_CHART_FAMILY, // family
                    "netdata.metric_types", // ctx
                    "Dimensions by metric type", // title
                    "dimensions", // units
                    NETDATA_ML_PLUGIN, // plugin
                    NETDATA_ML_MODULE_TRAINING, // module
                    NETDATA_ML_CHART_PRIO_METRIC_TYPES, // priority
                    rh->rrd_update_every, // update_every
                    RRDSET_TYPE_LINE // chart_type
            );
            rrdset_flag_set(rs, RRDSET_FLAG_ANOMALY_DETECTION);

            rd_constant = rrddim_add(rs, "constant", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
            rd_variable = rrddim_add(rs, "variable", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
        }

        rrddim_set_by_pointer(rs, rd_constant, mls.num_metric_type_constant);
        rrddim_set_by_pointer(rs, rd_variable, mls.num_metric_type_variable);

        rrdset_done(rs);
    }

    /*
     * Training status
    */
    {
        static thread_local RRDSET *rs = NULL;

        static thread_local RRDDIM *rd_untrained = NULL;
        static thread_local RRDDIM *rd_pending_without_model = NULL;
        static thread_local RRDDIM *rd_trained = NULL;
        static thread_local RRDDIM *rd_pending_with_model = NULL;

        if (!rs) {
            char id_buf[1024];
            char name_buf[1024];

            snprintfz(id_buf, 1024, "training_status_on_%s", localhost->machine_guid);
            snprintfz(name_buf, 1024, "training_status_on_%s", rrdhost_hostname(localhost));

            rs = rrdset_create(
                    rh,
                    "netdata", // type
                    id_buf, // id
                    name_buf, // name
                    NETDATA_ML_CHART_FAMILY, // family
                    "netdata.training_status", // ctx
                    "Training status of dimensions", // title
                    "dimensions", // units
                    NETDATA_ML_PLUGIN, // plugin
                    NETDATA_ML_MODULE_TRAINING, // module
                    NETDATA_ML_CHART_PRIO_TRAINING_STATUS, // priority
                    rh->rrd_update_every, // update_every
                    RRDSET_TYPE_LINE // chart_type
            );

            rrdset_flag_set(rs, RRDSET_FLAG_ANOMALY_DETECTION);

            rd_untrained = rrddim_add(rs, "untrained", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
            rd_pending_without_model = rrddim_add(rs, "pending-without-model", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
            rd_trained = rrddim_add(rs, "trained", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
            rd_pending_with_model = rrddim_add(rs, "pending-with-model", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
        }

        rrddim_set_by_pointer(rs, rd_untrained, mls.num_training_status_untrained);
        rrddim_set_by_pointer(rs, rd_pending_without_model, mls.num_training_status_pending_without_model);
        rrddim_set_by_pointer(rs, rd_trained, mls.num_training_status_trained);
        rrddim_set_by_pointer(rs, rd_pending_with_model, mls.num_training_status_pending_with_model);

        rrdset_done(rs);
    }

    /*
     * Prediction status
    */
    {
        static thread_local RRDSET *rs = NULL;

        static thread_local RRDDIM *rd_anomalous = NULL;
        static thread_local RRDDIM *rd_normal = NULL;

        if (!rs) {
            char id_buf[1024];
            char name_buf[1024];

            snprintfz(id_buf, 1024, "dimensions_on_%s", localhost->machine_guid);
            snprintfz(name_buf, 1024, "dimensions_on_%s", rrdhost_hostname(localhost));

            rs = rrdset_create(
                    rh,
                    "anomaly_detection", // type
                    id_buf, // id
                    name_buf, // name
                    "dimensions", // family
                    "anomaly_detection.dimensions", // ctx
                    "Anomaly detection dimensions", // title
                    "dimensions", // units
                    NETDATA_ML_PLUGIN, // plugin
                    NETDATA_ML_MODULE_TRAINING, // module
                    ML_CHART_PRIO_DIMENSIONS, // priority
                    rh->rrd_update_every, // update_every
                    RRDSET_TYPE_LINE // chart_type
            );
            rrdset_flag_set(rs, RRDSET_FLAG_ANOMALY_DETECTION);

            rd_anomalous = rrddim_add(rs, "anomalous", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
            rd_normal = rrddim_add(rs, "normal", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
        }

        rrddim_set_by_pointer(rs, rd_anomalous, mls.num_anomalous_dimensions);
        rrddim_set_by_pointer(rs, rd_normal, mls.num_normal_dimensions);

        rrdset_done(rs);
    }

}

void nml_update_host_and_detection_rate_charts(RRDHOST *RH, collected_number AnomalyRate) {
    static thread_local RRDSET *rs_host_rate = NULL;
    static thread_local RRDDIM *rd_anomaly_rate = NULL;

    if (!rs_host_rate) {
        char id_buf[1024];
        char name_buf[1024];

        snprintfz(id_buf, 1024, "anomaly_rate_on_%s", localhost->machine_guid);
        snprintfz(name_buf, 1024, "anomaly_rate_on_%s", rrdhost_hostname(localhost));

        rs_host_rate = rrdset_create(
                RH,
                "anomaly_detection", // type
                id_buf, // id
                name_buf, // name
                "anomaly_rate", // family
                "anomaly_detection.anomaly_rate", // ctx
                "Percentage of anomalous dimensions", // title
                "percentage", // units
                NETDATA_ML_PLUGIN, // plugin
                NETDATA_ML_MODULE_DETECTION, // module
                ML_CHART_PRIO_ANOMALY_RATE, // priority
                RH->rrd_update_every, // update_every
                RRDSET_TYPE_LINE // chart_type
        );
        rrdset_flag_set(rs_host_rate, RRDSET_FLAG_ANOMALY_DETECTION);

        rd_anomaly_rate = rrddim_add(rs_host_rate, "anomaly_rate", NULL,
                1, 100, RRD_ALGORITHM_ABSOLUTE);
    }

    rrddim_set_by_pointer(rs_host_rate, rd_anomaly_rate, AnomalyRate);
    rrdset_done(rs_host_rate);

    static thread_local RRDSET *rs_anomaly_detection = NULL;
    static thread_local RRDDIM *rd_above_threshold = NULL;
    static thread_local RRDDIM *rd_new_anomaly_event = NULL;

    if (!rs_anomaly_detection) {
        char id_buf[1024];
        char name_buf[1024];

        snprintfz(id_buf, 1024, "anomaly_detection_on_%s", localhost->machine_guid);
        snprintfz(name_buf, 1024, "anomaly_detection_on_%s", rrdhost_hostname(localhost));

        rs_anomaly_detection = rrdset_create(
                RH,
                "anomaly_detection", // type
                id_buf, // id
                name_buf, // name
                "anomaly_detection", // family
                "anomaly_detection.detector_events", // ctx
                "Anomaly detection events", // title
                "percentage", // units
                NETDATA_ML_PLUGIN, // plugin
                NETDATA_ML_MODULE_DETECTION, // module
                ML_CHART_PRIO_DETECTOR_EVENTS, // priority
                RH->rrd_update_every, // update_every
                RRDSET_TYPE_LINE // chart_type
        );
        rrdset_flag_set(rs_anomaly_detection, RRDSET_FLAG_ANOMALY_DETECTION);

        rd_above_threshold  = rrddim_add(rs_anomaly_detection, "above_threshold", NULL,
                                       1, 1, RRD_ALGORITHM_ABSOLUTE);
        rd_new_anomaly_event = rrddim_add(rs_anomaly_detection, "new_anomaly_event", NULL,
                                       1, 1, RRD_ALGORITHM_ABSOLUTE);
    }

    /*
     * Compute the values of the dimensions based on the host rate chart
    */
    ONEWAYALLOC *OWA = onewayalloc_create(0);
    time_t Now = now_realtime_sec();
    time_t Before = Now - RH->rrd_update_every;
    time_t After = Before - Cfg.anomaly_detection_query_duration;
    RRDR_OPTIONS Options = static_cast<RRDR_OPTIONS>(0x00000000);

    RRDR *R = rrd2rrdr_legacy(
            OWA, rs_host_rate,
            1 /* points wanted */,
            After,
            Before,
            Cfg.anomaly_detection_grouping_method,
            0 /* resampling time */,
            Options, "anomaly_rate",
            NULL /* group options */,
            0, /* timeout */
            0, /* tier */
            QUERY_SOURCE_ML,
            STORAGE_PRIORITY_BEST_EFFORT
    );

    if(R) {
        if(R->d == 1 && R->n == 1 && R->rows == 1) {
            static thread_local bool prev_above_threshold = false;
            bool above_threshold = R->v[0] >= Cfg.host_anomaly_rate_threshold;
            bool new_anomaly_event = above_threshold && !prev_above_threshold;
            prev_above_threshold = above_threshold;

            rrddim_set_by_pointer(rs_anomaly_detection, rd_above_threshold, above_threshold);
            rrddim_set_by_pointer(rs_anomaly_detection, rd_new_anomaly_event, new_anomaly_event);
            rrdset_done(rs_anomaly_detection);
        }

        rrdr_free(OWA, R);
    }

    onewayalloc_destroy(OWA);
}

void nml_update_resource_usage_charts(RRDHOST *RH, const struct rusage &PredictionRU, const struct rusage &TrainingRU) {
    /*
     * prediction rusage
    */
    {
        static thread_local RRDSET *rs = NULL;

        static thread_local RRDDIM *rd_user = NULL;
        static thread_local RRDDIM *rd_system = NULL;

        if (!rs) {
            char id_buf[1024];
            char name_buf[1024];

            snprintfz(id_buf, 1024, "prediction_usage_for_%s", localhost->machine_guid);
            snprintfz(name_buf, 1024, "prediction_usage_for_%s", rrdhost_hostname(RH));

            rs = rrdset_create_localhost(
                    "netdata", // type
                    id_buf, // id
                    name_buf, // name
                    NETDATA_ML_CHART_FAMILY, // family
                    "netdata.prediction_usage", // ctx
                    "Prediction resource usage", // title
                    "milliseconds/s", // units
                    NETDATA_ML_PLUGIN, // plugin
                    NETDATA_ML_MODULE_PREDICTION, // module
                    NETDATA_ML_CHART_PRIO_PREDICTION_USAGE, // priority
                    RH->rrd_update_every, // update_every
                    RRDSET_TYPE_STACKED // chart_type
            );
            rrdset_flag_set(rs, RRDSET_FLAG_ANOMALY_DETECTION);

            rd_user = rrddim_add(rs, "user", NULL, 1, 1000, RRD_ALGORITHM_INCREMENTAL);
            rd_system = rrddim_add(rs, "system", NULL, 1, 1000, RRD_ALGORITHM_INCREMENTAL);
        }

        rrddim_set_by_pointer(rs, rd_user, PredictionRU.ru_utime.tv_sec * 1000000ULL + PredictionRU.ru_utime.tv_usec);
        rrddim_set_by_pointer(rs, rd_system, PredictionRU.ru_stime.tv_sec * 1000000ULL + PredictionRU.ru_stime.tv_usec);

        rrdset_done(rs);
    }

    /*
     * training rusage
    */
    {
        static thread_local RRDSET *rs = NULL;

        static thread_local RRDDIM *rd_user = NULL;
        static thread_local RRDDIM *rd_system = NULL;

        if (!rs) {
            char id_buf[1024];
            char name_buf[1024];

            snprintfz(id_buf, 1024, "training_usage_for_%s", localhost->machine_guid);
            snprintfz(name_buf, 1024, "training_usage_for_%s", rrdhost_hostname(RH));

            rs = rrdset_create_localhost(
                    "netdata", // type
                    id_buf, // id,
                    name_buf, // name
                    NETDATA_ML_CHART_FAMILY, // family
                    "netdata.training_usage", // ctx
                    "Training resource usage", // title
                    "milliseconds/s", // units
                    NETDATA_ML_PLUGIN, // plugin
                    NETDATA_ML_MODULE_TRAINING, // module
                    NETDATA_ML_CHART_PRIO_TRAINING_USAGE, // priority
                    RH->rrd_update_every, // update_every
                    RRDSET_TYPE_STACKED // chart_type
            );
            rrdset_flag_set(rs, RRDSET_FLAG_ANOMALY_DETECTION);

            rd_user = rrddim_add(rs, "user", NULL, 1, 1000, RRD_ALGORITHM_INCREMENTAL);
            rd_system = rrddim_add(rs, "system", NULL, 1, 1000, RRD_ALGORITHM_INCREMENTAL);
        }

        rrddim_set_by_pointer(rs, rd_user, TrainingRU.ru_utime.tv_sec * 1000000ULL + TrainingRU.ru_utime.tv_usec);
        rrddim_set_by_pointer(rs, rd_system, TrainingRU.ru_stime.tv_sec * 1000000ULL + TrainingRU.ru_stime.tv_usec);

        rrdset_done(rs);
    }
}

void nml_update_training_statistics_chart(RRDHOST *rh, const nml_training_stats_t &ts) {
    /*
     * queue stats
    */
    {
        static thread_local RRDSET *rs = NULL;

        static thread_local RRDDIM *rd_queue_size = NULL;
        static thread_local RRDDIM *rd_popped_items = NULL;

        if (!rs) {
            char id_buf[1024];
            char name_buf[1024];

            snprintfz(id_buf, 1024, "queue_stats_on_%s", localhost->machine_guid);
            snprintfz(name_buf, 1024, "queue_stats_on_%s", rrdhost_hostname(localhost));

            rs = rrdset_create(
                    rh,
                    "netdata", // type
                    id_buf, // id
                    name_buf, // name
                    NETDATA_ML_CHART_FAMILY, // family
                    "netdata.queue_stats", // ctx
                    "Training queue stats", // title
                    "items", // units
                    NETDATA_ML_PLUGIN, // plugin
                    NETDATA_ML_MODULE_TRAINING, // module
                    NETDATA_ML_CHART_PRIO_QUEUE_STATS, // priority
                    rh->rrd_update_every, // update_every
                    RRDSET_TYPE_LINE// chart_type
            );
            rrdset_flag_set(rs, RRDSET_FLAG_ANOMALY_DETECTION);

            rd_queue_size = rrddim_add(rs, "queue_size", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
            rd_popped_items = rrddim_add(rs, "popped_items", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
        }

        rrddim_set_by_pointer(rs, rd_queue_size, ts.queue_size);
        rrddim_set_by_pointer(rs, rd_popped_items, ts.num_popped_items);

        rrdset_done(rs);
    }

    /*
     * training stats
    */
    {
        static thread_local RRDSET *rs = NULL;

        static thread_local RRDDIM *rd_allotted = NULL;
        static thread_local RRDDIM *rd_consumed = NULL;
        static thread_local RRDDIM *rd_remaining = NULL;

        if (!rs) {
            char id_buf[1024];
            char name_buf[1024];

            snprintfz(id_buf, 1024, "training_time_stats_on_%s", localhost->machine_guid);
            snprintfz(name_buf, 1024, "training_time_stats_on_%s", rrdhost_hostname(localhost));

            rs = rrdset_create(
                    rh,
                    "netdata", // type
                    id_buf, // id
                    name_buf, // name
                    NETDATA_ML_CHART_FAMILY, // family
                    "netdata.training_time_stats", // ctx
                    "Training time stats", // title
                    "milliseconds", // units
                    NETDATA_ML_PLUGIN, // plugin
                    NETDATA_ML_MODULE_TRAINING, // module
                    NETDATA_ML_CHART_PRIO_TRAINING_TIME_STATS, // priority
                    rh->rrd_update_every, // update_every
                    RRDSET_TYPE_LINE// chart_type
            );
            rrdset_flag_set(rs, RRDSET_FLAG_ANOMALY_DETECTION);

            rd_allotted = rrddim_add(rs, "allotted", NULL, 1, 1000, RRD_ALGORITHM_ABSOLUTE);
            rd_consumed = rrddim_add(rs, "consumed", NULL, 1, 1000, RRD_ALGORITHM_ABSOLUTE);
            rd_remaining = rrddim_add(rs, "remaining", NULL, 1, 1000, RRD_ALGORITHM_ABSOLUTE);
        }

        rrddim_set_by_pointer(rs, rd_allotted, ts.allotted_ut);
        rrddim_set_by_pointer(rs, rd_consumed, ts.consumed_ut);
        rrddim_set_by_pointer(rs, rd_remaining, ts.remaining_ut);

        rrdset_done(rs);
    }

    /*
     * training result stats
    */
    {
        static thread_local RRDSET *rs = NULL;

        static thread_local RRDDIM *rd_ok = NULL;
        static thread_local RRDDIM *rd_invalid_query_time_range = NULL;
        static thread_local RRDDIM *rd_not_enough_collected_value = NULL;
        static thread_local RRDDIM *rd_null_acquired_dimension = NULL;
        static thread_local RRDDIM *rd_chart_under_replication = NULL;

        if (!rs) {
            char id_buf[1024];
            char name_buf[1024];

            snprintfz(id_buf, 1024, "training_results_on_%s", localhost->machine_guid);
            snprintfz(name_buf, 1024, "training_results_on_%s", rrdhost_hostname(localhost));

            rs = rrdset_create(
                    rh,
                    "netdata", // type
                    id_buf, // id
                    name_buf, // name
                    NETDATA_ML_CHART_FAMILY, // family
                    "netdata.training_results", // ctx
                    "Training results", // title
                    "events", // units
                    NETDATA_ML_PLUGIN, // plugin
                    NETDATA_ML_MODULE_TRAINING, // module
                    NETDATA_ML_CHART_PRIO_TRAINING_RESULTS, // priority
                    rh->rrd_update_every, // update_every
                    RRDSET_TYPE_LINE// chart_type
            );
            rrdset_flag_set(rs, RRDSET_FLAG_ANOMALY_DETECTION);

            rd_ok = rrddim_add(rs, "ok", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
            rd_invalid_query_time_range = rrddim_add(rs, "invalid-queries", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
            rd_not_enough_collected_value = rrddim_add(rs, "not-enough-values", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
            rd_null_acquired_dimension = rrddim_add(rs, "null-acquired-dimensions", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
            rd_chart_under_replication = rrddim_add(rs, "chart-under-replication", NULL, 1, 1, RRD_ALGORITHM_ABSOLUTE);
        }

        rrddim_set_by_pointer(rs, rd_ok, ts.training_result_ok);
        rrddim_set_by_pointer(rs, rd_invalid_query_time_range, ts.training_result_invalid_query_time_range);
        rrddim_set_by_pointer(rs, rd_not_enough_collected_value, ts.training_result_not_enough_collected_values);
        rrddim_set_by_pointer(rs, rd_null_acquired_dimension, ts.training_result_null_acquired_dimension);
        rrddim_set_by_pointer(rs, rd_chart_under_replication, ts.training_result_chart_under_replication);

        rrdset_done(rs);
    }
}
