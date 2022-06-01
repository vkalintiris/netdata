#ifndef REPLICATION_PRIVATE_H
#define REPLICATION_PRIVATE_H

#include "replication.h"

#include "gaps.pb.h"

#if GOOGLE_PROTOBUF_VERSION < 3001000
#define PROTO_COMPAT_MSG_SIZE(msg) (size_t)msg.ByteSize()
#define PROTO_COMPAT_MSG_SIZE_PTR(msg) (size_t)msg->ByteSize()
#else
#define PROTO_COMPAT_MSG_SIZE(msg) msg.ByteSizeLong()
#define PROTO_COMPAT_MSG_SIZE_PTR(msg) msg->ByteSizeLong()
#endif

#include <algorithm>
#include <chrono>
#include <mutex>
#include <thread>
#include <vector>

#include "Config.h"
#include "Utils.h"

#endif /* REPLICATION_PRIVATE_H */
