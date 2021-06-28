#ifndef ML_CHART_H
#define ML_CHART_H

#include "ml-private.h"

namespace ml {

class Dimension;

class Chart {
public:
    Chart(RRDSET *RS) : RS(RS), MLRS(nullptr) {}

    RRDSET *getRS() const { return RS; }

    void addDimension(Dimension *D);
    void removeDimension(Dimension *D);

    void updateMLChart();

public:
    RRDSET *RS;
    RRDSET *MLRS;

    std::map<RRDDIM *, Dimension *> DimensionsMap;
};

} // namespace ml

#endif /* ML_CHART_H */
