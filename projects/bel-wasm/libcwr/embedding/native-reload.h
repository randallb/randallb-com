#ifndef LIBCWR_NATIVE_RELOAD_H
#define LIBCWR_NATIVE_RELOAD_H
#include <stddef.h>
#include "native-values.h"

/* Configuration and the last successful executable survive host value resets.
 * Configure/check/reload print diagnostics and return zero on success.
 * Prepare validates launcher paths without compiling or activating source. */
int coworker_configure(const char *source, const char *compiler);
int coworker_prepare(const char *source, const char *compiler);
int coworker_reload(void);
int coworker_check(void);
/* Bounded UTF-8 compiler output and outcome for the most recent operation. */
const char *coworker_diagnostics(void);
int coworker_active(void);
/* Structured ordinary-program mode; validator belongs to trusted host/core. */
void coworker_resource_validator(int (*validator)(V *descriptor,V *previous));
int coworker_resource_prepare(const char *source,const char *compiler);
int coworker_resource_configure(const char *source,const char *compiler);
int coworker_resource_mode(void);
V *coworker_resource_descriptor(void);
V *coworker_resource_invoke(V *request);
const char *coworker_source_path(void);
const char *coworker_build_path(void);
unsigned long coworker_build_version(void);
char *coworker_transform(const char *input, size_t size, size_t *output_size);
void coworker_close(void);
/* Signal-handler safe: terminate only the currently running private child group. */
void coworker_interrupt(void);
#endif
