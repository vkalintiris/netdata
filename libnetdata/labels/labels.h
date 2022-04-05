#ifndef LABELS_H
#define LABELS_H

#include "../libnetdata.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef int label_source_t;
typedef void *label_t;

const char *label_key(label_t label);
const char *label_value(label_t label);
label_source_t label_source(label_t label);

typedef void *label_list_t;

label_list_t label_list_new();
void label_list_delete(label_list_t list);
void label_list_clear(label_list_t list);

void label_list_print(label_list_t list);
size_t label_list_size(label_list_t list);

void label_list_add(label_list_t list, const char *key, const char *value, label_source_t source);
label_t label_list_lookup_key(label_list_t list, const char *key);
label_t label_list_lookup_keylist(label_list_t list, const char *keylist);

// the callback can return true to stop the iteration
typedef bool (*label_callback_t)(const label_t label, void *data);
bool label_list_foreach(label_list_t list, label_callback_t cb, void *cb_data);
void label_list_update(label_list_t dst, const label_list_t src);

void label_list_to_json_buffer(label_list_t list, BUFFER *wb,
                               const char *kv_format,
                               const char *kv_separator,
                               size_t indentation);

#ifdef __cplusplus
};
#endif

#endif /* LABELS_H */
