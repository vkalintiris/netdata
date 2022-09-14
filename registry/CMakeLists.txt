# SPDX-License-Identifier: GPL-3.0-or-later

set(REGISTRY_PLUGIN_FILES registry.c
                          registry.h
                          registry_db.c
                          registry_init.c
                          registry_internals.c
                          registry_internals.h
                          registry_log.c
                          registry_machine.c
                          registry_machine.h
                          registry_person.c
                          registry_person.h
                          registry_url.c
                          registry_url.h)

target_sources(netdata PRIVATE ${REGISTRY_PLUGIN_FILES})
