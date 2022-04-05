// SPDX-License-Identifier: GPL-3.0-or-later

#define NETDATA_RRD_INTERNALS
#include "rrd.h"

char *rrdlabel_source_to_string(RRDLABEL_SOURCE l) {
    switch (l) {
        case RRDLABEL_SOURCE_AUTO:
            return "AUTO";
        case RRDLABEL_SOURCE_NETDATA_CONF:
            return "NETDATA.CONF";
        case RRDLABEL_SOURCE_DOCKER :
            return "DOCKER";
        case RRDLABEL_SOURCE_ENVIRONMENT  :
            return "ENVIRONMENT";
        case RRDLABEL_SOURCE_KUBERNETES :
            return "KUBERNETES";
        default:
            return "Invalid label source";
    }
}

int rrdlabel_is_valid_value(const char *value) {
    while(*value) {
        if(*value == '"' || *value == '\'' || *value == '*' || *value == '!') {
            return 0;
        }

        value++;
    }

    return 1;
}

int rrdlabel_is_valid_key(const char *key) {
    //Prometheus exporter
    if(!strcmp(key, "chart") || !strcmp(key, "family")  || !strcmp(key, "dimension"))
        return 0;

    //Netdata and Prometheus  internal
    if (*key == '_')
        return 0;

    while(*key) {
        if(!(isdigit(*key) || isalpha(*key) || *key == '.' || *key == '_' || *key == '-'))
            return 0;

        key++;
    }

    return 1;
}

void strip_last_symbol(
    char *str,
    char symbol,
    SKIP_ESCAPED_CHARACTERS_OPTION skip_escaped_characters)
{
    char *end = str;

    while (*end && *end != symbol) {
        if (unlikely(skip_escaped_characters && *end == '\\')) {
            end++;
            if (unlikely(!*end))
                break;
        }
        end++;
    }
    if (likely(*end == symbol))
        *end = '\0';
}

char *strip_double_quotes(char *str, SKIP_ESCAPED_CHARACTERS_OPTION skip_escaped_characters)
{
    if (*str == '"') {
        str++;
        strip_last_symbol(str, '"', skip_escaped_characters);
    }

    return str;
}

void rrdlabel_index_replace(struct label_index *labels, label_list_t list) {
    netdata_rwlock_wrlock(&labels->labels_rwlock);

    label_list_delete(labels->label_list);
    labels->label_list = list;

    netdata_rwlock_unlock(&labels->labels_rwlock);
}
