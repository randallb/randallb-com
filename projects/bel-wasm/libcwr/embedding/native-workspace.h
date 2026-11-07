#ifndef LIBCWR_NATIVE_WORKSPACE_H
#define LIBCWR_NATIVE_WORKSPACE_H
int workspace_main(const char *directory, const char *source, const char *compiler, const char *coworker_name, const char *resource);
/* Signal-handler safe: stop only the private worker for this terminal owner. */
void workspace_interrupt(void);
#endif
