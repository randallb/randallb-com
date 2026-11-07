/* File snapshots and fence extraction only. rbel scan-unit owns import semantics.
 * Include after Buffer, append(), readfile(), fail(), MAX_FILES and PATH_MAX.
 * scan(unit,context) fills imports and returns 0 on success. */
typedef struct { char *line; Buffer text; } ModBlock;
typedef struct { char *line, *name; Buffer expression, expected; } ModExample;
typedef struct { char *path, *alias, *line; } ModImport;
typedef struct {
  char *id, *path, *label;
  dev_t device; ino_t inode; int active;
  Buffer source;
  ModBlock *blocks; size_t block_count;
  ModExample *examples; size_t example_count;
  ModImport *imports; size_t import_count;
} ModUnit;
typedef struct { size_t importer, target; char *authored_path; } ModEdge;
typedef struct {
  ModUnit units[MAX_FILES]; size_t unit_count;
  size_t roots[MAX_FILES], root_count;
  ModEdge edges[MAX_FILES * MAX_FILES]; size_t edge_count;
  size_t source_bytes, example_count;
  char base[PATH_MAX];
} ModGraph;

static char *mod_copy(const char *s, size_t n) {
  char *p = malloc(n + 1); if (!p) fail("module loader allocation failed");
  memcpy(p, s, n); p[n] = 0; return p;
}
static char *mod_decimal(size_t value) {
  char buffer[32]; snprintf(buffer, sizeof(buffer), "%zu", value); return strdup(buffer);
}
static int mod_blank(const char *s, size_t n) {
  for (size_t i = 0; i < n; ++i) if (!isspace((unsigned char)s[i])) return 0;
  return 1;
}
static size_t mod_fence(const char *s, size_t n, char *marker, size_t *after) {
  size_t p = 0; while (p < n && p < 4 && s[p] == ' ') ++p;
  if (p > 3 || p == n || (s[p] != '`' && s[p] != '~')) return 0;
  size_t start = p; *marker = s[p]; while (p < n && s[p] == *marker) ++p;
  if (p - start < 3) return 0; *after = p; return p - start;
}
static int mod_label_valid(const char *name) {
  if (!name[0] || name[0] < 'a' || name[0] > 'z') return 0;
  for (size_t p = 1; name[p]; ++p)
    if (!((name[p] >= 'a' && name[p] <= 'z') || (name[p] >= '0' && name[p] <= '9') || name[p] == '-')) return 0;
  return 1;
}
static void mod_example(ModUnit *unit, const char *name, size_t first_line, const char *body, size_t n) {
  size_t separator = SIZE_MAX, end = 0;
  for (size_t p = 0; p < n;) {
    size_t e = p; while (e < n && body[e] != '\n') ++e;
    size_t logical = e; if (logical > p && body[logical - 1] == '\r') --logical;
    if (logical - p == 2 && !memcmp(body + p, "=>", 2)) {
      if (separator != SIZE_MAX) fail("%s:%zu: multiple example separators", unit->label, first_line);
      separator = p; end = e < n ? e + 1 : e;
    }
    p = e < n ? e + 1 : e;
  }
  if (separator == SIZE_MAX || mod_blank(body, separator) || mod_blank(body + end, n - end))
    fail("%s:%zu: malformed example %s", unit->label, first_line, name);
  if (unit->example_count >= 2048) fail("%s: too many examples", unit->label);
  ModExample *next = realloc(unit->examples, (unit->example_count + 1) * sizeof(*next));
  if (!next) fail("module loader allocation failed"); unit->examples = next;
  ModExample *example = &next[unit->example_count++]; memset(example, 0, sizeof(*example));
  example->line = mod_decimal(first_line); example->name = strdup(name);
  append(&example->expression, body, separator); append(&example->expected, body + end, n - end);
}
static void mod_extract(ModUnit *unit) {
  const char *source = unit->source.data; size_t n = unit->source.size;
  if (memchr(source, 0, n)) fail("%s: source contains NUL; use a scalar escape", unit->label);
  size_t path_length = strlen(unit->path);
  if (path_length < 3 || strcmp(unit->path + path_length - 3, ".md")) {
    unit->blocks = calloc(1, sizeof(*unit->blocks)); if (!unit->blocks) fail("module loader allocation failed");
    unit->block_count = 1; unit->blocks[0].line = strdup("1"); append(&unit->blocks[0].text, source, n); return;
  }
  size_t p = 0, line = 1, width = 0, first_line = 0, body_start = 0;
  char marker = 0, *name = NULL; int kind = 0;
  char **names = NULL; size_t name_count = 0;
  while (p < n) {
    size_t e = p; while (e < n && source[e] != '\n') ++e;
    size_t logical = e; if (logical > p && source[logical - 1] == '\r') --logical;
    char current = 0; size_t after = 0, count = mod_fence(source + p, logical - p, &current, &after);
    if (width) {
      if (count >= width && current == marker && mod_blank(source + p + after, logical - p - after)) {
        if (kind) {
          size_t body_size = p - body_start;
          if (mod_blank(source + body_start, body_size)) fail("%s:%zu: empty block %s", unit->label, first_line, name);
          if (kind == 1) {
            if (unit->block_count >= 2048) fail("%s: too many source blocks", unit->label);
            ModBlock *next = realloc(unit->blocks, (unit->block_count + 1) * sizeof(*next));
            if (!next) fail("module loader allocation failed"); unit->blocks = next;
            ModBlock *block = &next[unit->block_count++]; memset(block, 0, sizeof(*block));
            block->line = mod_decimal(first_line); append(&block->text, source + body_start, body_size);
          } else mod_example(unit, name, first_line, source + body_start, body_size);
        }
        width = 0; kind = 0; free(name); name = NULL;
      }
    } else if (count) {
      width = count; marker = current; first_line = line + 1; body_start = e < n ? e + 1 : e;
      size_t begin = p + after; while (begin < logical && isspace((unsigned char)source[begin])) ++begin;
      size_t finish = logical; while (finish > begin && isspace((unsigned char)source[finish - 1])) --finish;
      char *info = mod_copy(source + begin, finish - begin); const char *label = NULL;
      if (!strncmp(info, "rbel definition=", 16)) { kind = 1; label = info + 16; }
      else if (!strncmp(info, "rbel example=", 13)) { kind = 2; label = info + 13; }
      else if (!strncmp(info, "rbel", 4)) fail("%s:%zu: unsupported rbel fence", unit->label, line);
      if (label) {
        if (!mod_label_valid(label)) fail("%s:%zu: invalid block name", unit->label, line);
        for (size_t i = 0; i < name_count; ++i) if (!strcmp(names[i], label)) fail("%s:%zu: duplicate block %s", unit->label, line, label);
        if (name_count >= 2048) fail("%s: too many source blocks", unit->label);
        char **next = realloc(names, (name_count + 1) * sizeof(*next)); if (!next) fail("module loader allocation failed");
        names = next; names[name_count++] = strdup(label); name = strdup(label);
      }
      free(info);
    }
    p = e < n ? e + 1 : e; ++line;
  }
  if (width) fail("%s:%zu: unclosed fence", unit->label, first_line - 1);
  for (size_t i = 0; i < name_count; ++i) free(names[i]); free(names);
  if (!unit->block_count) fail("%s: literate source requires a definition block", unit->label);
}
static char *mod_display_label(const char *base, const char *path) {
  size_t n = strlen(base);
  if (n == 1 && base[0] == '/') return strdup(path + 1);
  if (!strncmp(base, path, n) && path[n] == '/') return strdup(path + n + 1);
  size_t p = 0, common = 0;
  while (base[p] && path[p] && base[p] == path[p]) { if (base[p] == '/') common = p + 1; ++p; }
  Buffer label = {0};
  for (p = common; base[p];) {
    while (base[p] == '/') ++p;
    if (!base[p]) break;
    append(&label, "../", 3); while (base[p] && base[p] != '/') ++p;
  }
  append(&label, path + common, strlen(path + common)); return label.data;
}
static size_t mod_load_unit(ModGraph *graph, const char *path, const char *importer, const char *authored, const char *line,
                           int (*scan)(ModUnit *, void *), void *context) {
  char resolved[PATH_MAX]; struct stat st;
  if (!realpath(path, resolved) || stat(resolved, &st)) {
    if (importer) fail("%s:%s: import %s: %s", importer, line, authored, strerror(errno));
    fail("%s: %s", path, strerror(errno));
  }
  if (!S_ISREG(st.st_mode)) fail("%s: source must be a regular file", resolved);
  for (size_t i = 0; i < graph->unit_count; ++i) {
    ModUnit *prior = &graph->units[i];
    if (prior->device == st.st_dev && prior->inode == st.st_ino) {
      if (prior->active) fail("%s:%s: import cycle through %s", importer ? importer : resolved, line ? line : "1", authored ? authored : resolved);
      return i;
    }
  }
  if (graph->unit_count == MAX_FILES) fail("too many source files (maximum 32)");
  size_t index = graph->unit_count++; ModUnit *unit = &graph->units[index]; memset(unit, 0, sizeof(*unit));
  unit->device = st.st_dev; unit->inode = st.st_ino; unit->path = strdup(resolved); unit->active = 1;
  char id[32]; snprintf(id, sizeof(id), "unit-%zu", index); unit->id = strdup(id);
  unit->label = mod_display_label(graph->base, resolved);
  unit->source = readfile(resolved); graph->source_bytes += unit->source.size;
  if (graph->source_bytes > 512 * 1024) fail("combined source exceeds 512 KiB");
  mod_extract(unit);
  graph->example_count += unit->example_count; if (graph->example_count > 2048) fail("too many executable examples");
  if (scan(unit, context)) fail("%s: rbel scan-unit failed", unit->label);
  if (unit->import_count > MAX_FILES) fail("%s: too many imports", unit->label);
  for (size_t i = 0; i < unit->import_count; ++i) {
    ModImport *item = &unit->imports[i];
    if (!item->path || !*item->path || item->path[0] == '/' || item->path[0] == '~' || strchr(item->path, ':'))
      fail("%s:%s: import requires a relative path", unit->label, item->line ? item->line : "0");
    for (const unsigned char *c = (const unsigned char *)item->path; *c; ++c)
      if (*c < 32 || *c == 127) fail("%s:%s: invalid import path", unit->label, item->line);
    char dependency[PATH_MAX]; char *slash = strrchr(resolved, '/');
    if (snprintf(dependency, sizeof(dependency), "%.*s/%s", (int)(slash - resolved), resolved, item->path) >= (int)sizeof(dependency)) fail("import path too long");
    size_t target = mod_load_unit(graph, dependency, unit->label, item->path, item->line, scan, context);
    int present = 0;
    for (size_t j = 0; j < graph->edge_count; ++j)
      if (graph->edges[j].importer == index && !strcmp(graph->edges[j].authored_path, item->path)) present = 1;
    if (!present) {
      if (graph->edge_count >= MAX_FILES * MAX_FILES) fail("too many import edges");
      graph->edges[graph->edge_count++] = (ModEdge){index, target, strdup(item->path)};
    }
  }
  unit->active = 0; return index;
}
static void mod_load_graph(ModGraph *graph, const char **paths, size_t count, int (*scan)(ModUnit *, void *), void *context) {
  if (!count || count > MAX_FILES) fail("invalid number of source roots");
  memset(graph, 0, sizeof(*graph));
  char root[PATH_MAX]; if (!realpath(paths[0], root)) fail("%s: %s", paths[0], strerror(errno));
  char *slash = strrchr(root, '/'); if (!slash) fail("invalid source root");
  size_t length = (size_t)(slash - root); if (!length) length = 1;
  memcpy(graph->base, root, length); graph->base[length] = 0;
  for (size_t i = 0; i < count; ++i) graph->roots[graph->root_count++] = mod_load_unit(graph, paths[i], NULL, NULL, NULL, scan, context);
}
static void mod_free_graph(ModGraph *graph) {
  for (size_t i = 0; i < graph->unit_count; ++i) {
    ModUnit *unit = &graph->units[i]; free(unit->id); free(unit->path); free(unit->label); free(unit->source.data);
    for (size_t j = 0; j < unit->block_count; ++j) { free(unit->blocks[j].line); free(unit->blocks[j].text.data); }
    for (size_t j = 0; j < unit->example_count; ++j) { ModExample *e = &unit->examples[j]; free(e->line); free(e->name); free(e->expression.data); free(e->expected.data); }
    for (size_t j = 0; j < unit->import_count; ++j) { free(unit->imports[j].path); free(unit->imports[j].alias); free(unit->imports[j].line); }
    free(unit->blocks); free(unit->examples); free(unit->imports);
  }
  for (size_t i = 0; i < graph->edge_count; ++i) free(graph->edges[i].authored_path);
  memset(graph, 0, sizeof(*graph));
}
