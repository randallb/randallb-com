/* Generic byte-list adapter. The compiled Bel function makes every decision
 * about the transformation. Its module and wasm-rt are linked into this binary. */
#include <errno.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "module.h"
#include "wasm-rt-impl.h"
#include "wasm-rt-exceptions.h"

#define CHUNK_BYTES 256
#define MAX_OUTPUT_BYTES (1024 * 1024)

static uint32_t load32(const uint8_t *p) {
  return (uint32_t)p[0] | ((uint32_t)p[1] << 8) |
         ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

static int transform(const unsigned char *input, size_t count) {
  /* Keep modified module state on the heap so trap recovery does not read
   * indeterminate automatic variables after longjmp. */
  w2c_bel *module = calloc(1, sizeof(*module));
  unsigned char *output = malloc(MAX_OUTPUT_BYTES);
  if (!module || !output) {
    fprintf(stderr, "bel filter: allocation failed\n");
    free(module);
    free(output);
    return 1;
  }
  const wasm_rt_trap_t trap = wasm_rt_impl_try();
  if (trap != WASM_RT_TRAP_NONE) {
    fprintf(stderr, "bel filter: program trapped: %s\n", wasm_rt_strerror(trap));
    if (w2c_bel_memory(module)->data) wasm2c_bel_free(module);
    free(module);
    free(output);
    return 1;
  }
  wasm2c_bel_instantiate(module);
  uint32_t list = 0;
  for (size_t i = count; i > 0; --i)
    list = w2c_bel_join(module, ((uint32_t)input[i - 1] << 3) | 2, list);
  uint32_t result = w2c_bel_choose(module, list);
  size_t size = 0;
  while (result) {
    const wasm_rt_memory_t *memory = w2c_bel_memory(module);
    if (result < 4096 || (result & 7) || (uint64_t)result + 8 > memory->size ||
        size == MAX_OUTPUT_BYTES) {
      fprintf(stderr, "bel filter: result must be a bounded proper byte list\n");
      wasm2c_bel_free(module);
      free(module);
      free(output);
      return 1;
    }
    const uint32_t head = load32(memory->data + result);
    if ((head & 7) != 2 || (head >> 3) > 255) {
      fprintf(stderr, "bel filter: result contains a non-byte character\n");
      wasm2c_bel_free(module);
      free(module);
      free(output);
      return 1;
    }
    output[size++] = (unsigned char)(head >> 3);
    result = load32(memory->data + result + 4);
  }
  /* Validate a complete output chunk before writing any part of that chunk. */
  const int failed = fwrite(output, 1, size, stdout) != size;
  if (failed) fprintf(stderr, "bel filter: stdout: %s\n", strerror(errno));
  wasm2c_bel_free(module);
  free(module);
  free(output);
  return failed;
}

int main(int argc, char **argv) {
  if (argc == 2 && strcmp(argv[1], "--help") == 0) {
    puts("Usage: filter < input > output\nTransforms stdin in independent 256-byte chunks.");
    return 0;
  }
  if (argc != 1) {
    fprintf(stderr, "Usage: %s < input > output\n", argv[0]);
    return 2;
  }
  /* Report a closed pipe as an ordinary I/O failure. */
  signal(SIGPIPE, SIG_IGN);
  wasm_rt_init();
  unsigned char input[CHUNK_BYTES];
  size_t count;
  int status = 0;
  while ((count = fread(input, 1, sizeof(input), stdin)) != 0) {
    if (transform(input, count)) {
      status = 1;
      break;
    }
  }
  if (ferror(stdin)) {
    fprintf(stderr, "bel filter: stdin: %s\n", strerror(errno));
    status = 1;
  }
  if (fflush(stdout) != 0) {
    fprintf(stderr, "bel filter: stdout: %s\n", strerror(errno));
    status = 1;
  }
  wasm_rt_free();
  return status;
}
