#ifndef REPLICATION_PRIVATE_H
#define REPLICATION_PRIVATE_H

#include "replication.h"
#include "collectops.h"

#include "gaps.pb.h"

#if GOOGLE_PROTOBUF_VERSION < 3001000
#define PROTO_COMPAT_MSG_SIZE(msg) (size_t)msg.ByteSize()
#define PROTO_COMPAT_MSG_SIZE_PTR(msg) (size_t)msg->ByteSize()
#else
#define PROTO_COMPAT_MSG_SIZE(msg) msg.ByteSizeLong()
#define PROTO_COMPAT_MSG_SIZE_PTR(msg) msg->ByteSizeLong()
#endif

#include <chrono>
#include <mutex>
#include <sstream>
#include <stack>
#include <thread>
#include <utility>
#include <vector>
#include <queue>

#include "Config.h"
#include "Utils.h"
#include "TimeRange.h"
#include "Base64.h"
#include "GapData.h"

#endif /* REPLICATION_PRIVATE_H */
