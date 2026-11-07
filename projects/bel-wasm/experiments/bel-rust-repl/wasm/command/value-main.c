/* Generic JSON value ABI for an ordinary compiled rbel program. */
#include "native-values.c"
#include <stdlib.h>
#include <string.h>
#include "module.h"
#include "wasm-rt-impl.h"
#include "wasm-rt-exceptions.h"

#define NODE_BOUND 262144u
#define DEPTH_BOUND 128u
#define SYMBOL_CACHE_BOUND 4096u
typedef struct { size_t n; uint32_t ids[SYMBOL_CACHE_BOUND]; V *values[SYMBOL_CACHE_BOUND]; } Symbols;
static void count(size_t *nodes,unsigned depth){if(++*nodes>NODE_BOUND||depth>DEPTH_BOUND)v_fail("core value resource bound");}
static uint32_t encode(w2c_bel *module,V *value,unsigned depth,size_t *nodes,Symbols *symbols){
  count(nodes,depth);if(!value)return 0;
  if(value->type==VT_STRING){
    for(size_t i=0;i<symbols->n;i++) {
      V *known=symbols->values[i];
      if(known->n==value->n&&!memcmp(known->s,value->s,value->n))return symbols->ids[i];
    }
    uint32_t *characters=v_alloc((value->n+1)*sizeof(*characters));size_t n=0;
    for(size_t position=0;position<value->n;){if(n>=4096)v_fail("symbol name bound");characters[n++]=v_scalar(value->s,value->n,&position);count(nodes,depth+1);}
    uint32_t name=0;for(size_t i=n;i>0;i--)name=w2c_bel_join(module,(characters[i-1]<<3)|2,name);
    uint32_t result=w2c_bel_intern(module,name);
    if(symbols->n<SYMBOL_CACHE_BOUND){symbols->ids[symbols->n]=result;symbols->values[symbols->n++]=value;}
    return result;
  }
  if(value->type==VT_ARRAY){
    uint32_t *items=v_alloc((value->n+1)*sizeof(*items));
    for(size_t i=0;i<value->n;i++)items[i]=encode(module,value->items[i],depth+1,nodes,symbols);
    uint32_t list=0;for(size_t i=value->n;i>0;i--)list=w2c_bel_join(module,items[i-1],list);return list;
  }
  if(value->type==VT_OBJECT&&value->n==1){
    V *character=v_get(value,"char");
    if(character&&character->type==VT_STRING){size_t position=0;uint32_t cp=v_scalar(character->s,character->n,&position);if(position!=character->n)v_fail("character must contain one scalar");return (cp<<3)|2;}
    V *pair=v_get(value,"pair");if(pair&&pair->type==VT_ARRAY&&pair->n==2){uint32_t head=encode(module,pair->items[0],depth+1,nodes,symbols);uint32_t tail=encode(module,pair->items[1],depth+1,nodes,symbols);return w2c_bel_join(module,head,tail);}
  }
  v_fail("unsupported core transport value");
}
static int pairp(w2c_bel *module,uint32_t value){return value>=4096&&!(value&7)&&(uint64_t)value+8<=w2c_bel_memory(module)->size;}
static uint32_t part(w2c_bel *module,uint32_t value,unsigned offset){if(!pairp(module,value))v_fail("invalid core pair");const uint8_t *p=w2c_bel_memory(module)->data+value+offset;return (uint32_t)p[0]|(uint32_t)p[1]<<8|(uint32_t)p[2]<<16|(uint32_t)p[3]<<24;}
static V *decode(w2c_bel *module,uint32_t value,unsigned depth,size_t *nodes,Symbols *symbols){
  count(nodes,depth);if(!value)return NULL;if(value==1)return v_string("t");
  if((value&7)==2)return v_character(value>>3);
  if((value&7)==1){
    for(size_t i=0;i<symbols->n;i++)if(symbols->ids[i]==value)return symbols->values[i];
    uint32_t list=w2c_bel_symbolname(module,value), scalars[4096];size_t length=0;
    while(list){if(length>=4096)v_fail("symbol name bound");count(nodes,depth+1);uint32_t c=part(module,list,0);if((c&7)!=2)v_fail("invalid core symbol character");scalars[length++]=c>>3;list=part(module,list,4);}
    /* Symbols are UTF-8 strings, not text-character values. Building temporary
     * V objects for each scalar inflated every retained identity on each call. */
    char *text=v_alloc(length*4+1);size_t n=0;
    for(size_t i=0;i<length;i++) {
      uint32_t cp=scalars[i];
      if(cp>0x10ffff||(cp>=0xd800&&cp<=0xdfff))v_fail("invalid Unicode scalar");
      if(cp<0x80)text[n++]=(char)cp;
      else if(cp<0x800){text[n++]=(char)(0xc0|(cp>>6));text[n++]=(char)(0x80|(cp&63));}
      else if(cp<0x10000){text[n++]=(char)(0xe0|(cp>>12));text[n++]=(char)(0x80|((cp>>6)&63));text[n++]=(char)(0x80|(cp&63));}
      else {text[n++]=(char)(0xf0|(cp>>18));text[n++]=(char)(0x80|((cp>>12)&63));text[n++]=(char)(0x80|((cp>>6)&63));text[n++]=(char)(0x80|(cp&63));}
    }
    V *result=v_string_n(text,n);
    if(symbols->n<SYMBOL_CACHE_BOUND){symbols->ids[symbols->n]=value;symbols->values[symbols->n++]=result;}
    return result;
  }
  if(pairp(module,value)){
    V *heads=v_array();uint32_t tail=value;
    while(tail&&pairp(module,tail)){count(nodes,depth);v_push(heads,decode(module,part(module,tail,0),depth+1,nodes,symbols));tail=part(module,tail,4);}
    if(!tail)return heads;
    V *result=decode(module,tail,depth+1,nodes,symbols);
    for(size_t i=heads->n;i>0;i--){V *pair=v_array();v_push(pair,heads->items[i-1]);v_push(pair,result);V *object=v_object();v_put(object,"pair",pair);result=object;}return result;
  }
  v_fail("invalid core value");
}

int main(void) {
  char input[262145]; size_t size=fread(input,1,sizeof(input),stdin);
  if(ferror(stdin)||size>262144){fputs("rbel value: input bound or read failure\n",stderr);return 1;}
  jmp_buf failure;v_handler=&failure;
  if(setjmp(failure)){fprintf(stderr,"rbel value: %s\n",v_error);return 1;}
  V *request=v_parse(input,size);
  wasm_rt_init();w2c_bel *module=calloc(1,sizeof(*module));if(!module)v_fail("allocation failed");
  wasm_rt_trap_t trap=wasm_rt_impl_try();
  if(trap!=WASM_RT_TRAP_NONE){fprintf(stderr,"rbel value: program trapped: %s\n",wasm_rt_strerror(trap));return 1;}
  wasm2c_bel_instantiate(module);Symbols symbols={0};size_t nodes=0;
  uint32_t encoded=encode(module,request,0,&nodes,&symbols);
  uint32_t result=w2c_bel_choose(module,encoded);nodes=0;
  V *answer=decode(module,result,0,&nodes,&symbols);char *json=v_json(answer);
  if(strlen(json)>262144)v_fail("output bound");
  int failed=fputs(json,stdout)==EOF||fputc('\n',stdout)==EOF||fflush(stdout);
  wasm2c_bel_free(module);free(module);wasm_rt_free();v_reset();return !!failed;
}
