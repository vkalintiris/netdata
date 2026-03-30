// SPDX-License-Identifier: GPL-3.0-or-later

#include "api_v2_calls.h"

#ifdef HAVE_BEARING

#include "bearing.h"
#include "web/server/web_server.h"
#include "web/server/static/static-threaded.h"

int api_v2_bearing(RRDHOST *host __maybe_unused, struct web_client *w, char *url) {
    const char *action = "ping";
    const char *q = NULL;

    while(url) {
        char *value = strsep_skip_consecutive_separators(&url, "&");
        if(!value || !*value) continue;

        char *name = strsep_skip_consecutive_separators(&value, "=");
        if(!name || !*name) continue;
        if(!value || !*value) continue;

        if(!strcmp(name, "action"))
            action = value;
        else if(!strcmp(name, "q"))
            q = value;
    }

    BUFFER *wb = w->response.data;
    buffer_reset(wb);
    wb->content_type = CT_APPLICATION_JSON;

    if(!strcmp(action, "ping")) {
        uint8_t buf[256];
        uintptr_t len = 0;

        int32_t rc = bearing_ping(buf, sizeof(buf), &len);
        if(rc != 0)
            return HTTP_RESP_INTERNAL_SERVER_ERROR;

        buffer_strncat(wb, (const char *)buf, (size_t)len);
        return HTTP_RESP_OK;
    }

    if(!strcmp(action, "query")) {
        if(!q || !*q) {
            buffer_strcat(wb, "{\"error\":\"missing 'q' parameter\"}");
            return HTTP_RESP_BAD_REQUEST;
        }

        uint8_t buf[4096];
        uintptr_t len = 0;

        int32_t rc = bearing_query(
            (const uint8_t *)q, strlen(q),
            buf, sizeof(buf), &len);

        if(rc != 0) {
            buffer_sprintf(wb, "{\"error\":\"bearing_query failed with code %d\"}", (int)rc);
            return HTTP_RESP_INTERNAL_SERVER_ERROR;
        }

        buffer_strncat(wb, (const char *)buf, (size_t)len);
        return HTTP_RESP_OK;
    }

    buffer_sprintf(wb, "{\"error\":\"unknown action '%s'\"}",  action);
    return HTTP_RESP_BAD_REQUEST;
}

int bearing_accept_connection(struct web_client *w) {
    int fd = w->fd;

    // Take over the socket from the web server — same pattern as
    // stream_receiver_takeover_web_connection() in stream-receiver-connection.c
    WEB_CLIENT_IS_DEAD(w);

    if(web_server_mode == WEB_SERVER_MODE_STATIC_THREADED)
        web_client_flag_set(w, WEB_CLIENT_FLAG_DONT_CLOSE_SOCKET);
    else
        w->fd = -1;

    buffer_flush(w->response.data);
    web_server_remove_current_socket_from_poll();

    // Hand the raw fd to the Rust coordinator.
    int32_t rc = bearing_accept_fd(fd);
    if(rc != 0) {
        close(fd);
        return HTTP_RESP_INTERNAL_SERVER_ERROR;
    }

    return HTTP_RESP_OK;
}

#else

int api_v2_bearing(RRDHOST *host __maybe_unused, struct web_client *w, char *url __maybe_unused) {
    buffer_reset(w->response.data);
    buffer_strcat(w->response.data, "bearing not enabled");
    return HTTP_RESP_SERVICE_UNAVAILABLE;
}

int bearing_accept_connection(struct web_client *w __maybe_unused) {
    return HTTP_RESP_SERVICE_UNAVAILABLE;
}

#endif
