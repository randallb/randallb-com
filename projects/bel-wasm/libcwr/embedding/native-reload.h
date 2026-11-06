#ifndef LIBCWR_NATIVE_RELOAD_H
#define LIBCWR_NATIVE_RELOAD_H
#include <stddef.h>

/* Configuration and the last successful executable survive host value resets.
 * Configure/reload print diagnostics and return zero on success. */
int coworker_configure(const char *source, const char *compiler);
int coworker_reload(void);
int coworker_active(void);
char *coworker_transform(const char *input, size_t size, size_t *output_size);
void coworker_close(void);
/* Signal-handler safe: terminate only the currently running private child group. */
void coworker_interrupt(void);
#endif
