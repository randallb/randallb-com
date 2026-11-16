/* Generic acceptance adapter: one named check, one true/false result. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "module.h"
#include "wasm-rt-impl.h"
#include "wasm-rt-exceptions.h"
#include "value-roots.h"
int main(int argc, char **argv) {
  if (argc != 2) return 2;
  wasm_rt_init();
  w2c_bel *module = calloc(1, sizeof(*module));
  if (!module) return 1;
  const wasm_rt_trap_t trap = wasm_rt_impl_try();
  if (trap != WASM_RT_TRAP_NONE) {
    fprintf(stderr, "example trapped: %s\n", wasm_rt_strerror(trap));
    if (w2c_bel_memory(module)->data) { rb_forget(module); wasm2c_bel_free(module); }
    free(module); wasm_rt_free(); return 1;
  }
  wasm2c_bel_instantiate(module);
  rb_initialize(module);
  uint32_t name = 0;
  for (size_t i = strlen(argv[1]); i > 0; --i)
    name = rb_join(module, ((uint32_t)(unsigned char)argv[1][i - 1] << 3) | 2, name);
  uint32_t result = rb_choose(module, rb_intern(module, name));
  int status = result == 1 ? 0 : 1;
  if (status) fputs("example result differs from expected data\n", stderr);
  rb_forget(module); wasm2c_bel_free(module); free(module); wasm_rt_free(); return status;
}
