#ifndef NETDATA_OTEL_TRANSFORM_HPP
#define NETDATA_OTEL_TRANSFORM_HPP

#include "otel_utils.hpp"
#include "otel_config.hpp"

namespace otel
{
void transformMetrics(const ScopeConfig *ScopeCfg, pb::RepeatedPtrField<pb::Metric> *RPF);
} // namespace otel

#endif /* NETDATA_OTEL_TRANSFORM_HPP */
