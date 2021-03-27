// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_PRIVATE_H
#define ML_PRIVATE_H

#include "kmeans/KMeans.h"
#include "spdr/spdr.hh"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <fstream>
#include <iostream>
#include <map>
#include <set>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

extern "C" {

#include "daemon/common.h"

};

#include "Config.h"
#include "Unit.h"
#include "Chart.h"
#include "Host.h"
#include "Window.h"

namespace ml {

void trainMain(struct netdata_static_thread *Thread);
void predictMain(struct netdata_static_thread *Thread);

};

#endif /* ML_PRIVATE_H */
