#include "daemon/common.h"
#include "nd_sentry.h"
#include "sentry.h"

static void set_sentry_database_path(sentry_options_t *options, const char *cache_dir) {
    char path[FILENAME_MAX + 1];
    snprintfz(path, FILENAME_MAX, "%s/sentry-native", cache_dir);
    sentry_options_set_database_path(options, path);
}

static void send_event(void)
{
    sentry_set_transaction("send_event");

    sentry_add_breadcrumb(sentry_value_new_breadcrumb(0, "Configuring GPU Context"));

    sentry_value_t gpu = sentry_value_new_object();
    sentry_value_set_by_key(gpu, "name", sentry_value_new_string("AMD Radeon Pro 560"));
    sentry_value_set_by_key(gpu, "vendor_name", sentry_value_new_string("Apple"));
    sentry_value_set_by_key(gpu, "memory_size", sentry_value_new_int32(4096));
    sentry_value_set_by_key(gpu, "api_type", sentry_value_new_string("Metal"));
    sentry_value_set_by_key(gpu, "multi_threaded_rendering", sentry_value_new_bool(1));
    sentry_value_set_by_key(gpu, "version", sentry_value_new_string("Metal"));

    sentry_value_t os = sentry_value_new_object();
    sentry_value_set_by_key(os, "name", sentry_value_new_string("macOS"));
    sentry_value_set_by_key(os, "version", sentry_value_new_string("10.14.6 (18G95)"));

    sentry_value_t contexts = sentry_value_new_object();
    sentry_value_set_by_key(contexts, "gpu", gpu);
    sentry_value_set_by_key(contexts, "os", os);

    sentry_value_t event = sentry_value_new_event();
    sentry_value_set_by_key(event, "message", sentry_value_new_string("Sentry Message Capture"));
    sentry_value_set_by_key(event, "contexts", contexts);

    sentry_set_tag("key_name", "value");

    sentry_capture_event(event);
}

void nd_sentry_init() {
    sentry_options_t *options = sentry_options_new();

    set_sentry_database_path(options, netdata_configured_cache_dir);
    sentry_options_set_symbolize_stacktraces(options, true);
    sentry_options_set_dsn(options, "https://0a236f1a50b942e39c44408e682b8510@o4504255002116096.ingest.sentry.io/4504255009062912");
    sentry_options_set_release(options, "native@1.2.10");
    
    sentry_options_set_debug(options, 1);

    sentry_init(options);

    send_event();
}

void nd_sentry_close() {
    sentry_close();
}
