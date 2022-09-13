find_library(HAVE_AWS_CHECKSUMS aws-checksums)
find_library(HAVE_AWS_COMMON aws-c-common)
find_library(HAVE_AWS_EVENT_STREAM aws-c-event-stream)

pkg_check_modules(AWS_CORE aws-cpp-sdk-core)
pkg_check_modules(KINESIS aws-cpp-sdk-kinesis)
