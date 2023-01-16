// SPDX-License-Identifier: GPL-3.0-or-later

#include "nml.h"

/*
 * Global configuration instance to be shared between training and
 * prediction threads.
 */
Config Cfg;

template <typename T>
static T clamp(const T& Value, const T& Min, const T& Max) {
  return std::max(Min, std::min(Value, Max));
}

/*
 * Initialize global configuration variable.
 */
void Config::readMLConfig(void) {
    const char *config_section_ml = CONFIG_SECTION_ML;

    bool enable_anomaly_detection = config_get_boolean(config_section_ml, "enabled", true);

    /*
     * Read values
     */

    unsigned max_train_samples = config_get_number(config_section_ml, "maximum num samples to train", 4 * 3600);
    unsigned min_train_samples = config_get_number(config_section_ml, "minimum num samples to train", 1 * 900);
    unsigned train_every = config_get_number(config_section_ml, "train every", 1 * 3600);
    unsigned num_models_to_use = config_get_number(config_section_ml, "number of models per dimension", 1);

    unsigned diff_n = config_get_number(config_section_ml, "num samples to diff", 1);
    unsigned smooth_n = config_get_number(config_section_ml, "num samples to smooth", 3);
    unsigned lag_n = config_get_number(config_section_ml, "num samples to lag", 5);

    double random_sampling_ratio = config_get_float(config_section_ml, "random sampling ratio", 1.0 / lag_n);
    unsigned max_kmeans_iters = config_get_number(config_section_ml, "maximum number of k-means iterations", 1000);

    double dimension_anomaly_rate_threshold = config_get_float(config_section_ml, "dimension anomaly score threshold", 0.99);

    double host_anomaly_rate_threshold = config_get_float(config_section_ml, "host anomaly rate threshold", 1.0);
    std::string anomaly_detection_grouping_method = config_get(config_section_ml, "anomaly detection grouping method", "average");
    time_t anomaly_detection_query_duration = config_get_number(config_section_ml, "anomaly detection grouping duration", 5 * 60);

    /*
     * Clamp
     */

    max_train_samples = clamp<unsigned>(max_train_samples, 1 * 3600, 24 * 3600);
    min_train_samples = clamp<unsigned>(min_train_samples, 1 * 900, 6 * 3600);
    train_every = clamp<unsigned>(train_every, 1 * 3600, 6 * 3600);
    num_models_to_use = clamp<unsigned>(num_models_to_use, 1, 7 * 24);

    diff_n = clamp(diff_n, 0u, 1u);
    smooth_n = clamp(smooth_n, 0u, 5u);
    lag_n = clamp(lag_n, 1u, 5u);

    random_sampling_ratio = clamp(random_sampling_ratio, 0.2, 1.0);
    max_kmeans_iters = clamp(max_kmeans_iters, 500u, 1000u);

    dimension_anomaly_rate_threshold = clamp(dimension_anomaly_rate_threshold, 0.01, 5.00);

    host_anomaly_rate_threshold = clamp(host_anomaly_rate_threshold, 0.1, 10.0);
    anomaly_detection_query_duration = clamp<time_t>(anomaly_detection_query_duration, 60, 15 * 60);

    /*
     * Validate
     */

    if (min_train_samples >= max_train_samples) {
        error("invalid min/max train samples found (%u >= %u)", min_train_samples, max_train_samples);

        min_train_samples = 1 * 3600;
        max_train_samples = 4 * 3600;
    }

    /*
     * Assign to config instance
     */

    Cfg.enable_anomaly_detection = enable_anomaly_detection;

    Cfg.max_train_samples = 60;
    Cfg.min_train_samples = 30;
    Cfg.train_every = 60;
    Cfg.num_models_to_use = num_models_to_use;

    Cfg.diff_n = diff_n;
    Cfg.smooth_n = smooth_n;
    Cfg.lag_n = lag_n;

    Cfg.random_sampling_ratio = random_sampling_ratio;
    Cfg.max_kmeans_iters = max_kmeans_iters;

    Cfg.dimension_anomaly_score_threshold = dimension_anomaly_rate_threshold;

    Cfg.host_anomaly_rate_threshold = host_anomaly_rate_threshold;
    Cfg.anomaly_detection_grouping_method = web_client_api_request_v1_data_group(anomaly_detection_grouping_method.c_str(), RRDR_GROUPING_AVERAGE);
    Cfg.anomaly_detection_query_duration = anomaly_detection_query_duration;

    Cfg.hosts_to_skip = config_get(config_section_ml, "hosts to skip from training", "!*");
    Cfg.sp_host_to_skip = simple_pattern_create(Cfg.hosts_to_skip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    // Always exclude anomaly_detection charts from training.
 #if 0
    Cfg.charts_to_skip = "anomaly_detection.* ";
    Cfg.charts_to_skip += config_get(ConfigSectionML, "charts to skip from training", "netdata.*");
 #else
    Cfg.charts_to_skip = "!profile.* *";
 #endif
    Cfg.sp_charts_to_skip = simple_pattern_create(Cfg.charts_to_skip.c_str(), NULL, SIMPLE_PATTERN_EXACT);

    Cfg.stream_anomaly_detection_charts = config_get_boolean(config_section_ml, "stream anomaly detection charts", true);
}
