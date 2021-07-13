// SPDX-License-Identifier: GPL-3.0-or-later

#ifndef ML_PRIVATE_H
#define ML_PRIVATE_H

#include "kmeans/KMeans.h"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <fstream>
#include <iostream>
#include <map>
#include <mutex>
#include <set>
#include <sstream>
#include <string>
#include <thread>
#include <vector>
#include <queue>

extern "C" {

#include "ml.h"

}

namespace ml {

using SteadyClock = std::chrono::steady_clock;
using TimePoint = std::chrono::time_point<SteadyClock>;

template<typename T>
using Duration = std::chrono::duration<T>;

using Seconds = std::chrono::seconds;

}

#endif /* ML_PRIVATE_H */
