#ifndef NETDATA_OTEL_ITERATOR_H
#define NETDATA_OTEL_ITERATOR_H

#include "libnetdata/blake3/blake3.h"
#include "otel_utils.h"

#include "otel_config.h"
#include "fmt_utils.h"
#include "otel_hash.h"

#include "absl/status/status.h"
#include "absl/status/statusor.h"

using KeyValueArray = google::protobuf::RepeatedPtrField<pb::KeyValue>;

enum class DataPointKind {
    Number,
    Sum,
    Summary,
    Histogram,
    Exponential,
    NotAvailable,
};

class DataPoint {
public:
    static const std::string DefaultDimensionName;

public:
    DataPoint() : DpKind(DataPointKind::NotAvailable)
    {
    }

    DataPoint(const pb::NumberDataPoint *NDP) : DpKind(DataPointKind::Number), NDP(NDP)
    {
    }

    DataPoint(const pb::SummaryDataPoint *SDP) : DpKind(DataPointKind::Summary), SDP(SDP)
    {
    }

    DataPoint(const pb::HistogramDataPoint *HDP) : DpKind(DataPointKind::Histogram), HDP(HDP)
    {
    }

    DataPoint(const pb::ExponentialHistogramDataPoint *EHDP) : DpKind(DataPointKind::Exponential), EHDP(EHDP)
    {
    }

    inline DataPointKind kind() const
    {
        return DpKind;
    }

    inline const pb::NumberDataPoint *ndp() const
    {
        assert(DpKind == DataPointKind::Number || DpKind == DataPointKind::Sum);
        return NDP;
    }

    inline const pb::SummaryDataPoint *sdp() const
    {
        assert(DpKind == DataPointKind::Summary);
        return SDP;
    }

    inline absl::StatusOr<const pb::AnyValue *> attribute(const std::string *Key) const
    {
        const KeyValueArray *KVA = getAttrs();
        if (!KVA) {
            return absl::NotFoundError("DataPoint has no attributes");
        }

        for (const auto &KV : *KVA) {
            if (KV.key() != *Key)
                continue;

            if (!KV.has_value())
                return absl::NotFoundError("Datapoint key has no value");

            return &KV.value();
        }

        return absl::NotFoundError(absl::StrFormat("data point %s key not found", Key->c_str()));
    }

    uint64_t time() const
    {
        switch (DpKind) {
            case DataPointKind::Number:
            case DataPointKind::Sum:
                return NDP->time_unix_nano();
            case DataPointKind::Summary:
                return SDP->time_unix_nano();
            case DataPointKind::Histogram:
                return HDP->time_unix_nano();
            case DataPointKind::Exponential:
                return EHDP->time_unix_nano();
            case DataPointKind::NotAvailable:
                return 0;
        }

        return 0;
    }

    uint64_t value(uint64_t Multiplier) const
    {
        switch (DpKind) {
            case DataPointKind::Number:
            case DataPointKind::Sum:
                if (NDP->has_as_double())
                    return NDP->as_double() * Multiplier;
                else
                    return NDP->as_int() * Multiplier;
            case DataPointKind::Summary:
            case DataPointKind::Histogram:
            case DataPointKind::Exponential:
            case DataPointKind::NotAvailable:
                return 0;
        }
    }

    friend inline bool operator==(const DataPoint &LHS, const DataPoint &RHS)
    {
        return LHS.time() == RHS.time() && LHS.getAttrs() == RHS.getAttrs();
    }

    friend inline bool operator<(const DataPoint &LHS, const DataPoint &RHS)
    {
        if (LHS.time() != RHS.time()) {
            return LHS.time() < RHS.time();
        }

        // FIXME: we should perform a value-based comparison
        return LHS.getAttrs() < RHS.getAttrs();
    }

