#include "KMeans.h"
#include <dlib/clustering.h>

void KMeans::train(std::vector<DSample> &Samples) {
    dlib::pick_initial_centers(NumClusters, ClusterCenters, Samples);
    dlib::find_clusters_using_kmeans(Samples, ClusterCenters);

    for (const auto &S : Samples) {
        CalculatedNumber MeanDist = 0.0L;

        for (const auto &KMCenter : ClusterCenters)
            MeanDist += dlib::length(KMCenter - S);

        MeanDist /= NumClusters;

        if (MeanDist < MinDist)
            MinDist = MeanDist;

        if (MeanDist > MaxDist)
            MaxDist = MeanDist;
    }
}

CalculatedNumber KMeans::anomalyScore(DSample &Sample) {
    CalculatedNumber MeanDist = 0.0L;

    for (const auto &CC: ClusterCenters)
        MeanDist += dlib::length(CC - Sample);

    MeanDist /= NumClusters;

    // TODO: Make sure we don't divide by zero
    return (MeanDist - MinDist) / (MaxDist - MinDist);
}

/*
 * C <-> C++ API stubs
*/
#include "kmeans-c.h"
#include "SamplesBuffer.h"

extern "C" kmeans_ref
kmeans_new(size_t num_centers) {
    return reinterpret_cast<KMeans*>(new KMeans(num_centers));
}

extern "C" void
kmeans_train(kmeans_ref km_ref, calculated_number *calc_nums,
             size_t num_samples, size_t num_dims_per_sample,
             size_t diff_n, size_t smooth_n, size_t lag_n) {
    KMeans *KM = reinterpret_cast<KMeans *>(km_ref);

    SamplesBuffer SB = SamplesBuffer(calc_nums,
                                     num_samples, num_dims_per_sample,
                                     diff_n, smooth_n, lag_n);
    std::vector<DSample> DSamples = SB.preprocess();
    KM->train(DSamples);
}

extern "C" calculated_number
kmeans_anomaly_score(kmeans_ref km_ref, calculated_number *calc_nums,
                     size_t num_samples, size_t num_dims_per_sample,
                     size_t diff_n, size_t smooth_n, size_t lag_n) {
    KMeans *KM = reinterpret_cast<KMeans *>(km_ref);

    SamplesBuffer SB = SamplesBuffer(calc_nums,
                                     num_samples, num_dims_per_sample,
                                     diff_n, smooth_n, lag_n);
    std::vector<DSample> DSamples = SB.preprocess();
    return KM->anomalyScore(DSamples.back());
}

extern "C" void
kmeans_delete(kmeans_ref km_ref) {
    KMeans *KM = reinterpret_cast<KMeans *>(km_ref);
    delete KM;
}
