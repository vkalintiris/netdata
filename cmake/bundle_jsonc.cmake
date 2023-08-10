include(ExternalProject)

ExternalProject_Add(jsonc
  URL "https://github.com/json-c/json-c/archive/json-c-0.14-20200419.tar.gz"
  CMAKE_ARGS -DBUILD_SHARED_LIBS=Off
)
