add_subdirectory(libjudy)

set(LIBNETDATA_FILES
        adaptive_resortable_list/adaptive_resortable_list.c
        adaptive_resortable_list/adaptive_resortable_list.h
        config/appconfig.c
        config/appconfig.h
        arrayalloc/arrayalloc.c
        arrayalloc/arrayalloc.h
        avl/avl.c
        avl/avl.h
        buffer/buffer.c
        buffer/buffer.h
        clocks/clocks.c
        clocks/clocks.h
        completion/completion.c
        completion/completion.h
        dictionary/dictionary.c
        dictionary/dictionary.h
        eval/eval.c
        eval/eval.h
        inlined.h
        libnetdata.c
        libnetdata.h
        required_dummies.h
        locks/locks.c
        locks/locks.h
        log/log.c
        log/log.h
        os.c
        os.h
        onewayalloc/onewayalloc.c
        onewayalloc/onewayalloc.h
        popen/popen.c
        popen/popen.h
        procfile/procfile.c
        procfile/procfile.h
        simple_pattern/simple_pattern.c
        simple_pattern/simple_pattern.h
        socket/socket.c
        socket/socket.h
        statistical/statistical.c
        statistical/statistical.h
        storage_number/storage_number.c
        storage_number/storage_number.h
        threads/threads.c
        threads/threads.h
        url/url.c
        url/url.h
        json/json.c
        json/json.h
        json/jsmn.c
        json/jsmn.h
        health/health.c
        health/health.h
        string/utf8.h
        socket/security.c
        socket/security.h
        worker_utilization/worker_utilization.c
        worker_utilization/worker_utilization.h
        circular_buffer/circular_buffer.c
        circular_buffer/circular_buffer.h
    )

if(ENABLE_PLUGIN_EBPF)
    list(APPEND LIBNETDATA_FILES
        libnetdata/ebpf/ebpf.c
        libnetdata/ebpf/ebpf.h)
endif()

add_library(libnetdata OBJECT ${LIBNETDATA_FILES})
target_include_directories(libnetdata PRIVATE ${NETDATA_INCLUDE_DIR})

# Our build relies on adding the top-level src dir to the header search
# path. Instead of adding the dir to every single cc/cxx command, we propagate
# path through upstream deps of libnetdata.
target_include_directories(libnetdata INTERFACE ${CMAKE_SOURCE_DIR})
