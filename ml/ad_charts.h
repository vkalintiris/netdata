// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_ADCHARTS_H
#define ML_ADCHARTS_H

#include "nml.h"

void nml_update_dimensions_chart(RRDHOST *rh, const nml_machine_learning_stats_t &mls);

void nml_update_host_and_detection_rate_charts(RRDHOST *rh, collected_number anomaly_rate);

void nml_update_resource_usage_charts(RRDHOST *rh, const struct rusage &prediction_ru, const struct rusage &training_ru);

void nml_update_training_statistics_chart(RRDHOST *rh, const nml_training_stats_t &ts);

#endif /* ML_ADCHARTS_H */