    absl::Nullable<const KeyValueArray *> getAttrs() const
    {
        switch (DpKind) {
            case DataPointKind::Number:
            case DataPointKind::Sum:
                return &NDP->attributes();
            case DataPointKind::Summary:
                return &SDP->attributes();
            case DataPointKind::Histogram:
                return &HDP->attributes();
            case DataPointKind::Exponential:
                return &EHDP->attributes();
            default:
                return nullptr;
        }
    }

private:
    DataPointKind DpKind;

    union {
        const pb::NumberDataPoint *NDP;
        const pb::SummaryDataPoint *SDP;
        const pb::HistogramDataPoint *HDP;
        const pb::ExponentialHistogramDataPoint *EHDP;
    };
};

template <> struct fmt::formatter<DataPoint> {
    constexpr auto parse(format_parse_context &Ctx) -> decltype(Ctx.begin())
    {
        return Ctx.end();
    }

    template <typename FormatContext> auto format(const DataPoint &DP, FormatContext &Ctx) const -> decltype(Ctx.out())
    {
        switch (DP.kind()) {
            case DataPointKind::Number:
            case DataPointKind::Sum:
                return fmt::format_to(Ctx.out(), "{}", *DP.ndp());
            default:
                return fmt::format_to(Ctx.out(), "<unknown-dp>");
        }
    }
};

struct OtelElement {
    const pb::ResourceMetrics *RM;
    const pb::ScopeMetrics *SM;
    const pb::Metric *M;
    DataPoint DP;

    absl::Nullable<const std::string *> DimAttr;
    absl::Nullable<const std::vector<std::string> *> InstanceAttrs;

    OtelElement() : RM(nullptr), SM(nullptr), M(nullptr), DP(), DimAttr(nullptr), InstanceAttrs(nullptr)
    {
    }

public:
    const absl::StatusOr<const std::string *> name() const {
        static std::string DefaultName = "value";

        if (!DimAttr) {
            return &DefaultName;
        }

        auto Res = DP.attribute(DimAttr);
        if (!Res.ok()) {
            return Res.status();
        }

        const pb::AnyValue *AV = *Res;
        if (!AV->has_string_value()) {
            return absl::InvalidArgumentError(absl::StrFormat("data point %s key contains a non-string value", *DimAttr));
        }

        return &AV->string_value();
    }

    inline uint64_t time() const
    {
        return DP.time();
    }

    inline uint64_t value(uint64_t multiplier) const
    {
        return DP.value(multiplier);
    }

    inline bool monotonic() const
    {
        if (!M->has_sum())
            return false;

        return M->sum().is_monotonic();
    }

    friend inline bool operator==(const OtelElement &LHS, const OtelElement &RHS)
    {
        return LHS.RM == RHS.RM && LHS.SM == RHS.SM && LHS.M == RHS.M && LHS.DP == RHS.DP;
    }

    friend inline bool operator<(const OtelElement &LHS, const OtelElement &RHS)
    {
        if (LHS.RM != RHS.RM) {
            return std::less<const pb::ResourceMetrics *>()(LHS.RM, RHS.RM);
        }

        if (LHS.SM != RHS.SM) {
            return std::less<const pb::ScopeMetrics *>()(LHS.SM, RHS.SM);
        }

        if (LHS.M != RHS.M) {
            return std::less<const pb::Metric *>()(LHS.M, RHS.M);
        }

        return LHS.DP < RHS.DP;
    }

    BlakeId chartHash() const
    {
        blake3_hasher H;
        blake3_hasher_init(&H);

        if (RM->has_resource()) {
            otel::hashResource(H, RM->resource());
        }

        if (SM->has_scope()) {
            otel::hashInstrumentationScope(H, SM->scope());
        }

        otel::hashMetric(H, *M);

        // Hash all the data point attributes except for the one used for the
        // dimension name
        if (auto *Attrs = DP.getAttrs()) {
            for (const auto &KV : *Attrs) {
                if (!DimAttr || KV.key() != *DimAttr) {
                    otel::hashKeyValue(H, KV);
                }
            }
        }

        BlakeId BID;
        blake3_hasher_finalize(&H, BID.data(), BID.size());
        return BID;
    }
};

