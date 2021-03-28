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

namespace ml {

using SteadyClock = std::chrono::steady_clock;
using TimePoint = std::chrono::time_point<SteadyClock>;
using Seconds = std::chrono::seconds;
using Millis = std::chrono::milliseconds;

void trainMain(struct netdata_static_thread *Thread);
void predictMain(struct netdata_static_thread *Thread);

};

#include "Config.h"
#include "Unit.h"
#include "Chart.h"
#include "Host.h"
#include "Window.h"

#endif /* ML_PRIVATE_H */
