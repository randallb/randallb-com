/* File/fence transport for the compiled literate rbel lint rules.
 * Included after the native command's shared file/value helpers. */
typedef struct { size_t line, count; char *label; } LintBlock;
static uint32_t lint_number(w2c_bel *m, size_t number) {
  char digits[32]; int size = snprintf(digits, sizeof(digits), "%zu", number);
  return encode(m, digits, (size_t)size);
}
static uint32_t lint_list(w2c_bel *m, uint32_t a, uint32_t b) {
  return w2c_bel_join(m, a, w2c_bel_join(m, b, 0));
}
static int lint_option(const char *s, size_t *value) {
  if (!*s) return 0;
  size_t n = 0;
  for (; *s; ++s) {
    if (*s < '0' || *s > '9' || n > (SOURCE_LIMIT - (size_t)(*s - '0')) / 10) return 0;
    n = n * 10 + (size_t)(*s - '0');
  }
  *value = n; return 1;
}
static int lint_command(int argc, char **argv) {
  if (argc < 3) { fputs("Usage: rbel lint SOURCE [--file-limit N] [--block-limit N]\n", stderr); return 2; }
  size_t file_limit = 500, block_limit = 80; int got_file = 0, got_block = 0;
  for (int i = 3; i < argc; ++i) {
    size_t *limit = NULL;
    if (!strcmp(argv[i], "--file-limit") && !got_file++) limit = &file_limit;
    else if (!strcmp(argv[i], "--block-limit") && !got_block++) limit = &block_limit;
    if (!limit || i + 1 == argc || !lint_option(argv[++i], limit)) {
      fprintf(stderr, "rbel: lint expects one source and unique limits from 0 to %d\n", SOURCE_LIMIT); return 2;
    }
  }
  const char *path = argv[2]; Buffer source = readfile(path), out = {0};
  LintBlock *blocks = calloc(MAX_NAMES, sizeof(*blocks)); if (!blocks) fail("allocation failed");
  size_t offset = 0, lines = 0, width = 0, start = 0, body = 0, block_count = 0;
  char marker = 0; char *label = NULL;
  while (offset < source.size) {
    const char *line = source.data + offset, *newline = memchr(line, '\n', source.size - offset);
    size_t full = newline ? (size_t)(newline - line) + 1 : source.size - offset;
    size_t len = full; while (len && (line[len-1] == '\n' || line[len-1] == '\r')) --len;
    offset += full; ++lines;
    if (!suffix(path, ".md")) continue;
    char current = 0; size_t after = 0, count = fence(line, len, &current, &after);
    if (width) {
      if (current == marker && count >= width && blank(line + after, len - after)) {
        if (label) {
          if (block_count == MAX_NAMES) fail("%s: too many rbel blocks", path);
          blocks[block_count++] = (LintBlock){start, body, label}; label = NULL;
        }
        width = 0;
      } else ++body;
      continue;
    }
    if (!count) continue;
    width = count; marker = current; start = lines; body = 0;
    while (after < len && isspace((unsigned char)line[after])) ++after;
    size_t end = len; while (end > after && isspace((unsigned char)line[end-1])) --end;
    if (end - after >= 4 && !memcmp(line + after, "rbel", 4) &&
        (end - after == 4 || isspace((unsigned char)line[after+4]))) {
      label = strndup(line + after, end - after); if (!label) fail("allocation failed");
    }
  }
  if (width) fail("%s:%zu: unclosed fence", path, start);
  w2c_bel *m = calloc(1, sizeof(*m)); if (!m) fail("allocation failed");
  wasm_rt_init(); const wasm_rt_trap_t trap = wasm_rt_impl_try();
  if (trap != WASM_RT_TRAP_NONE) {
    if (w2c_bel_memory(m)->data) wasm2c_bel_free(m); free(m); wasm_rt_free();
    fail("lint: rule execution reached a resource bound (%s)", wasm_rt_strerror(trap));
  }
  wasm2c_bel_instantiate(m);
  /* Validate UTF-8 using the same transport boundary as read/build. */
  (void)encode(m, source.data, source.size);
  uint32_t entries = 0;
  for (size_t i = block_count; i; --i)
    entries = w2c_bel_join(m, lint_list(m, lint_number(m, i-1), lint_number(m, blocks[i-1].count)), entries);
  uint32_t input = w2c_bel_join(m, lint_number(m, lines),
    w2c_bel_join(m, lint_number(m, file_limit), w2c_bel_join(m, lint_number(m, block_limit), w2c_bel_join(m, entries, 0))));
  uint32_t tag = w2c_bel_intern(m, encode(m, "lint", 4));
  uint32_t result = w2c_bel_choose(m, lint_list(m, tag, input));
  size_t findings = 0;
  while (result) {
    if (++findings > block_count + 1) fail("lint: invalid finding list");
    uint32_t finding = part(m, result, 0), kind = part(m, finding, 0);
    Buffer name = {0}; flatten(m, w2c_bel_symbolname(m, kind), &name);
    char *warning;
    if (!strcmp(name.data, "file")) {
      warning = formatted("%s:1: warning: file has %zu lines (limit %zu)\n", path, lines, file_limit);
    } else if (!strcmp(name.data, "block")) {
      uint32_t location = part(m, part(m, finding, 4), 0); Buffer digits = {0}; flatten(m, location, &digits);
      size_t index = 0;
      if (!digits.data || !lint_option(digits.data, &index) || index >= block_count) fail("lint: invalid block location");
      LintBlock b = blocks[index];
      warning = formatted("%s:%zu: warning: %s block has %zu lines (limit %zu)\n", path, b.line, b.label, b.count, block_limit);
      free(digits.data);
    } else { fail("lint: invalid finding kind"); return 1; }
    text(&out, warning); free(warning); free(name.data); result = part(m, result, 4);
  }
  wasm2c_bel_free(m); free(m); wasm_rt_free();
  for (size_t i = 0; i < block_count; ++i) free(blocks[i].label);
  free(blocks); free(source.data);
  int failed = out.size && fwrite(out.data, 1, out.size, stdout) != out.size;
  free(out.data); return failed || fflush(stdout) ? 1 : 0;
}