template <> struct fmt::formatter<OtelElement> {
    constexpr auto parse(format_parse_context &Ctx) -> decltype(Ctx.begin())
    {
        return Ctx.end();
    }

    template <typename FormatContext>
    auto format(const OtelElement &OE, FormatContext &Ctx) const -> decltype(Ctx.out())
    {
        fmt::format_to(Ctx.out(), "OtelElement{{");

        if (OE.RM->has_resource()) {
            fmt::format_to(Ctx.out(), "resource: {}", OE.RM->resource());
        }

        if (!OE.RM->schema_url().empty()) {
            fmt::format_to(Ctx.out(), ", resource_url: {}", OE.RM->schema_url());
        }

        if (OE.SM->has_scope()) {
            fmt::format_to(Ctx.out(), ", instrumentation_scope: {}", OE.SM->scope());
        }

        if (!OE.SM->schema_url().empty()) {
            fmt::format_to(Ctx.out(), ", scope_url: {}", OE.SM->schema_url());
        }

        fmt::format_to(Ctx.out(), ", name: {}", OE.M->name());
        fmt::format_to(Ctx.out(), ", description: {}", OE.M->description());
        fmt::format_to(Ctx.out(), ", point: {}", OE.DP);

        return Ctx.out();
    }
};

class OtelData {
    class Iterator {
    public:
        using ResourceMetricsIterator = typename pb::ConstFieldIterator<pb::ResourceMetrics>;
        using ScopeMetricsIterator = typename pb::ConstFieldIterator<pb::ScopeMetrics>;
        using MetricsIterator = typename pb::ConstFieldIterator<pb::Metric>;

        using NumberDataPointIterator = typename pb::ConstFieldIterator<pb::NumberDataPoint>;
        using SummaryDataPointIterator = typename pb::ConstFieldIterator<pb::SummaryDataPoint>;
        using HistogramDataPointIterator = typename pb::ConstFieldIterator<pb::HistogramDataPoint>;
        using ExponentialHistogramDataPointIterator =
            typename pb::ConstFieldIterator<pb::ExponentialHistogramDataPoint>;

        union DataPointIterator {
            NumberDataPointIterator NDPIt;
            SummaryDataPointIterator SDPIt;
            HistogramDataPointIterator HDPIt;
            ExponentialHistogramDataPointIterator EHDPIt;

            DataPointIterator()
            {
            }

            ~DataPointIterator()
            {
            }
        };

    public:
        using iterator_category = std::input_iterator_tag;
        using value_type = OtelElement;
        using difference_type = std::ptrdiff_t;
        using pointer = OtelElement *;
        using reference = OtelElement &;

    public:
        explicit Iterator(otel::Config *Cfg, const pb::RepeatedPtrField<pb::ResourceMetrics> *RPF)
            : Cfg(Cfg), DPKind(DataPointKind::NotAvailable), End(true)
        {
            if (RPF) {
                RMIt = RPF->begin();
                RMEnd = RPF->end();

                if (RMIt != RMEnd) {
                    SMIt = RMIt->scope_metrics().begin();
                    SMEnd = RMIt->scope_metrics().end();

                    if (SMIt != SMEnd) {
                        MIt = SMIt->metrics().begin();
                        MEnd = SMIt->metrics().end();

                        if (MIt != MEnd) {
                            const pb::Metric &M = *MIt;
                            initializeDataPointIterator(M);
                            End = false;
                        }
                    }
                }
            }

            if (!End) {
                CurrOE = next();
            }
        }

        ~Iterator()
        {
            destroyCurrentIterator();
        }

        inline reference operator*()
        {
            return CurrOE;
        }

        inline pointer operator->()
        {
            return &CurrOE;
        }

