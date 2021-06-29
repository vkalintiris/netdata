#ifndef ML_CHART_H
#define ML_CHART_H

#include "ml-private.h"

namespace ml {

class Dimension;

class Chart {
public:
    Chart(RRDSET *RS) : RS(RS) {}

    RRDSET *getRS() const { return RS; }
    const char *getName() const { return RS->name; }

    void addDimension(Dimension *D);
    void removeDimension(Dimension *D);
    bool forEachDimension(std::function<bool(Dimension *)> Func);

    void updateMLChart();

public:
    RRDSET *RS;
    RRDSET *MLRS{nullptr};

    std::mutex Mutex;
    std::map<RRDDIM *, Dimension *> DimensionsMap;
};

} // namespace ml

#endif /* ML_CHART_H */
