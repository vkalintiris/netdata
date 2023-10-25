#ifndef RDB_COMMON_H
#define RDB_COMMON_H

#include "protos/rdbv.pb.h"
#include "rdb.h"
#include "UuidShard.h"

#include <rocksdb/advanced_options.h>
#include <rocksdb/db.h>
#include <rocksdb/statistics.h>
#include <rocksdb/table.h>

#ifdef ENABLE_TESTS
#include <gtest/gtest.h>
#include <random>
#endif

namespace rdb {

namespace pb = google::protobuf;

using Options = rocksdb::Options;
using Slice = rocksdb::Slice;
using Status = rocksdb::Status;

using Value = rdbv::RdbValue;
using StorageNumbersPage = rdbv::StorageNumbersPage;

};

#endif /* RDB_COMMON_H */
