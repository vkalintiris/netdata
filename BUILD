#
# proto
#

proto_library(
    name = "aclk_protos",
    srcs = [
        "aclk/aclk-schemas/proto/aclk/v1/lib.proto",
        "aclk/aclk-schemas/proto/alarm/v1/stream.proto",
        "aclk/aclk-schemas/proto/alarm/v1/config.proto",
        "aclk/aclk-schemas/proto/chart/v1/stream.proto",
        "aclk/aclk-schemas/proto/chart/v1/config.proto",
        "aclk/aclk-schemas/proto/chart/v1/instance.proto",
        "aclk/aclk-schemas/proto/chart/v1/dimension.proto",
        "aclk/aclk-schemas/proto/nodeinstance/info/v1/info.proto",
        "aclk/aclk-schemas/proto/nodeinstance/connection/v1/connection.proto",
        "aclk/aclk-schemas/proto/nodeinstance/create/v1/creation.proto",
        "aclk/aclk-schemas/proto/agent/v1/disconnect.proto",
        "aclk/aclk-schemas/proto/agent/v1/connection.proto",
        "aclk/aclk-schemas/proto/context/v1/context.proto",
        "aclk/aclk-schemas/proto/context/v1/stream.proto",
    ],
    deps = [
        "@com_google_protobuf//:duration_proto",
        "@com_google_protobuf//:timestamp_proto",
    ],
    strip_import_prefix = "aclk/aclk-schemas",
)

cc_proto_library(
    name = "aclk_cc_protos",
    deps = [":aclk_protos"],
)

