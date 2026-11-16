/* Native command orchestration and value transport. Reader and compiler
 * semantics are compiled from rbel and linked in module.c. */
#define _DARWIN_C_SOURCE
#include <ctype.h>
#include <errno.h>
#include <limits.h>
#include <mach-o/dyld.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>
#include "module.h"
#include "wasm-rt-impl.h"
#include "wasm-rt-exceptions.h"

#define LIMIT (32 * 1024 * 1024)
#define SOURCE_LIMIT (512 * 1024)
#define MAX_FILES 32
#define MAX_NAMES 2048

typedef struct { char *data; size_t size, capacity; } Buffer;
typedef struct { char *name; const char *file; size_t line; int example; const char *display; } Name;
static Name names[MAX_NAMES];
static size_t name_count, example_count;
static Buffer definitions, rows;
static char tempdir[PATH_MAX];
static char bundle[PATH_MAX];
static int quiet;

static void cleanup(void) {
  if (!*tempdir) return;
  const char *files[] = {"module.wat", "module.wasm", "module.c", "module.h", "candidate", NULL};
  char path[PATH_MAX];
  for (int i = 0; files[i]; ++i) {
    snprintf(path, sizeof(path), "%s/%s", tempdir, files[i]); unlink(path);
  }
  rmdir(tempdir); *tempdir = 0;
}
static void fail(const char *format, ...) {
  va_list args; va_start(args, format); fputs("rbel: ", stderr);
  vfprintf(stderr, format, args); fputc('\n', stderr); va_end(args);
  cleanup(); exit(1);
}
#define RBEL_FAILURE(message) fail("%s", (message))
#include "value-roots.h"

