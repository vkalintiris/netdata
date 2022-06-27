#ifndef REPLICATION_GAPDATA_H
#define REPLICATION_GAPDATA_H

#include "replication-private.h"

namespace replication {

class GapData {
public:
    static GapData fromBase64(const std::string &EncodedData);

public:
    std::string getChart() const {
        return Chart;
    }

    void setChart(std::string Name) {
        Chart = Name;
    }

    std::string getDimension() const {
        return Dimension;
    }

    void setDimension(std::string Name) {
        Dimension = Name;
    }

    std::vector<std::pair<time_t, storage_number>> getStorageNumbers() const {
        return StorageNumbers;
    }

    void setStorageNumbers(std::vector<std::pair<time_t, storage_number>> SNs) {
        StorageNumbers = SNs;
    }

    std::pair<size_t, TimeRange> getTimeRangeSpan() const {
        if (StorageNumbers.size() == 0)
            return { 0, TimeRange(0, 0) };

        return { StorageNumbers.size(), TimeRange(StorageNumbers.front().first, StorageNumbers.back().first) };
    }

    void print(RRDHOST *RH) const;

    bool push(struct sender_state *sender) const;

    std::string toBase64() const;

    bool flushToDBEngine(RRDHOST *RH) const;

private:
    pb::GapData toProto() const;
    static GapData fromProto(const pb::GapData &PGD);

    std::vector<TimeRange> getTimeRanges() const;

private:
    std::string Chart;
    std::string Dimension;
    std::vector<std::pair<time_t, storage_number>> StorageNumbers;
};

} // namespace replication

#endif /* REPLICATION_GAPDATA_H */
