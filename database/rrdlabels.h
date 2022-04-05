#ifndef NETDATA_RRDLABELS_H
#define NETDATA_RRDLABELS_H

#include "rrd.h"

/*
 * label source
 */

typedef enum label_source {
    RRDLABEL_SOURCE_AUTO             = 0,
    RRDLABEL_SOURCE_NETDATA_CONF     = 1,
    RRDLABEL_SOURCE_DOCKER           = 2,
    RRDLABEL_SOURCE_ENVIRONMENT      = 3,
    RRDLABEL_SOURCE_KUBERNETES       = 4
} RRDLABEL_SOURCE;

char *rrdlabel_source_to_string(RRDLABEL_SOURCE l);

/*
 * A label is a key/value pair that records its place of origin
*/
extern int rrdlabel_is_valid_value(const char *value);
extern int rrdlabel_is_valid_key(const char *key);

/*
 * Labels + flags to detect whether we should stream/export them or not
 */

struct label_index {
    label_list_t label_list;
    netdata_rwlock_t labels_rwlock;         // lock for the label list
    uint32_t labels_flag;                   // Flags for labels
};

#define RRDLABEL_FLAG_UPDATE_STREAM 1
#define RRDLABEL_FLAG_STOP_STREAM 2

extern void rrdlabel_index_replace(struct label_index *labels, label_list_t list);

/*
 * Util functions to fixup label keys/values.
 */

typedef enum skip_escaped_characters {
    DO_NOT_SKIP_ESCAPED_CHARACTERS,
    SKIP_ESCAPED_CHARACTERS
} SKIP_ESCAPED_CHARACTERS_OPTION;

extern void strip_last_symbol(char *str, char symbol, SKIP_ESCAPED_CHARACTERS_OPTION skip_escaped_characters);
extern char *strip_double_quotes(char *str, SKIP_ESCAPED_CHARACTERS_OPTION skip_escaped_characters);

typedef enum strip_quotes {
    DO_NOT_STRIP_QUOTES,
    STRIP_QUOTES
} STRIP_QUOTES_OPTION;

#endif /* NETDATA_RRDLABELS_H */
