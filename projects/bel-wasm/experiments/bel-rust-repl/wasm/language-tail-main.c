/* Native control-flow acceptance. Sibling-call optimization is disabled. */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "module.h"
#include "wasm-rt-impl.h"
#include "wasm-rt-exceptions.h"
static uint32_t word(const uint8_t *p) {
 return (uint32_t)p[0] | ((uint32_t)p[1]<<8) | ((uint32_t)p[2]<<16) | ((uint32_t)p[3]<<24);
}
static int symbol_is(w2c_bel *m, uint32_t value, const char *text) {
 uint32_t chars=w2c_bel_symbolname(m,value);
 if(w2c_bel_completion_status(m))return 0;
 for(size_t i=0;text[i];i++) {
  if(!w2c_bel_pair_valid(m,chars))return 0;
  const uint8_t *cell=w2c_bel_memory(m)->data+chars;
  if(word(cell)!=(((uint32_t)(unsigned char)text[i]<<3)|2))return 0;
  chars=word(cell+4);
 }
 return chars==0;
}
int main(void) {
 wasm_rt_init();
 w2c_bel *m=calloc(1,sizeof(*m));
 if(!m)return 2;
 wasm_rt_trap_t trap=wasm_rt_impl_try();
 if(trap!=WASM_RT_TRAP_NONE){fprintf(stderr,"unexpected trap: %s\n",wasm_rt_strerror(trap));return 3;}
 wasm2c_bel_instantiate(m);
#ifdef RBEL_TEST_NONTAIL
 if(w2c_bel_runtime_init(m,4096,32768,256,256,512))return 4;
 uint32_t value=w2c_bel_choose(m,1);
 if(w2c_bel_completion_status(m) || !symbol_is(m,value,"call-depth-exhausted") || w2c_bel_stat_root_top(m))return 5;
 value=w2c_bel_choose(m,0);
 if(w2c_bel_completion_status(m) || !symbol_is(m,value,"recovered") || w2c_bel_stat_root_top(m) || w2c_bel_stat_max_calls(m)!=512)return 6;
#else
 if(w2c_bel_runtime_init(m,1200000,4096,256,256,8))return 4;
 uint32_t xs=0;
 for(unsigned i=0;i<1000000;i++) {
  xs=w2c_bel_join(m,0,xs);
  if(w2c_bel_completion_status(m))return 5;
 }
 uint32_t value=w2c_bel_choose(m,xs);
 if(value!=1 || w2c_bel_completion_status(m) || w2c_bel_stat_root_top(m) || w2c_bel_stat_max_calls(m)>3)return 6;
#endif
 printf("{\"passed\":true,\"maximum_calls\":%u,\"maximum_roots\":%u,\"tail_transfers\":%u,\"collections\":%u}\n",w2c_bel_stat_max_calls(m),w2c_bel_stat_max_roots(m),w2c_bel_stat_tail_transfers(m),w2c_bel_stat_collections(m));
 wasm2c_bel_free(m);free(m);wasm_rt_free();return 0;
}
