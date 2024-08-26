#ifndef NETDATA_OTEL_FLATTEN_HPP
#define NETDATA_OTEL_FLATTEN_HPP

#include "otel_utils.hpp"

namespace pb
{

void flattenAttributes(
    pb::Arena *A,
    const std::string &Prefix,
    const pb::KeyValue &KV,
    pb::RepeatedPtrField<pb::KeyValue> *RPF);

} // namespace pb

#endif /* NETDATA_OTEL_FLATTEN_HPP */
