#ifndef BEARING_H
#define BEARING_H

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

/**
 * Initialize the bearing coordinator. Spawns a tokio runtime in a background thread.
 */
void bearing_init(void);

/**
 * Shut down the bearing coordinator.
 */
void bearing_shutdown(void);

/**
 * Accept a new child connection fd from the C web server.
 * The coordinator takes ownership of the fd.
 */
int32_t bearing_accept_fd(int32_t fd);

/**
 * Simple ping — no async, just returns pong.
 */
int32_t bearing_ping(uint8_t *out_buf, uintptr_t out_cap, uintptr_t *out_len);

/**
 * Send a query through the coordinator to all connected children.
 * Blocks until the response is ready (or timeout).
 */
int32_t bearing_query(const uint8_t *query_buf,
                      uintptr_t query_len,
                      uint8_t *out_buf,
                      uintptr_t out_cap,
                      uintptr_t *out_len);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* BEARING_H */
