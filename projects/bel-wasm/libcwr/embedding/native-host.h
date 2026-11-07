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
void host_workspace_environment(void);
/* Register one launcher-configured source coworker without restoring revoked authority. */
void host_workspace_source(const char *name);
V *host_workspace_ref(const char *kind, const char *name);
V *host_workspace_binding(const char *action);
/* Trusted resource descriptor checks and installation. No source-selected grants. */
int host_resource_check(V *descriptor, V *previous);
/* Validate untrusted worker output against the retained invocation contract. */
int host_resource_output_valid(V *invocation, V *result);
void host_workspace_resource(V *descriptor);
V *host_workspace_resource_target(const char *slug, const char *action);
int host_workspace_resource_matches(V *target);
V *host_resource_provenance(void);
V *host_workspace_step(V *event);
V *host_workspace_finish(V *event, V *executor);
void host_workspace_refresh(void);
V *host_chat_send(V *conversation, const char *name, const char *text);
V *host_artifact(V *reference);
void host_close(void);
int cli_main(int argc, char **argv);
#endif
