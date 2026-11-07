/* ABI2 native value transport helpers. No language evaluation. */
#ifndef RBEL_VALUE_ROOTS_H
#define RBEL_VALUE_ROOTS_H
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#ifndef RBEL_FAILURE
#define RBEL_FAILURE(message) do { fprintf(stderr, "rbel: %s\n", (message)); exit(1); } while (0)
#endif
static uint32_t rb_word(const uint8_t *p) {
  return (uint32_t)p[0] | (uint32_t)p[1]<<8 | (uint32_t)p[2]<<16 | (uint32_t)p[3]<<24;
}
static size_t rb_scalar(uint32_t c, char *out) {
  if (c < 128) { out[0]=(char)c; return 1; }
  if (c < 2048) { out[0]=(char)(192|(c>>6));out[1]=(char)(128|(c&63));return 2; }
  if (c < 65536) { out[0]=(char)(224|(c>>12));out[1]=(char)(128|((c>>6)&63));out[2]=(char)(128|(c&63));return 3; }
  out[0]=(char)(240|(c>>18));out[1]=(char)(128|((c>>12)&63));out[2]=(char)(128|((c>>6)&63));out[3]=(char)(128|(c&63));return 4;
}
static void rb_require(w2c_bel *m, const char *operation) {
  if (w2c_bel_completion_status(m) != 0) {
    char message[8192]; size_t used=0;
    uint32_t function=w2c_bel_error_function(m), site=w2c_bel_error_site(m);
    uint32_t kind=w2c_bel_error_kind(m), value=w2c_bel_error_value(m);
    const wasm_rt_memory_t *memory=w2c_bel_memory(m);
    uint32_t a=w2c_bel_diag_source_addr(m,function), n=w2c_bel_diag_source_len(m,function);
    uint32_t f=w2c_bel_diag_function_addr(m,function), fl=w2c_bel_diag_function_len(m,function);
    if ((uint64_t)a+n<=memory->size && (uint64_t)f+fl<=memory->size && n<2048 && fl<1024)
      used=(size_t)snprintf(message,sizeof(message),"%.*s:%u: %.*s: %s: ",(int)n,(const char *)memory->data+a,
        w2c_bel_diag_line(m,function),(int)fl,(const char *)memory->data+f,operation);
    else used=(size_t)snprintf(message,sizeof(message),"%s: language error: ",operation);
    uint32_t p=value;
    if (w2c_bel_value_kind(m,p)==2) p=w2c_bel_symbol_name_view(m,p);
    while (p && used+8<sizeof(message) && w2c_bel_pair_valid(m,p)) {
      uint32_t c=rb_word(memory->data+p);
      if (w2c_bel_value_kind(m,c)!=3) break;
      used+=rb_scalar(c>>3,message+used);p=rb_word(memory->data+p+4);
    }
    snprintf(message+used,sizeof(message)-used," (kind %u; site %u; form starts here)",kind,site);
    RBEL_FAILURE(message);
  }
}
typedef struct { w2c_bel *owner; uint64_t epoch; } RbOwner;
static RbOwner rb_owners[128];
static uint64_t rb_next_epoch=1;
static uint64_t rb_epoch(w2c_bel *m) {
  for(size_t i=0;i<128;++i) if(rb_owners[i].owner==m) return rb_owners[i].epoch;
  return 0;
}
static void rb_forget(w2c_bel *m) {
  for(size_t i=0;i<128;++i) if(rb_owners[i].owner==m) {rb_owners[i].owner=NULL;rb_owners[i].epoch=0;return;}
}
static void rb_register(w2c_bel *m) {
  if(rb_next_epoch==UINT64_MAX) RBEL_FAILURE("native instance identity exhausted");
  rb_forget(m);
  for(size_t i=0;i<128;++i) if(!rb_owners[i].owner) {
    rb_owners[i].owner=m;rb_owners[i].epoch=rb_next_epoch++;return;
  }
  RBEL_FAILURE("too many live native rbel instances");
}
static void rb_initialize_capacity(w2c_bel *m, uint32_t pairs) {
  if (w2c_bel_rbel_abi_version(m) != 2) RBEL_FAILURE("incompatible rbel value ABI");
  if (w2c_bel_runtime_init(m, pairs, 262144, 65536, 65536, 512) != 0)
    RBEL_FAILURE("rbel runtime initialization failed");
  rb_register(m);
}
typedef struct { w2c_bel *owner; uint64_t epoch, token; } RbRoot;
static void rb_initialize(w2c_bel *m) { rb_initialize_capacity(m,2097152); }
static void rb_initialize_compiler(w2c_bel *m) { rb_initialize_capacity(m,4194304); }
static RbRoot rb_hold(w2c_bel *m, uint32_t value) {
  uint64_t epoch=rb_epoch(m);
  if(!epoch) RBEL_FAILURE("native instance is not registered");
  uint64_t token = w2c_bel_root_retain(m, value);
  if (!token) RBEL_FAILURE("host root capacity exhausted or invalid value");
  RbRoot root = {m,epoch,token};
  if(!root.epoch) RBEL_FAILURE("native instance is not registered");
  return root;
}
static int rb_release_checked(w2c_bel *m, RbRoot root) {
  return root.owner==m && root.epoch && root.epoch==rb_epoch(m) && w2c_bel_root_release(m,root.token);
}
static void rb_drop(w2c_bel *m, RbRoot root) {
  if (!rb_release_checked(m,root)) RBEL_FAILURE("invalid or stale host root");
}
static uint32_t rb_join(w2c_bel *m, uint32_t a, uint32_t d) {
  uint32_t answer = w2c_bel_join(m, a, d); rb_require(m,"join"); return answer;
}
static uint32_t rb_intern(w2c_bel *m, uint32_t name) {
  uint32_t answer = w2c_bel_intern(m,name); rb_require(m,"intern"); return answer;
}
static uint32_t rb_symbolname(w2c_bel *m, uint32_t value) {
  uint32_t answer = w2c_bel_symbolname(m,value); rb_require(m,"symbolname"); return answer;
}
static uint32_t rb_list(w2c_bel *m, const uint32_t *values, size_t count) {
  RbRoot *roots = count ? malloc(count * sizeof(*roots)) : NULL;
  if (count && !roots) RBEL_FAILURE("host root bookkeeping allocation failed");
  for (size_t i=0;i<count;++i) roots[i]=rb_hold(m,values[i]);
  uint32_t answer=0;
  for (size_t i=count;i>0;--i) answer=rb_join(m,values[i-1],answer);
  for (size_t i=0;i<count;++i) rb_drop(m,roots[i]);
  free(roots); return answer;
}
static uint32_t rb_choose(w2c_bel *m, uint32_t value) {
  uint32_t args = rb_join(m,value,0);
  uint32_t answer = w2c_bel_choose_args(m,args); rb_require(m,"entry"); return answer;
}
#endif
