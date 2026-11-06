/* C value adapter for the unchanged compiled rbel transition function. */
#include "native-values.h"
#include <stdlib.h>
#include <string.h>
#include "module.h"
#include "wasm-rt-impl.h"
#include "wasm-rt-exceptions.h"

#define NODE_BOUND 262144u
#define DEPTH_BOUND 128u
static void count(size_t *nodes,unsigned depth){if(++*nodes>NODE_BOUND||depth>DEPTH_BOUND)v_fail("core value resource bound");}
static uint32_t encode(w2c_bel *module,V *value,unsigned depth,size_t *nodes){
  count(nodes,depth);if(!value)return 0;
  if(value->type==VT_STRING){
    uint32_t *characters=v_alloc((value->n+1)*sizeof(*characters));size_t n=0;
    for(size_t position=0;position<value->n;){if(n>=4096)v_fail("symbol name bound");characters[n++]=v_scalar(value->s,value->n,&position);count(nodes,depth+1);}
    uint32_t name=0;for(size_t i=n;i>0;i--)name=w2c_bel_join(module,(characters[i-1]<<3)|2,name);
    return w2c_bel_intern(module,name);
  }
  if(value->type==VT_ARRAY){
    uint32_t *items=v_alloc((value->n+1)*sizeof(*items));
    for(size_t i=0;i<value->n;i++)items[i]=encode(module,value->items[i],depth+1,nodes);
    uint32_t list=0;for(size_t i=value->n;i>0;i--)list=w2c_bel_join(module,items[i-1],list);return list;
  }
  if(value->type==VT_OBJECT&&value->n==1){
    V *character=v_get(value,"char");
    if(character&&character->type==VT_STRING){size_t position=0;uint32_t cp=v_scalar(character->s,character->n,&position);if(position!=character->n)v_fail("character must contain one scalar");return (cp<<3)|2;}
    V *pair=v_get(value,"pair");if(pair&&pair->type==VT_ARRAY&&pair->n==2){uint32_t head=encode(module,pair->items[0],depth+1,nodes);uint32_t tail=encode(module,pair->items[1],depth+1,nodes);return w2c_bel_join(module,head,tail);}
  }
  v_fail("unsupported core transport value");
}
static int pairp(w2c_bel *module,uint32_t value){return value>=4096&&!(value&7)&&(uint64_t)value+8<=w2c_bel_memory(module)->size;}
static uint32_t part(w2c_bel *module,uint32_t value,unsigned offset){if(!pairp(module,value))v_fail("invalid core pair");const uint8_t *p=w2c_bel_memory(module)->data+value+offset;return (uint32_t)p[0]|(uint32_t)p[1]<<8|(uint32_t)p[2]<<16|(uint32_t)p[3]<<24;}
static V *decode(w2c_bel *module,uint32_t value,unsigned depth,size_t *nodes){
  count(nodes,depth);if(!value)return NULL;if(value==1)return v_string("t");
  if((value&7)==2)return v_character(value>>3);
  if((value&7)==1){
    uint32_t list=w2c_bel_symbolname(module,value);V *characters=v_array();
    while(list){if(characters->n>=4096)v_fail("symbol name bound");count(nodes,depth+1);uint32_t c=part(module,list,0);if((c&7)!=2)v_fail("invalid core symbol character");v_push(characters,v_character(c>>3));list=part(module,list,4);}
    size_t length;char *text=v_text_n(characters,&length);return v_string_n(text,length);
  }
  if(pairp(module,value)){
    V *heads=v_array();uint32_t tail=value;
    while(tail&&pairp(module,tail)){count(nodes,depth);v_push(heads,decode(module,part(module,tail,0),depth+1,nodes));tail=part(module,tail,4);}
    if(!tail)return heads;
    V *result=decode(module,tail,depth+1,nodes);
    for(size_t i=heads->n;i>0;i--){V *pair=v_array();v_push(pair,heads->items[i-1]);v_push(pair,result);V *object=v_object();v_put(object,"pair",pair);result=object;}return result;
  }
  v_fail("invalid core value");
}
V *core_transition(V *state,V *event,V *environment){
  w2c_bel *module=calloc(1,sizeof(*module));if(!module)v_fail("core allocation failed");
  jmp_buf local;jmp_buf *previous=v_handler;v_handler=&local;
  if(setjmp(local)){
    char message[sizeof(v_error)];memcpy(message,v_error,sizeof(message));
    wasm2c_bel_free(module);free(module);v_handler=previous;v_fail("%s",message);
  }
  wasm_rt_trap_t trap=wasm_rt_impl_try();
  if(trap!=WASM_RT_TRAP_NONE){wasm2c_bel_free(module);free(module);v_handler=previous;v_fail("core trapped: %s",wasm_rt_strerror(trap));}
  wasm2c_bel_instantiate(module);
  V *arguments=v_array();v_push(arguments,state);v_push(arguments,event);v_push(arguments,environment);
  V *request=v_array();v_push(request,v_string("call"));v_push(request,arguments);
  size_t nodes=0;uint32_t encoded=encode(module,request,0,&nodes);uint32_t result=w2c_bel_choose(module,encoded);
  nodes=0;V *decoded=decode(module,result,0,&nodes);
  wasm2c_bel_free(module);free(module);v_handler=previous;
  if(!decoded||decoded->type!=VT_ARRAY||decoded->n!=3)v_fail("invalid libcwr transition result");return decoded;
}
