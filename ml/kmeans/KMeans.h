#ifndef KMEANS_H
#define KMEANS_H

#include <vector>
#include <limits>
#include <dlib/matrix.h>

typedef long double CalculatedNumber;
typedef dlib::matrix<CalculatedNumber, 0, 1> DSample;

class KMeans {
public:
    KMeans(size_t NumClusters = 2) : NumClusters(NumClusters) {
        MinDist = std::numeric_limits<CalculatedNumber>::max();
        MaxDist = std::numeric_limits<CalculatedNumber>::min();
    };

    void train(std::vector<DSample> &Samples);
    CalculatedNumber anomalyScore(DSample &Sample);

private:
    size_t NumClusters;

    std::vector<DSample> ClusterCenters;
    CalculatedNumber MinDist;
    CalculatedNumber MaxDist;
};

#endif /* KMEANS_H */
