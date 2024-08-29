#ifndef NETDATA_OTEL_ITERATOR_HPP
#define NETDATA_OTEL_ITERATOR_HPP

#include "otel_utils.hpp"
#include <fstream>

namespace otel
{
struct Element {
    const pb::MetricsData *MD;
    const pb::ResourceMetrics *RM;
    const pb::ScopeMetrics *SM;
    const pb::Metric *M;

    Element() : MD(nullptr), RM(nullptr), SM(nullptr), M(nullptr)
    {
    }

public:
    friend inline bool operator==(const Element &LHS, const Element &RHS)
    {
        return LHS.M == RHS.M;
    }
};

class Processor {
public:
    virtual void onResourceMetrics(const pb::ResourceMetrics &RMs) = 0;
    virtual void onScopeMetrics(const pb::ScopeMetrics &SMs) = 0;
    virtual void onMetric(const pb::Metric &M) = 0;
    virtual ~Processor() = default;
};

class FileProcessor : public Processor {
public:
    FileProcessor(const std::string &Path) : OS(Path) {}

    void onResourceMetrics(const pb::ResourceMetrics &RMs) override {
        if (!OS)
            return;

        OS << "RMs with " << RMs.scope_metrics_size() << " SMs\n";
    }

    void onScopeMetrics(const pb::ScopeMetrics &SMs) override {
        if (!OS)
            return;

        OS << "SMs with " << SMs.metrics_size() << " Ms\n";
    }

    void onMetric(const pb::Metric &M) override {
        if (!OS)
            return;

        OS << "------\n";
        OS << M.Utf8DebugString();
    }

    virtual ~FileProcessor() {
        OS.close();
    }

private:
    std::ofstream OS;
};


class Data {
    class Iterator {
    public:
        using ResourceMetricsIterator = typename pb::ConstFieldIterator<pb::ResourceMetrics>;
        using ScopeMetricsIterator = typename pb::ConstFieldIterator<pb::ScopeMetrics>;
        using MetricsIterator = typename pb::ConstFieldIterator<pb::Metric>;

    public:
        using iterator_category = std::input_iterator_tag;
        using value_type = Element;
        using difference_type = std::ptrdiff_t;
        using pointer = Element *;
        using reference = Element &;

    public:
        explicit Iterator(ResourceMetricsIterator RMBegin, ResourceMetricsIterator RMEnd, Processor &P)
            : RMIt(RMBegin), RMEnd(RMEnd), P(P), CurrElem()
        {
            if (RMIt != RMEnd) {
                SMIt = RMIt->scope_metrics().begin();
                SMEnd = RMIt->scope_metrics().end();

                if (SMIt != SMEnd) {
                    MIt = SMIt->metrics().begin();
                    MEnd = SMIt->metrics().end();
                }
            }

            CurrElem = next();
        }

        inline reference operator*()
        {
            return CurrElem;
        }

        inline pointer operator->()
        {
            return &CurrElem;
        }

        // Pre-increment operator
        Iterator &operator++()
        {
            CurrElem = next();
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
            return CurrElem == Other.CurrElem;
        }

        inline bool operator!=(const Iterator &Other) const
        {
            return !(*this == Other);
        }

    private:
        inline bool hasNext() const
        {
            if (RMIt == RMEnd)
                return false;

            if (SMIt == SMEnd)
                return false;

            return MIt != MEnd;
        }

        Element next()
        {
            if (!hasNext()) {
                return Element();
            }

            // Fill element
            Element NewElem;
            {
                NewElem.RM = &*RMIt;
                NewElem.SM = &*SMIt;
                NewElem.M = &*MIt;
            }

            if (NewElem.RM != CurrElem.RM) {
                P.onResourceMetrics(*NewElem.RM);
            }

            if (NewElem.SM != CurrElem.SM) {
                P.onScopeMetrics(*NewElem.SM);
            }

            if (NewElem.M != CurrElem.M) {
                P.onMetric(*NewElem.M);
            }

            advanceIterators();
            return NewElem;
        }

        void advanceIterators()
        {
            if (++MIt != MEnd) {
                return;
            }

            if (++SMIt != SMEnd) {
                MIt = SMIt->metrics().begin();
                MEnd = SMIt->metrics().end();
                return;
            }

            if (++RMIt != RMEnd) {
                SMIt = RMIt->scope_metrics().begin();
                SMEnd = RMIt->scope_metrics().end();

                if (SMIt != SMEnd) {
                    MIt = SMIt->metrics().begin();
                    MEnd = SMIt->metrics().end();
                }

                return;
            }
        }

    private:
        ResourceMetricsIterator RMIt, RMEnd;
        ScopeMetricsIterator SMIt, SMEnd;
        MetricsIterator MIt, MEnd;
        Processor &P;

        Element CurrElem;
    };

public:
    Data(const pb::MetricsData &MD, Processor &P) : MD(MD), P(P)
    {
    }

    inline Iterator begin()
    {
        return Iterator(MD.resource_metrics().begin(), MD.resource_metrics().end(), P);
    }

    inline Iterator end() const
    {
        return Iterator(MD.resource_metrics().end(), MD.resource_metrics().end(), P);
    }

private:
    const pb::MetricsData &MD;
    Processor &P;
};

} // namespace otel

#endif /* NETDATA_OTEL_ITERATOR_HPP */
