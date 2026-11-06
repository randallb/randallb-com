#ifndef LIBCWR_NATIVE_HOST_H
#define LIBCWR_NATIVE_HOST_H
#include "native-values.h"

/* A single local owning session; values live until host_close/v_reset. */
void host_prepare_signals(void);
void host_open(const char *directory, const char *context);
V *host_init(const char *directory);
V *host_event(V *event);
V *host_state(void);
V *host_environment(void);
void host_chat_binding(void);
V *host_chat_send(V *conversation, const char *name, const char *text);
V *host_artifact(V *reference);
void host_close(void);
int cli_main(int argc, char **argv);
#endif
