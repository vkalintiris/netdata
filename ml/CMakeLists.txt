# SPDX-License-Identifier: GPL-3.0-or-later

set(ML_FILES ml.h
             ml-dummy.c)

if(ENABLE_ML)
    list(APPEND ML_FILES BitBufferCounter.h
                         BitBufferCounter.cc
                         BitRateWindow.h
                         BitRateWindow.cc
                         Config.h
                         Config.cc
                         Database.h
                         Database.cc
                         Dimension.cc
                         Dimension.h
                         Host.h
                         Host.cc
                         Query.h
                         kmeans/KMeans.h
                         kmeans/KMeans.cc
                         kmeans/SamplesBuffer.h
                         kmeans/SamplesBuffer.cc
                         kmeans/dlib/dlib/all/source.cpp
                         json/single_include/nlohmann/json.hpp
                         ml.cc
                         ml-private.h)

    target_include_directories(netdata PRIVATE "kmeans/dlib")
    target_compile_options(netdata PRIVATE "-DDLIB_NO_GUI_SUPPORT")
endif()

target_sources(netdata PRIVATE ${ML_FILES})