        // Pre-increment operator
        Iterator &operator++()
        {
            if (hasNext()) {
                CurrOE = next();
            } else {
                End = true;
            }
            return *this;
        }

        // Post-increment operator
        inline Iterator operator++(int)
        {
            Iterator Tmp = *this;
            ++(*this);
            return Tmp;
        }

        inline bool operator==(const Iterator &Other) const
        {
            if (!End && !Other.End) {
                return CurrOE == Other.CurrOE;
            }

            return End == Other.End;
        }

        inline bool operator!=(const Iterator &Other) const
        {
            return !(*this == Other);
        }

    private:
        inline DataPointKind dpKind(const pb::Metric &M) const
        {
            if (M.has_gauge())
                return DataPointKind::Number;
            if (M.has_sum())
                return DataPointKind::Sum;
            if (M.has_summary())
                return DataPointKind::Summary;
            else if (M.has_histogram())
                return DataPointKind::Histogram;
            else if (M.has_exponential_histogram())
                return DataPointKind::Exponential;

            throw std::out_of_range("Unknown data point kind");
        }

        inline bool hasNext() const
        {
            if (RMIt == RMEnd)
                return false;

            if (SMIt == SMEnd)
                return false;

            if (MIt == MEnd)
                return false;

            switch (DPKind) {
                case DataPointKind::Number:
                case DataPointKind::Sum:
                    return DPIt.NDPIt != DPEnd.NDPIt;
                case DataPointKind::Summary:
                    return DPIt.SDPIt != DPEnd.SDPIt;
                case DataPointKind::Histogram:
                    return DPIt.HDPIt != DPEnd.HDPIt;
                case DataPointKind::Exponential:
                    return DPIt.EHDPIt != DPEnd.EHDPIt;
                case DataPointKind::NotAvailable:
                    return false;
                default:
                    throw std::out_of_range("WTF?");
            }
        }

        OtelElement next()
        {
            if (!hasNext())
                throw std::out_of_range("No more elements");

            // Fill element
            OtelElement OE;
            {
                OE.RM = &*RMIt;
                OE.SM = &*SMIt;
                OE.M = &*MIt;

                switch (DPKind) {
                    case DataPointKind::Number:
                    case DataPointKind::Sum:
                        OE.DP = DataPoint(&*DPIt.NDPIt);
                        break;
                    case DataPointKind::Summary:
                        OE.DP = DataPoint(&*DPIt.SDPIt);
                        break;
                    case DataPointKind::Histogram:
                        OE.DP = DataPoint(&*DPIt.HDPIt);
                        break;
                    case DataPointKind::Exponential:
                        OE.DP = DataPoint(&*DPIt.EHDPIt);
                        break;
                    case DataPointKind::NotAvailable:
                    default:
                        throw std::out_of_range("No more elements");
                }
            }

            const auto *MetricCfg = Cfg->getMetric(OE.SM->scope().name(), OE.M->name());
            if (MetricCfg) {
                OE.DimAttr = MetricCfg->getDimensionsAttribute();
                OE.InstanceAttrs = MetricCfg->getInstanceAttributes();
            } else {
                OE.DimAttr = nullptr;
                OE.InstanceAttrs = nullptr;
            }

            advanceIterators();
            return OE;
        }

        inline void destroyCurrentIterator()
        {
            switch (DPKind) {
                case DataPointKind::Number:
                case DataPointKind::Sum:
                    DPIt.NDPIt.~NumberDataPointIterator();
                    DPEnd.NDPIt.~NumberDataPointIterator();
                    break;
                case DataPointKind::Summary:
                    DPIt.SDPIt.~SummaryDataPointIterator();
                    DPEnd.SDPIt.~SummaryDataPointIterator();
                    break;
                case DataPointKind::Histogram:
                    DPIt.HDPIt.~HistogramDataPointIterator();
                    DPEnd.HDPIt.~HistogramDataPointIterator();
                    break;
                case DataPointKind::Exponential:
                    DPIt.EHDPIt.~ExponentialHistogramDataPointIterator();
                    DPEnd.EHDPIt.~ExponentialHistogramDataPointIterator();
                    break;
                case DataPointKind::NotAvailable:
                    break;
                default:
                    throw std::out_of_range("Unknown data point kind");
            }
        }

