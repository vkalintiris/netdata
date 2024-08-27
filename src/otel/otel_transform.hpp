#ifndef NETDATA_OTEL_TRANSFORM_HPP
#define NETDATA_OTEL_TRANSFORM_HPP

#include "otel_utils.hpp"
#include "otel_config.hpp"

namespace otel
{
void transformMetricData(const Config *Cfg, pb::MetricsData &MD);
} // namespace otel

#endif /* NETDATA_OTEL_TRANSFORM_HPP */