static char *formatted(const char *format, ...) {
  va_list args; va_start(args, format); char *text = NULL;
  if (vasprintf(&text, format, args) < 0) { va_end(args); fail("allocation failed"); }
  va_end(args); return text;
}
static void append(Buffer *b, const void *text, size_t size) {
  if (size > LIMIT || b->size > LIMIT - size) fail("text exceeds 32 MiB bound");
  size_t needed = b->size + size + 1;
  if (needed > b->capacity) {
    size_t capacity = b->capacity ? b->capacity : 256;
    while (capacity < needed) capacity *= 2;
    char *data = realloc(b->data, capacity); if (!data) fail("allocation failed");
    b->data = data; b->capacity = capacity;
  }
  if (size) memcpy(b->data + b->size, text, size);
  b->size += size; b->data[b->size] = 0;
}
static void text(Buffer *b, const char *s) { append(b, s, strlen(s)); }
static int blank(const char *s, size_t n) {
  for (size_t i = 0; i < n; ++i) if (!isspace((unsigned char)s[i])) return 0;
  return 1;
}
static void source_utf8(const unsigned char *source, size_t size, const char *path) {
  for(size_t i=0;i<size;) {
    uint32_t c=source[i++], minimum=0;unsigned continuation=0;
    if(c<128) { }
    else if((c&224)==192) { c&=31;continuation=1;minimum=128; }
    else if((c&240)==224) { c&=15;continuation=2;minimum=2048; }
    else if((c&248)==240) { c&=7;continuation=3;minimum=65536; }
    else fail("%s: invalid UTF-8 source",path);
    while(continuation--) {
      if(i==size || (source[i]&192)!=128) fail("%s: invalid UTF-8 source",path);
      c=(c<<6)|(source[i++]&63);
    }
    if(c<minimum || c>1114111 || (c>=55296 && c<=57343)) fail("%s: invalid UTF-8 source",path);
  }
}
static Buffer readfile(const char *path) {
  FILE *file = fopen(path, "rb"); if (!file) fail("%s: %s", path, strerror(errno));
  Buffer b = {0}; char chunk[8192]; size_t count;
  while ((count = fread(chunk, 1, sizeof(chunk), file))) {
    if (b.size + count > SOURCE_LIMIT) fail("%s: source exceeds 512 KiB", path);
    for (size_t i = 0; i < count; ++i)
      if ((unsigned char)chunk[i] < 32 && chunk[i] != '\n' && chunk[i] != '\r' && chunk[i] != '\t')
        fail("%s: unsupported control byte", path);
    append(&b, chunk, count);
  }
  if (ferror(file)) fail("%s: read failed", path);
  fclose(file); if (!b.data) text(&b, ""); source_utf8((const unsigned char *)b.data,b.size,path); return b;
}
static int suffix(const char *s, const char *end) {
  size_t a = strlen(s), b = strlen(end); return a >= b && !strcmp(s + a - b, end);
}
static void addname(const char *name, const char *file, size_t line, int example) {
  if (!name[0] || name[0] < 'a' || name[0] > 'z') fail("%s:%zu: invalid block name", file, line);
  for (const char *p = name; *p; ++p)
    if (!((*p >= 'a' && *p <= 'z') || (*p >= '0' && *p <= '9') || *p == '-'))
      fail("%s:%zu: invalid block name", file, line);
  for (size_t i = 0; i < name_count; ++i)
    if (!strcmp(names[i].name, name)) fail("%s:%zu: duplicate block name %s", file, line, name);
  if (name_count == MAX_NAMES) fail("too many blocks");
  names[name_count++] = (Name){strdup(name), file, line, example, NULL};
  names[name_count-1].display = names[name_count-1].name;
  if (example) ++example_count;
}
/* Return marker length and the first byte following it. */
static size_t fence(const char *line, size_t size, char *marker, size_t *after) {
  size_t start = 0; while (start < size && line[start] == ' ' && start < 4) ++start;
  if (start > 3 || start == size || (line[start] != '`' && line[start] != '~')) return 0;
  *marker = line[start]; size_t end = start;
  while (end < size && line[end] == *marker) ++end;
  if (end - start < 3) return 0;
  *after = end; return end - start;
}
static void extract_file(const char *path) {
  Buffer source = readfile(path);
  if (!suffix(path, ".md")) { append(&definitions, source.data, source.size); text(&definitions, "\n"); free(source.data); return; }
  size_t offset = 0, line_number = 0, start_line = 0, width = 0;
  char marker = 0; int kind = 0; Buffer body = {0}; char *name = NULL;
  size_t file_defs = 0, file_examples = 0;
  while (offset < source.size) {
    const char *line = source.data + offset;
    const char *newline = memchr(line, '\n', source.size - offset);
    size_t full = newline ? (size_t)(newline - line) + 1 : source.size - offset;
    size_t len = full; while (len && (line[len - 1] == '\n' || line[len - 1] == '\r')) --len;
    ++line_number; offset += full;
    char current = 0; size_t after = 0, count = fence(line, len, &current, &after);
    if (width) {
      if (current == marker && count >= width && blank(line + after, len - after)) {
        if (kind && blank(body.data, body.size)) fail("%s:%zu: empty block", path, start_line);
        if (kind == 1) { append(&definitions, body.data, body.size); text(&definitions, "\n"); ++file_defs; }
        if (kind == 2) {
          size_t separator = SIZE_MAX, end_separator = 0;
          for (size_t p = 0; p < body.size;) {
            size_t e = p; while (e < body.size && body.data[e] != '\n') ++e;
            size_t end = e; if (end > p && body.data[end - 1] == '\r') --end;
            if (end - p == 2 && !memcmp(body.data + p, "=>", 2)) {
              if (separator != SIZE_MAX) fail("%s:%zu: multiple => separators", path, start_line);
              separator = p; end_separator = e < body.size ? e + 1 : e;
            }
            p = e < body.size ? e + 1 : e;
          }
          if (separator == SIZE_MAX || blank(body.data, separator) || blank(body.data + end_separator, body.size - end_separator))
            fail("%s:%zu: example requires expression, => line, expected value", path, start_line);
          text(&rows, "("); text(&rows, name); text(&rows, "\n");
          append(&rows, body.data, separator); text(&rows, "\n");
          append(&rows, body.data + end_separator, body.size - end_separator); text(&rows, "\n)\n"); ++file_examples;
        }
        body.size = 0; if (body.data) body.data[0] = 0;
        width = 0; kind = 0; free(name); name = NULL;
      } else if (kind) append(&body, line, full);
      continue;
    }
    if (!count) continue;
    width = count; marker = current; start_line = line_number;
    while (after < len && isspace((unsigned char)line[after])) ++after;
    size_t end = len; while (end > after && isspace((unsigned char)line[end - 1])) --end;
    char *info = strndup(line + after, end - after);
    if (!strncmp(info, "rbel", 4)) {
      const char *label = NULL;
      if (!strncmp(info, "rbel definition=", 16)) { kind = 1; label = info + 16; }
      else if (!strncmp(info, "rbel example=", 13)) { kind = 2; label = info + 13; }
      else fail("%s:%zu: unsupported rbel fence", path, line_number);
      addname(label, path, line_number, kind == 2); name = strdup(label);
    }
    free(info);
  }
  if (width) fail("%s:%zu: unclosed fence", path, start_line);
  if (!file_defs || !file_examples) fail("%s: literate input needs definition and example blocks", path);
  free(body.data); free(source.data);
}
static uint32_t load32(const uint8_t *p) {
  return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}