        void initializeDataPointIterator(const pb::Metric &M)
        {
            DPKind = dpKind(M);

            switch (DPKind) {
                case DataPointKind::Number:
                    DPIt.NDPIt = M.gauge().data_points().begin();
                    DPEnd.NDPIt = M.gauge().data_points().end();
                    break;
                case DataPointKind::Sum:
                    DPIt.NDPIt = M.sum().data_points().begin();
                    DPEnd.NDPIt = M.sum().data_points().end();
                    break;
                case DataPointKind::Summary:
                    DPIt.SDPIt = M.summary().data_points().begin();
                    DPEnd.SDPIt = M.summary().data_points().end();
                    break;
                case DataPointKind::Histogram:
                    DPIt.HDPIt = M.histogram().data_points().begin();
                    DPEnd.HDPIt = M.histogram().data_points().end();
                    break;
                case DataPointKind::Exponential:
                    DPIt.EHDPIt = M.exponential_histogram().data_points().begin();
                    DPEnd.EHDPIt = M.exponential_histogram().data_points().end();
                    break;
                default:
                    throw std::out_of_range("WTF?");
            }
        }

        void advanceIterators()
        {
            switch (DPKind) {
                case DataPointKind::Number:
                case DataPointKind::Sum:
                    if (++DPIt.NDPIt != DPEnd.NDPIt)
                        return;
                    break;
                case DataPointKind::Summary:
                    if (++DPIt.SDPIt != DPEnd.SDPIt)
                        return;
                    break;
                case DataPointKind::Histogram:
                    if (++DPIt.HDPIt != DPEnd.HDPIt)
                        return;
                    break;
                case DataPointKind::Exponential:
                    if (++DPIt.EHDPIt != DPEnd.EHDPIt)
                        return;
                    break;
                case DataPointKind::NotAvailable:
                    // FIXME: What should we do here?
                    break;
            }

            if (++MIt != MEnd) {
                initializeDataPointIterator(*MIt);
                return;
            }

            if (++SMIt != SMEnd) {
                MIt = SMIt->metrics().begin();
                MEnd = SMIt->metrics().end();

                if (MIt != MEnd)
                    initializeDataPointIterator(*MIt);

                return;
            }

            if (++RMIt != RMEnd) {
                SMIt = RMIt->scope_metrics().begin();
                SMEnd = RMIt->scope_metrics().end();

                if (SMIt != SMEnd) {
                    MIt = SMIt->metrics().begin();
                    MEnd = SMIt->metrics().end();

                    if (MIt != MEnd)
                        initializeDataPointIterator(*MIt);
                }

                return;
            }
        }

    private:
        otel::Config *Cfg;
        ResourceMetricsIterator RMIt, RMEnd;
        ScopeMetricsIterator SMIt, SMEnd;
        MetricsIterator MIt, MEnd;

        DataPointIterator DPIt, DPEnd;
        DataPointKind DPKind;

        OtelElement CurrOE;
        bool End;
    };

public:
    OtelData(otel::Config *Cfg, const pb::RepeatedPtrField<pb::ResourceMetrics> *RPF) : Cfg(Cfg), RPF(RPF)
    {
    }

    inline Iterator begin()
    {
        return Iterator(Cfg, RPF);
    }

    inline Iterator end() const
    {
        return Iterator(Cfg, nullptr);
    }

private:
    otel::Config *Cfg;
    const pb::RepeatedPtrField<pb::ResourceMetrics> *RPF;
};

#endif /* NETDATA_OTEL_ITERATOR_H */