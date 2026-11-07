/* ABI2 benchmark transport. Language workloads execute in the generated module.
 * Timed regions contain only choose_args; validation and setup are excluded. */
#define _POSIX_C_SOURCE 200809L
#include <errno.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include "module.h"
#include "wasm-rt-impl.h"
#include "wasm-rt-exceptions.h"
#include "value-roots.h"

#define COUNTERS(X) \
 X(live_cells) X(collections) X(reclaimed_cells) X(root_top) X(max_roots) \
 X(host_roots) X(live_symbols) X(max_calls) X(tail_transfers) X(total_bytes)
#define FIELD(name) uint32_t name;
typedef struct { COUNTERS(FIELD) } Counters;
#undef FIELD

static void failed(const char *message) {
  fprintf(stderr, "rbel benchmark: %s\n", message);
  exit(1);
}
static unsigned argument(const char *text, unsigned minimum, unsigned maximum) {
  if (!*text || *text == '-' || *text == '+') failed("invalid benchmark count");
  errno = 0;
  char *end;
  unsigned long value = strtoul(text, &end, 10);
  if (errno || *end || value < minimum || value > maximum) failed("benchmark count outside bounds");
  return (unsigned)value;
}
static uint64_t nanoseconds(void) {
  struct timespec value;
  if (clock_gettime(CLOCK_MONOTONIC, &value)) failed("monotonic clock unavailable");
  return (uint64_t)value.tv_sec * UINT64_C(1000000000) + (uint64_t)value.tv_nsec;
}
static Counters counters(w2c_bel *module) {
  Counters value;
#define READ(name) value.name = w2c_bel_stat_##name(module);
  COUNTERS(READ)
#undef READ
  return value;
}
static void print_counters(Counters value) {
  int separator = 0;
  putchar('{');
#define PRINT(name) printf("%s\"" #name "\":%" PRIu32, separator++ ? "," : "", value.name);
  COUNTERS(PRINT)
#undef PRINT
  putchar('}');
}
static uint32_t field(w2c_bel *module, uint32_t pair, unsigned offset) {
  if (!w2c_bel_pair_valid(module, pair)) failed("invalid result list");
  return rb_word(w2c_bel_memory(module)->data + pair + offset);
}
static uint32_t ascii_symbol(w2c_bel *module, const char *name) {
  uint32_t chars = 0;
  for (size_t i = strlen(name); i; --i)
    chars = rb_join(module, ((uint32_t)(unsigned char)name[i-1] << 3) | 2, chars);
  return rb_intern(module, chars);
}
static int scalar_list(w2c_bel *module, uint32_t value, const uint32_t *expected, size_t count) {
  for (size_t i = 0; i < count; ++i) {
    if (!w2c_bel_pair_valid(module, value) || field(module,value,0) != (expected[i]<<3 | 2)) return 0;
    value = field(module,value,4);
  }
  return value == 0;
}
static int ascii_list(w2c_bel *module, uint32_t value, const char *expected) {
  for (size_t i = 0; expected[i]; ++i) {
    if (!w2c_bel_pair_valid(module, value) || field(module,value,0) != ((uint32_t)(unsigned char)expected[i]<<3 | 2)) return 0;
    value = field(module,value,4);
  }
  return value == 0;
}
static void validate(w2c_bel *module, const char *selector, uint32_t value, uint32_t left, uint32_t right) {
  if (!strcmp(selector,"list-reverse")) {
    for (unsigned i = 0; i < 1024; ++i) {
      if (!w2c_bel_pair_valid(module,value) || field(module,value,0) != (i%2 ? left : right))
        failed("incorrect reversed-list result");
      value = field(module,value,4);
    }
    if (value) failed("incorrect reversed-list length");
  } else if (!strcmp(selector,"exact-arithmetic")) {
    if (!ascii_list(module,value,"1000000000000000000000000000000")) failed("incorrect exact-arithmetic result");
  } else if (!strcmp(selector,"utf8-roundtrip")) {
    const uint32_t expected[] = {'h', 0xe9, 0x1f609};
    if (!scalar_list(module,value,expected,3)) failed("incorrect UTF-8 roundtrip result");
  } else {
    if (w2c_bel_value_kind(module,value) != 2 || !ascii_list(module,w2c_bel_symbol_name_view(module,value),"done"))
      failed("incorrect tail-loop result");
  }
}
static void sample(const char *selector, unsigned iterations, int publish) {
  w2c_bel *module = calloc(1,sizeof(*module));
  if (!module) failed("instance allocation failed");
  wasm2c_bel_instantiate(module);
  if (w2c_bel_rbel_abi_version(module) != 2 || w2c_bel_runtime_init(module,32768,65536,4096,1024,128))
    failed("benchmark runtime initialization failed");
  rb_register(module);
  uint32_t mode = ascii_symbol(module,selector);
  RbRoot mode_root = rb_hold(module,mode);
  uint32_t left = 0, right = 0, input = 0;
  RbRoot left_root = {0}, right_root = {0};
  if (!strcmp(selector,"list-reverse")) {
    left = ascii_symbol(module,"left"); left_root = rb_hold(module,left);
    right = ascii_symbol(module,"right"); right_root = rb_hold(module,right);
    for (unsigned i = 1024; i; --i) input = rb_join(module,(i-1)%2 ? right : left,input);
  } else if (!strcmp(selector,"tail-direct") || !strcmp(selector,"tail-apply") || !strcmp(selector,"allocation-churn")) {
    for (unsigned i = 0; i < 10000; ++i) input = rb_join(module,0,input);
  }
  RbRoot input_root = rb_hold(module,input);
  const uint32_t fields[] = {mode,input};
  uint32_t arguments = rb_list(module,fields,2);
  RbRoot arguments_root = rb_hold(module,arguments);
  rb_drop(module,input_root); rb_drop(module,mode_root);
  if (left_root.owner) { rb_drop(module,left_root); rb_drop(module,right_root); }
  /* Clear transport temporaries before measuring execution on the rooted graph. */
  if (w2c_bel_collect(module)) failed("preparation collection failed");
  rb_require(module,"preparation collection");
  Counters before = counters(module);
  if (before.root_top || before.host_roots != 1) failed("unexpected roots before timing");
  uint64_t elapsed = 0;
  for (unsigned i = 0; i < iterations; ++i) {
    uint64_t start = nanoseconds();
    uint32_t result = w2c_bel_choose_args(module,arguments);
    elapsed += nanoseconds() - start;
    rb_require(module,"benchmark execution");
    /* Validation cannot allocate, so the runtime result handoff remains rooted. */
    validate(module,selector,result,left,right);
    if (w2c_bel_stat_root_top(module) || w2c_bel_stat_host_roots(module) != 1)
      failed("benchmark execution leaked roots");
  }
  Counters after = counters(module);
  if (publish) {
    printf("{\"elapsed_ns\":%" PRIu64 ",\"iterations\":%u,\"counters_before\":",elapsed,iterations);
    print_counters(before); fputs(",\"counters_after\":",stdout); print_counters(after); putchar('}');
  }
  rb_drop(module,arguments_root);
  if (w2c_bel_stat_host_roots(module)) failed("benchmark teardown leaked roots");
  rb_forget(module); wasm2c_bel_free(module); free(module);
}
int main(int argc, char **argv) {
  if (argc != 5) {
    fputs("Usage: native-benchmark SELECTOR WARMUP SAMPLES ITERATIONS\n",stderr); return 2;
  }
  const char *selector = argv[1];
  const char *valid[] = {"tail-direct","tail-apply","allocation-churn","list-reverse","exact-arithmetic","utf8-roundtrip"};
  int found = 0;
  for (size_t i = 0; i < sizeof(valid)/sizeof(*valid); ++i) if (!strcmp(selector,valid[i])) found = 1;
  if (!found) failed("unknown benchmark selector");
  unsigned warmup = argument(argv[2],0,100), samples = argument(argv[3],1,100), iterations = argument(argv[4],1,1000);
  wasm_rt_init();
  const wasm_rt_trap_t trap = wasm_rt_impl_try();
  if (trap != WASM_RT_TRAP_NONE) { fprintf(stderr,"rbel benchmark: unexpected trap: %s\n",wasm_rt_strerror(trap)); return 1; }
  for (unsigned i = 0; i < warmup; ++i) sample(selector,iterations,0);
  fputs("{\"samples\":[",stdout);
  for (unsigned i = 0; i < samples; ++i) { if (i) putchar(','); sample(selector,iterations,1); }
  fputs("],\"validation\":true}\n",stdout);
  wasm_rt_free(); return 0;
}