static uint32_t part(w2c_bel *m, uint32_t x, unsigned offset) {
  const wasm_rt_memory_t *memory = w2c_bel_memory(m);
  if (!w2c_bel_pair_valid(m,x)) fail("invalid value transport pair");
  return load32(memory->data + x + offset);
}
static uint32_t encode(w2c_bel *m, const char *s, size_t size) {
  uint32_t *chars = malloc((size + 1) * sizeof(*chars)); if (!chars) fail("allocation failed");
  size_t used = 0;
  for (size_t i = 0; i < size;) {
    uint32_t c = (unsigned char)s[i++], minimum = 0; int continuation = 0;
    if (c < 128) { }
    else if ((c & 0xe0) == 0xc0) { c &= 31; continuation = 1; minimum = 128; }
    else if ((c & 0xf0) == 0xe0) { c &= 15; continuation = 2; minimum = 2048; }
    else if ((c & 0xf8) == 0xf0) { c &= 7; continuation = 3; minimum = 65536; }
    else fail("source is not valid UTF-8");
    while (continuation--) {
      if (i == size || ((unsigned char)s[i] & 0xc0) != 0x80) fail("source is not valid UTF-8");
      c = (c << 6) | ((unsigned char)s[i++] & 63);
    }
    if (c < minimum || c > 0x10ffff || (c >= 0xd800 && c <= 0xdfff)) fail("source is not valid UTF-8");
    chars[used++] = (c << 3) | 2;
  }
  uint32_t list = 0; while (used) list = rb_join(m, chars[--used], list);
  free(chars); return list;
}
static void utf8(Buffer *b, uint32_t c) {
  unsigned char bytes[4]; size_t n;
  if (c < 128) { bytes[0] = c; n = 1; }
  else if (c < 2048) { bytes[0] = 0xc0 | (c >> 6); bytes[1] = 0x80 | (c & 63); n = 2; }
  else if (c < 65536) { bytes[0] = 0xe0 | (c >> 12); bytes[1] = 0x80 | ((c >> 6) & 63); bytes[2] = 0x80 | (c & 63); n = 3; }
  else if (c <= 0x10ffff) { bytes[0] = 0xf0 | (c >> 18); bytes[1] = 0x80 | ((c >> 12) & 63); bytes[2] = 0x80 | ((c >> 6) & 63); bytes[3] = 0x80 | (c & 63); n = 4; }
  else { fail("invalid character transport"); return; }
  if (c >= 0xd800 && c <= 0xdfff) fail("invalid character transport");
  append(b, bytes, n);
}
static void flatten(w2c_bel *m, uint32_t result, Buffer *out) {
  size_t capacity = 256, size = 1, steps = 0;
  uint32_t *stack = malloc(capacity * sizeof(*stack)); if (!stack) fail("allocation failed");
  stack[0] = result;
  while (size) {
    if (++steps > 33554432) fail("compiler output traversal limit");
    uint32_t x = stack[--size];
    if (!x) continue;
    if ((x & 7) == 2) { utf8(out, x >> 3); continue; }
    uint32_t a = part(m, x, 0), d = part(m, x, 4);
    if (size + 2 > capacity) { capacity *= 2; uint32_t *next = realloc(stack, capacity * sizeof(*stack)); if (!next) fail("allocation failed"); stack = next; }
    stack[size++] = d; stack[size++] = a;
  }
  free(stack);
}
static void escaped(Buffer *out, uint32_t c) {
  char bytes[24];
  if (c > 0x10ffff || (c >= 0xd800 && c <= 0xdfff)) fail("invalid character");
  if (c == '\"' || c == '\\') { char pair[2] = {'\\', (char)c}; append(out, pair, 2); }
  else if (c >= 32 && c < 127) { char one = (char)c; append(out, &one, 1); }
  else if (c <= 0xffff) { snprintf(bytes, sizeof(bytes), "\\u%04x", c); text(out, bytes); }
  else { c -= 65536; snprintf(bytes, sizeof(bytes), "\\u%04x\\u%04x", 0xd800 + (c >> 10), 0xdc00 + (c & 1023)); text(out, bytes); }
}
static void json_value(w2c_bel *m, uint32_t x, unsigned depth, unsigned *steps, Buffer *out) {
  if (depth > 128 || ++*steps > 32768) fail("reader output traversal limit");
  if (!x) { text(out, "null"); return; }
  if ((x & 7) == 2) { text(out, "{\"char\":\""); escaped(out, x >> 3); text(out, "\"}"); return; }
  if ((x & 7) == 1) {
    uint32_t chars = rb_symbolname(m, x); text(out, "\"");
    while (chars) { if (++*steps > 32768) fail("symbol traversal limit"); uint32_t c = part(m, chars, 0); if ((c & 7) != 2) fail("invalid symbol transport"); escaped(out, c >> 3); chars = part(m, chars, 4); }
    text(out, "\""); return;
  }
  text(out, "{\"pair\":["); json_value(m, part(m, x, 0), depth + 1, steps, out); text(out, ",");
  json_value(m, part(m, x, 4), depth + 1, steps, out); text(out, "]}");
}
#include "module-loader.h"
static ModGraph module_graph;
static uint32_t module_text(w2c_bel *m,const char *s) { return encode(m,s,strlen(s)); }
static uint32_t module_symbol(w2c_bel *m,const char *s) { return rb_intern(m,module_text(m,s)); }
static uint32_t module_unit_value(w2c_bel *m,const ModUnit *unit) {
  uint32_t id=module_symbol(m,unit->id); RbRoot idroot=rb_hold(m,id);
  uint32_t label=module_text(m,unit->label); RbRoot labelroot=rb_hold(m,label);
  uint32_t *blocks=calloc(unit->block_count,sizeof(*blocks));
  RbRoot *roots=calloc(unit->block_count,sizeof(*roots));
  if(unit->block_count && (!blocks||!roots)) fail("module value allocation failed");
  for(size_t i=0;i<unit->block_count;++i) {
    uint32_t line=module_text(m,unit->blocks[i].line); RbRoot line_root=rb_hold(m,line);
    uint32_t body=encode(m,unit->blocks[i].text.data,unit->blocks[i].text.size);
    uint32_t fields[]={line,body};blocks[i]=rb_list(m,fields,2);roots[i]=rb_hold(m,blocks[i]);
    rb_drop(m,line_root);
  }
  uint32_t list=rb_list(m,blocks,unit->block_count);
  uint32_t fields[]={id,label,list};uint32_t answer=rb_list(m,fields,3);
  for(size_t i=0;i<unit->block_count;++i) rb_drop(m,roots[i]);
  rb_drop(m,labelroot);rb_drop(m,idroot);free(roots);free(blocks);return answer;
}
static uint32_t module_bundle_value(w2c_bel *m,const ModGraph *graph) {
  uint32_t *items=calloc(MAX_NAMES,sizeof(*items));RbRoot *roots=calloc(MAX_NAMES,sizeof(*roots));
  if(!items||!roots) fail("module bundle allocation failed");
  for(size_t i=0;i<graph->root_count;++i) {
    items[i]=module_symbol(m,graph->units[graph->roots[i]].id);roots[i]=rb_hold(m,items[i]);
  }
  uint32_t root_ids=rb_list(m,items,graph->root_count);RbRoot root_ids_root=rb_hold(m,root_ids);
  for(size_t i=0;i<graph->root_count;++i) rb_drop(m,roots[i]);
  for(size_t i=0;i<graph->unit_count;++i) {items[i]=module_unit_value(m,&graph->units[i]);roots[i]=rb_hold(m,items[i]);}
  uint32_t units=rb_list(m,items,graph->unit_count);RbRoot units_root=rb_hold(m,units);
  for(size_t i=0;i<graph->unit_count;++i) rb_drop(m,roots[i]);
  for(size_t i=0;i<graph->edge_count;++i) {
    const ModEdge *edge=&graph->edges[i];
    uint32_t importer=module_symbol(m,graph->units[edge->importer].id);RbRoot importer_root=rb_hold(m,importer);
    uint32_t path=module_text(m,edge->authored_path);RbRoot path_root=rb_hold(m,path);
    uint32_t target=module_symbol(m,graph->units[edge->target].id);
    uint32_t fields[]={importer,path,target};items[i]=rb_list(m,fields,3);roots[i]=rb_hold(m,items[i]);
    rb_drop(m,path_root);rb_drop(m,importer_root);
  }
  uint32_t edges=rb_list(m,items,graph->edge_count);RbRoot edges_root=rb_hold(m,edges);
  for(size_t i=0;i<graph->edge_count;++i) rb_drop(m,roots[i]);
  size_t count=0;
  for(size_t i=0;i<graph->unit_count;++i) for(size_t j=0;j<graph->units[i].example_count;++j) {
    const ModUnit *unit=&graph->units[i];const ModExample *example=&unit->examples[j];
    uint32_t fields[5];RbRoot holds[4];
    fields[0]=module_symbol(m,unit->id);holds[0]=rb_hold(m,fields[0]);
    fields[1]=module_text(m,example->line);holds[1]=rb_hold(m,fields[1]);
    fields[2]=module_symbol(m,example->name);holds[2]=rb_hold(m,fields[2]);
    fields[3]=encode(m,example->expression.data,example->expression.size);holds[3]=rb_hold(m,fields[3]);
    fields[4]=encode(m,example->expected.data,example->expected.size);
    items[count]=rb_list(m,fields,5);roots[count]=rb_hold(m,items[count]);++count;
    for(size_t k=0;k<4;++k) rb_drop(m,holds[k]);
  }
  uint32_t examples=rb_list(m,items,count);RbRoot examples_root=rb_hold(m,examples);
  for(size_t i=0;i<count;++i) rb_drop(m,roots[i]);
  uint32_t tag=module_symbol(m,"source-bundle-v1");
  uint32_t fields[]={tag,root_ids,units,edges,examples};uint32_t answer=rb_list(m,fields,5);
  rb_drop(m,examples_root);rb_drop(m,edges_root);rb_drop(m,units_root);rb_drop(m,root_ids_root);
  free(roots);free(items);return answer;
}
static char *module_decode_text(w2c_bel *m,uint32_t value) {
  Buffer out={0};
  while(value) {
    if(!w2c_bel_pair_valid(m,value)) fail("module scanner returned invalid metadata");
    uint32_t c=part(m,value,0);if(w2c_bel_value_kind(m,c)!=3 || !(c>>3)) fail("module metadata must be non-NUL text");
    utf8(&out,c>>3);value=part(m,value,4);
  }
  if(!out.data) text(&out,"");return out.data;
}
static int scan_module(ModUnit *unit,void *context) {
  (void)context;w2c_bel *m=calloc(1,sizeof(*m));if(!m) fail("module scanner allocation failed");
  wasm_rt_init();const wasm_rt_trap_t trap=wasm_rt_impl_try();
  if(trap!=WASM_RT_TRAP_NONE) fail("module scanner runtime defect: %s",wasm_rt_strerror(trap));
  wasm2c_bel_instantiate(m);rb_initialize_compiler(m);
  uint32_t value=module_unit_value(m,unit);RbRoot unit_root=rb_hold(m,value);
  uint32_t tag=module_symbol(m,"scan-unit");uint32_t fields[]={tag,value};
  uint32_t rows=rb_choose(m,rb_list(m,fields,2));RbRoot result_root=rb_hold(m,rows);
  while(rows) {
    if(unit->import_count==MAX_FILES || !w2c_bel_pair_valid(m,rows)) fail("module scanner import bound");
    uint32_t row=part(m,rows,0),field[3];
    for(size_t i=0;i<3;++i) {if(!w2c_bel_pair_valid(m,row)) fail("module scanner invalid import row");field[i]=part(m,row,0);row=part(m,row,4);}
    if(row) fail("module scanner invalid import row");
    ModImport *next=realloc(unit->imports,(unit->import_count+1)*sizeof(*next));if(!next) fail("module import allocation failed");
    unit->imports=next;ModImport *item=&next[unit->import_count++];
    item->path=module_decode_text(m,field[0]);item->line=module_decode_text(m,field[2]);item->alias=NULL;
    if(field[1]) {
      if(w2c_bel_value_kind(m,field[1])!=2) fail("module scanner invalid alias");
      item->alias=module_decode_text(m,w2c_bel_symbol_name_view(m,field[1]));
    }
    rows=part(m,rows,4);
  }
  rb_drop(m,result_root);rb_drop(m,unit_root);rb_forget(m); wasm2c_bel_free(m);free(m);wasm_rt_free();return 0;
}

