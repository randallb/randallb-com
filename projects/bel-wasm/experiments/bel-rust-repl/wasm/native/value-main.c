/* Generic native transport for compiled values, with no evaluation semantics.
 * argv[1] is an interned selector; stdout is a JSON value tree. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "module.h"
#include "wasm-rt-impl.h"
#include "wasm-rt-exceptions.h"

static unsigned steps;
static void failed(const char *message) {
  fprintf(stderr, "native value runner: %s\n", message);
  exit(1);
}
static uint32_t load32(const uint8_t *p) {
  return (uint32_t)p[0] | (uint32_t)p[1] << 8 |
         (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}
static uint32_t part(w2c_bel *module, uint32_t value, unsigned offset) {
  const wasm_rt_memory_t *memory = w2c_bel_memory(module);
  if (value < 4096 || (value & 7) || (uint64_t)value + 8 > memory->size)
    failed("invalid pair");
  return load32(memory->data + value + offset);
}
static void character(uint32_t scalar) {
  if (scalar > 0x10ffff || (scalar >= 0xd800 && scalar <= 0xdfff))
    failed("invalid character");
  if (scalar == '"' || scalar == '\\') { putchar('\\'); putchar((int)scalar); }
  else if (scalar >= 32 && scalar < 127) putchar((int)scalar);
  else if (scalar <= 0xffff) printf("\\u%04x", scalar);
  else {
    scalar -= 0x10000;
    printf("\\u%04x\\u%04x", 0xd800 + (scalar >> 10), 0xdc00 + (scalar & 1023));
  }
}
static void value(w2c_bel *module, uint32_t x, unsigned depth) {
  if (++steps > 32768 || depth > 128) failed("result traversal limit");
  if (!x) { fputs("null", stdout); return; }
  if ((x & 7) == 2) {
    fputs("{\"char\":\"", stdout); character(x >> 3); fputs("\"}", stdout); return;
  }
  if ((x & 7) == 1) {
    uint32_t chars = w2c_bel_symbolname(module, x);
    putchar('"');
    while (chars) {
      if (++steps > 32768) failed("symbol traversal limit");
      uint32_t c = part(module, chars, 0);
      if ((c & 7) != 2) failed("invalid symbol name");
      character(c >> 3); chars = part(module, chars, 4);
    }
    putchar('"'); return;
  }
  fputs("{\"pair\":[", stdout);
  value(module, part(module, x, 0), depth + 1);
  putchar(','); value(module, part(module, x, 4), depth + 1);
  fputs("]}", stdout);
}
int main(int argc, char **argv) {
  if (argc != 2) { fputs("Usage: value-runner SELECTOR\n", stderr); return 2; }
  wasm_rt_init();
  w2c_bel *module = calloc(1, sizeof(*module));
  if (!module) failed("allocation failed");
  const wasm_rt_trap_t trap = wasm_rt_impl_try();
  if (trap != WASM_RT_TRAP_NONE) {
    fprintf(stderr, "native value runner: program trapped: %s\n", wasm_rt_strerror(trap));
    if (w2c_bel_memory(module)->data) wasm2c_bel_free(module);
    free(module); wasm_rt_free(); return 1;
  }
  wasm2c_bel_instantiate(module);
  uint32_t name = 0;
  for (size_t i = strlen(argv[1]); i > 0; --i)
    name = w2c_bel_join(module, ((uint32_t)(unsigned char)argv[1][i - 1] << 3) | 2, name);
  uint32_t result = w2c_bel_choose(module, w2c_bel_intern(module, name));
  value(module, result, 0); putchar('\n');
  int status = ferror(stdout) ? 1 : 0;
  wasm2c_bel_free(module); free(module); wasm_rt_free(); return status;
}
