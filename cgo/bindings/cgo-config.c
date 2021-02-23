#include "cgo-config.h"
#include "libnetdata/libnetdata.h"

long long cfg_get_number(const char *section, const char *name, long long value) {
    return config_get_number(section, name, value);
}
