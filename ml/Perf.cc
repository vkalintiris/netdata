#include "Unit.h"
#include "Perf.h"

#include "ml-private.h"
#include "ml.h"

#include <random>

using namespace ml;

extern void ml_init(void);

void ml_perf() {
    default_health_enabled = 0;
    sql_init_database();
    generate_dbengine_dataset(3600 * 24 * 30);

    std::string HostName{"dbengine-dataset"};
    RRDHOST *RH = rrdhost_find_by_hostname(HostName.c_str(), 0);
    if (!RH) {
        std::cout << "Could not find host: " << HostName << std::endl;
        return;
    }

    RRDSET *RS = RH->rrdset_root;
    RRDDIM *RD = &RS->dimensions[0];

    Cfg.TrainSecs = Millis{1000} * 3600 * 6;
    Cfg.MinTrainSecs = Millis{1000} * 3600 * 5;

    Unit *U = new Unit(RD);
    KMeans &KM = U->getKMeansRef();

    std::cout << "MinDist: " << KM.getMinDist() << std::endl;
    std::cout << "MaxDist: " << KM.getMaxDist() << std::endl;

    U->train();

    std::cout << "MinDist: " << KM.getMinDist() << std::endl;
    std::cout << "MaxDist: " << KM.getMaxDist() << std::endl;
}