static Buffer core(const char *action) {
  Buffer out = {0}; w2c_bel *m = calloc(1, sizeof(*m)); if (!m) fail("allocation failed");
  wasm_rt_init(); const wasm_rt_trap_t trap = wasm_rt_impl_try();
  if (trap != WASM_RT_TRAP_NONE) {
    if (w2c_bel_memory(m)->data) { rb_forget(m); wasm2c_bel_free(m); } free(m); wasm_rt_free();
    fail("%s: unexpected reader/compiler runtime trap (%s)", action, wasm_rt_strerror(trap));
  }
  wasm2c_bel_instantiate(m); rb_initialize_compiler(m);
  uint32_t source_bundle = module_bundle_value(m,&module_graph);
  RbRoot bundle_root = rb_hold(m,source_bundle);
  const char *operation = !strcmp(action,"read") ? "read-bundle" : !strcmp(action,"check") ? "check-bundle" : "compile-bundle";
  uint32_t tag = module_symbol(m,operation);
  uint32_t fields[] = {tag,source_bundle};
  uint32_t result = rb_choose(m,rb_list(m,fields,2));
  RbRoot result_root = rb_hold(m,result); rb_drop(m,bundle_root);
  if (!strcmp(action, "read")) { unsigned steps = 0; json_value(m, result, 0, &steps, &out); text(&out, "\n"); }
  else flatten(m, result, &out);
  rb_drop(m,result_root);
  rb_forget(m); wasm2c_bel_free(m); free(m); wasm_rt_free(); return out;
}
static int run(char *const argv[]) {
  pid_t pid = fork(); if (pid < 0) fail("fork failed");
  if (!pid) { execv(argv[0], argv); fprintf(stderr, "rbel: cannot execute %s: %s\n", argv[0], strerror(errno)); _exit(127); }
  int status; while (waitpid(pid, &status, 0) < 0) if (errno != EINTR) fail("wait failed");
  return WIFEXITED(status) ? WEXITSTATUS(status) : 1;
}
static void writefile(const char *path, const Buffer *b) {
  FILE *file = fopen(path, "wb"); if (!file) fail("%s: %s", path, strerror(errno));
  if (b->size && fwrite(b->data, 1, b->size, file) != b->size) fail("%s: write failed", path);
  if (fclose(file)) fail("%s: close failed", path);
}
static char *resource(const char *name) { return formatted("%s/share/rbel/%s", bundle, name); }
static void compile_native(const Buffer *wat, int checking) {
  char *watpath = formatted("%s/module.wat", tempdir), *wasm = formatted("%s/module.wasm", tempdir);
  char *code = formatted("%s/module.c", tempdir), *candidate = formatted("%s/candidate", tempdir);
  writefile(watpath, wat);
  char *assembler = formatted("%s/libexec/rbel/wat2wasm", bundle);
  char *validator = formatted("%s/libexec/rbel/wasm-validate", bundle);
  char *translator = formatted("%s/libexec/rbel/wasm2c", bundle);
  char *assemble[] = {assembler, watpath, "-o", wasm, NULL};
  char *validate[] = {validator, wasm, NULL};
  char *translate[] = {translator, wasm, "-n", "bel", "-o", code, NULL};
  if (run(assemble) || run(validate) || run(translate)) fail("WABT compilation failed");
  char *runtime = resource("runtime"), *adapter = resource(checking ? "check-main.c" : "filter-main.c");
  char *impl = formatted("%s/wasm-rt-impl.c", runtime), *mem = formatted("%s/wasm-rt-mem-impl.c", runtime);
  char *exceptions = formatted("%s/wasm-rt-exceptions-impl.c", runtime);
  char *cc[] = {"/usr/bin/clang", "-std=c99", "-O2", "-Wall", "-Wextra", "-Werror",
    "-Wno-unused-function", "-Wno-unused-parameter", "-Wno-unused-but-set-variable",
    "-fno-optimize-sibling-calls", "-frounding-math", "-DWASM_RT_USE_MMAP=0",
    "-DWASM_RT_MEMCHECK_BOUNDS_CHECK=1", "-DWASM_RT_MEMCHECK_GUARD_PAGES=0",
    "-DWASM_RT_STACK_DEPTH_COUNT=1", "-DWASM_RT_STACK_EXHAUSTION_HANDLER=0",
    "-DWASM_RT_MAX_CALL_STACK_DEPTH=4096", "-I", runtime, "-I", tempdir,
    adapter, code, impl, mem, exceptions, "-lm", "-pthread", "-o", candidate, NULL};
  if (run(cc)) fail("native C compilation failed");
  free(watpath); free(wasm); free(code); free(candidate); free(assembler); free(validator); free(translator);
  free(runtime); free(adapter); free(impl); free(mem); free(exceptions);
}
static void find_bundle(void) {
  char raw[PATH_MAX]; uint32_t size = sizeof(raw);
  if (_NSGetExecutablePath(raw, &size) || !realpath(raw, bundle)) fail("cannot locate command bundle");
  char *slash = strrchr(bundle, '/'); if (!slash) fail("invalid command path"); *slash = 0;
  slash = strrchr(bundle, '/'); if (!slash) fail("invalid bundle path"); *slash = 0;
}
static char *canonical(const char *path) {
  char resolved[PATH_MAX]; if (realpath(path, resolved)) return strdup(resolved);
  char *copy = strdup(path), *slash = strrchr(copy, '/'); const char *base = copy; const char *parent = ".";
  if (slash) { *slash = 0; base = slash + 1; parent = *copy ? copy : "/"; }
  if (!*base || !strcmp(base, ".") || !strcmp(base, "..") || !realpath(parent, resolved)) fail("%s: invalid output parent", path);
  char *result = formatted("%s/%s", resolved, base); free(copy); return result;
}
static void make_temp(const char *output) {
  char parent[PATH_MAX];
  if (output) { if (strlen(output) >= sizeof(parent)) fail("path too long"); strcpy(parent, output); char *slash = strrchr(parent, '/'); if (!slash) fail("invalid output path"); *slash = 0; }
  else if (!getcwd(parent, sizeof(parent))) fail("cannot locate current directory");
  if (snprintf(tempdir, sizeof(tempdir), "%s/.rbel-build-XXXXXX", parent) >= (int)sizeof(tempdir) || !mkdtemp(tempdir)) fail("cannot create build directory: %s", strerror(errno));
}
static void help(void) {
  puts("Usage: rbel build SOURCE [--include SOURCE ...] -o OUTPUT\n"
       "       rbel check SOURCE [--include SOURCE ...]\n"
       "       rbel emit SOURCE [--include SOURCE ...] -o OUTPUT.wat\n"
       "       rbel read SOURCE\n"
       "Literate .md input uses rbel definition=NAME and example=NAME fences.\n"
       "build produces a native byte-list stdin/stdout filter; emit writes Wasm text.");
}
int main(int argc, char **argv) {
  if (argc == 2 && !strcmp(argv[1], "--abi-version")) { puts("2"); return 0; }
  atexit(cleanup);
  if (argc == 2 && (!strcmp(argv[1], "--help") || !strcmp(argv[1], "--version"))) {
    if (!strcmp(argv[1], "--version")) puts("rbel 0.1 native-command"); else help(); return 0;
  }
  if (argc < 3 || (strcmp(argv[1], "build") && strcmp(argv[1], "check") && strcmp(argv[1], "emit") && strcmp(argv[1], "read"))) { help(); return 2; }
  const char *mode = argv[1], *sources[MAX_FILES]; size_t count = 1; sources[0] = argv[2];
  const char *output_arg = NULL;
  for (int i = 3; i < argc; ++i) {
    if (!strcmp(argv[i], "--include") && i + 1 < argc && count < MAX_FILES) sources[count++] = argv[++i];
    else if (!strcmp(argv[i], "-o") && i + 1 < argc && !output_arg) output_arg = argv[++i];
    else if (!strcmp(argv[i], "--quiet")) quiet = 1;
    else { help(); return 2; }
  }
  int producing = !strcmp(mode, "build") || !strcmp(mode, "emit");
  if (!!output_arg != producing) { help(); return 2; }
  find_bundle(); char *output = NULL;
  mod_load_graph(&module_graph,sources,count,scan_module,NULL);
  size_t example_index=0;
  for(size_t i=0;i<module_graph.unit_count;++i) for(size_t j=0;j<module_graph.units[i].example_count;++j) {
    char selector[64];snprintf(selector,sizeof(selector),"mod-example-%zu",example_index++);
    const ModExample *example=&module_graph.units[i].examples[j];
    addname(selector,module_graph.units[i].label,(size_t)strtoul(example->line,NULL,10),1);
    names[name_count-1].display = example->name;
  }
  if (producing) {
    output = canonical(output_arg);
    if (suffix(output, ".md") || suffix(output, ".rbel") || suffix(output, ".bel") || suffix(output, ".c") || suffix(output, ".h")) fail("output has a protected source extension");
    if (!strncmp(output, bundle, strlen(bundle)) && output[strlen(bundle)] == '/') fail("output must not overwrite command bundle resources");
    for (size_t i = 0; i < module_graph.unit_count; ++i) {
      char *source = canonical(module_graph.units[i].path); struct stat a, b;
      int alias = !strcmp(source, output) || (!stat(source, &a) && !stat(output, &b) && a.st_dev == b.st_dev && a.st_ino == b.st_ino);
      free(source); if (alias) fail("output must not overwrite source");
    }
    if (unlink(output) && errno != ENOENT) fail("cannot clear old output: %s", strerror(errno));
  }
  if (!strcmp(mode, "read")) { Buffer answer = core("read"); int failed = fwrite(answer.data, 1, answer.size, stdout) != answer.size; free(answer.data); return failed || fflush(stdout) ? 1 : 0; }
  make_temp(output);
  if (example_count) {
    Buffer checks = core("check"); compile_native(&checks, 1); free(checks.data);
    char *binary = formatted("%s/candidate", tempdir);
    for (size_t i = 0; i < name_count; ++i) if (names[i].example) {
      char *args[] = {binary, names[i].name, NULL};
      if (run(args)) fail("%s:%zu: example %s failed", names[i].file, names[i].line, names[i].display);
    }
    free(binary); if (!quiet) fprintf(stderr, "checked %zu examples\n", example_count);
  } else if (!strcmp(mode, "check")) fail("no examples to check");
  if (!strcmp(mode, "check")) { cleanup(); return 0; }
  Buffer product = core("compile");
  if (!strcmp(mode, "emit")) { char *candidate = formatted("%s/candidate", tempdir); writefile(candidate, &product); if (rename(candidate, output)) fail("cannot publish Wasm text: %s", strerror(errno)); free(candidate); }
  else { compile_native(&product, 0); char *candidate = formatted("%s/candidate", tempdir); if (rename(candidate, output)) fail("cannot publish executable: %s", strerror(errno)); free(candidate); }
  free(product.data); cleanup(); if (!quiet) printf("built %s\n", output_arg); free(output); return 0;
}
