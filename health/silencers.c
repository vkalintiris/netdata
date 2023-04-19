#include "health.h"

SILENCERS *silencers;

/**
 * Create Silencer
 *
 * Allocate a new silencer to Netdata.
 *
 * @return It returns the address off the silencer on success and NULL otherwise
 */
SILENCER *create_silencer(void) {
    SILENCER *t = callocz(1, sizeof(SILENCER));
    debug(D_HEALTH, "HEALTH command API: Created empty silencer");

    return t;
}

/**
 * Health Silencers add
 *
 * Add more one silencer to the list of silencers.
 *
 * @param silencer
 */
void health_silencers_add(SILENCER *silencer) {
    // Add the created instance to the linked list in silencers
    silencer->next = silencers->silencers;
    silencers->silencers = silencer;
    debug(D_HEALTH, "HEALTH command API: Added silencer %s:%s:%s:%s:%s", silencer->alarms,
          silencer->charts, silencer->contexts, silencer->hosts, silencer->families
    );
}

/**
 * Silencers Add Parameter
 *
 * Create a new silencer and adjust the variables
 *
 * @param silencer a pointer to the silencer that will be adjusted
 * @param key the key value sent by client
 * @param value the value sent to the key
 *
 * @return It returns the silencer configured on success and NULL otherwise
 */
SILENCER *health_silencers_addparam(SILENCER *silencer, char *key, char *value) {
    static uint32_t
            hash_alarm = 0,
            hash_template = 0,
            hash_chart = 0,
            hash_context = 0,
            hash_host = 0,
            hash_families = 0;

    if (unlikely(!hash_alarm)) {
        hash_alarm = simple_uhash(HEALTH_ALARM_KEY);
        hash_template = simple_uhash(HEALTH_TEMPLATE_KEY);
        hash_chart = simple_uhash(HEALTH_CHART_KEY);
        hash_context = simple_uhash(HEALTH_CONTEXT_KEY);
        hash_host = simple_uhash(HEALTH_HOST_KEY);
        hash_families = simple_uhash(HEALTH_FAMILIES_KEY);
    }

    uint32_t hash = simple_uhash(key);
    if (unlikely(silencer == NULL)) {
        if (
                (hash == hash_alarm && !strcasecmp(key, HEALTH_ALARM_KEY)) ||
                (hash == hash_template && !strcasecmp(key, HEALTH_TEMPLATE_KEY)) ||
                (hash == hash_chart && !strcasecmp(key, HEALTH_CHART_KEY)) ||
                (hash == hash_context && !strcasecmp(key, HEALTH_CONTEXT_KEY)) ||
                (hash == hash_host && !strcasecmp(key, HEALTH_HOST_KEY)) ||
                (hash == hash_families && !strcasecmp(key, HEALTH_FAMILIES_KEY))
                ) {
            silencer = create_silencer();
            if(!silencer) {
                error("Cannot add a new silencer to Netdata");
                return NULL;
            }
        }
    }

    if (hash == hash_alarm && !strcasecmp(key, HEALTH_ALARM_KEY)) {
        silencer->alarms = strdupz(value);
        silencer->alarms_pattern = simple_pattern_create(silencer->alarms, NULL, SIMPLE_PATTERN_EXACT, true);
    } else if (hash == hash_chart && !strcasecmp(key, HEALTH_CHART_KEY)) {
        silencer->charts = strdupz(value);
        silencer->charts_pattern = simple_pattern_create(silencer->charts, NULL, SIMPLE_PATTERN_EXACT, true);
    } else if (hash == hash_context && !strcasecmp(key, HEALTH_CONTEXT_KEY)) {
        silencer->contexts = strdupz(value);
        silencer->contexts_pattern = simple_pattern_create(silencer->contexts, NULL, SIMPLE_PATTERN_EXACT, true);
    } else if (hash == hash_host && !strcasecmp(key, HEALTH_HOST_KEY)) {
        silencer->hosts = strdupz(value);
        silencer->hosts_pattern = simple_pattern_create(silencer->hosts, NULL, SIMPLE_PATTERN_EXACT, true);
    } else if (hash == hash_families && !strcasecmp(key, HEALTH_FAMILIES_KEY)) {
        silencer->families = strdupz(value);
        silencer->families_pattern = simple_pattern_create(silencer->families, NULL, SIMPLE_PATTERN_EXACT, true);
    }

    return silencer;
}

