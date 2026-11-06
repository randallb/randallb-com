#ifndef LIBCWR_NATIVE_VALUES_H
#define LIBCWR_NATIVE_VALUES_H
#include <stddef.h>
#include <stdint.h>
#include <setjmp.h>

typedef enum { VT_STRING, VT_ARRAY, VT_OBJECT, VT_NUMBER, VT_BOOL } VType;
typedef struct V {
  VType type;
  size_t n, capacity;
  char *s;
  struct V **items;
  char **keys;
} V;
extern jmp_buf *v_handler;
extern char v_error[512];
_Noreturn void v_fail(const char *format, ...);
void *v_alloc(size_t size);
void v_reset(void);
V *v_string(const char *text);
V *v_string_n(const char *text, size_t length);
V *v_number(long number);
V *v_array(void);
V *v_object(void);
void v_push(V *array, V *value);
void v_put(V *object, const char *key, V *value);
V *v_get(V *object, const char *key);
V *v_record_get(V *record, const char *key);
V *v_record_set(V *record, const char *key, V *value);
int v_isstr(V *value, const char *text);
V *v_parse(const char *text, size_t length);
char *v_json(V *value);
uint32_t v_scalar(const char *text, size_t length, size_t *position);
V *v_character(uint32_t scalar);
V *v_chars(const char *text);
V *v_chars_n(const char *text, size_t length);
char *v_text(V *characters);
char *v_text_n(V *characters, size_t *length);
V *core_transition(V *state, V *event, V *environment);
#endif