/**
 * JSON Read Callback
 *
 * Callback called by netdata to create the silencer.
 *
 * @param e the main json structure
 *
 * @return It always return 0.
 */
int health_silencers_json_read_callback(JSON_ENTRY *e)
{
    switch(e->type) {
        case JSON_OBJECT:
#ifndef ENABLE_JSONC
            e->callback_function = health_silencers_json_read_callback;
            if(strcmp(e->name,"")) {
                // init silencer
                debug(D_HEALTH, "JSON: Got object with a name, initializing new silencer for %s",e->name);
#endif
            e->callback_data = create_silencer();
            if(e->callback_data) {
                health_silencers_add(e->callback_data);
            }
#ifndef ENABLE_JSONC
            }
#endif
            break;

        case JSON_ARRAY:
            e->callback_function = health_silencers_json_read_callback;
            break;

        case JSON_STRING:
            if(!strcmp(e->name,"type")) {
                debug(D_HEALTH, "JSON: Processing type=%s",e->data.string);
                if (!strcmp(e->data.string,"SILENCE")) silencers->stype = STYPE_SILENCE_NOTIFICATIONS;
                else if (!strcmp(e->data.string,"DISABLE")) silencers->stype = STYPE_DISABLE_ALARMS;
            } else {
                debug(D_HEALTH, "JSON: Adding %s=%s", e->name, e->data.string);
                if (e->callback_data)
                    (void)health_silencers_addparam(e->callback_data, e->name, e->data.string);
            }
            break;

        case JSON_BOOLEAN:
            debug(D_HEALTH, "JSON: Processing all_alarms");
            silencers->all_alarms=e->data.boolean?1:0;
            break;

        case JSON_NUMBER:
        case JSON_NULL:
            break;
    }

    return 0;
}

/**
 * Silencers init
 *
 * Function used to initialize the silencer structure.
 */
static void health_silencers_init(void) {
    FILE *fd = fopen(silencers_filename, "r");
    if (fd) {
        fseek(fd, 0 , SEEK_END);
        off_t length = (off_t) ftell(fd);
        fseek(fd, 0 , SEEK_SET);

        if (length > 0 && length < HEALTH_SILENCERS_MAX_FILE_LEN) {
            char *str = mallocz((length+1)* sizeof(char));
            if(str) {
                size_t copied;
                copied = fread(str, sizeof(char), length, fd);
                if (copied == (length* sizeof(char))) {
                    str[length] = 0x00;
                    json_parse(str, NULL, health_silencers_json_read_callback);
                    info("Parsed health silencers file %s", silencers_filename);
                } else {
                    error("Cannot read the data from health silencers file %s", silencers_filename);
                }
                freez(str);
            }
        } else {
            error(
                "Health silencers file %s has the size %" PRId64 " that is out of range[ 1 , %d ]. Aborting read.",
                silencers_filename,
                (int64_t)length,
                HEALTH_SILENCERS_MAX_FILE_LEN);
        }
        fclose(fd);
    } else {
        info("Cannot open the file %s, so Netdata will work with the default health configuration.",silencers_filename);
    }
}

/**
 * Initialize Global Silencers
 *
 * Initialize the silencer  for the whole netdata system.
 *
 * @return It returns 0 on success and -1 otherwise
 */
int health_initialize_global_silencers() {
    char filename[FILENAME_MAX + 1];
    snprintfz(filename, FILENAME_MAX, "%s/health.silencers.json", netdata_configured_varlib_dir);
    silencers_filename = config_get(CONFIG_SECTION_HEALTH, "silencers file", filename);

    silencers =  mallocz(sizeof(SILENCERS));
    silencers->all_alarms=0;
    silencers->stype=STYPE_NONE;
    silencers->silencers=NULL;

    health_silencers_init();

    return 0;
}