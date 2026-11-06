/* Bounded UTF-8 JSON/value transport. No evaluation or protocol decisions. */
#include "native-values.h"
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define MAX_BYTES (128u * 1024u * 1024u)
#define MAX_JSON (32u * 1024u * 1024u)
#define MAX_NODES 262144u
#define MAX_DEPTH 128u

typedef struct Allocation { struct Allocation *next; } Allocation;
static Allocation *allocations;
static size_t allocated;
jmp_buf *v_handler;
char v_error[512];
_Noreturn void v_fail(const char *format, ...) {
  va_list arguments; va_start(arguments, format);
  vsnprintf(v_error, sizeof(v_error), format, arguments); va_end(arguments);
  if (v_handler) longjmp(*v_handler, 1);
  fprintf(stderr, "libcwr: %s\n", v_error); exit(1);
}
void *v_alloc(size_t size) {
  if (size > MAX_BYTES || allocated > MAX_BYTES - size) v_fail("value allocation bound");
  Allocation *allocation = calloc(1, sizeof(*allocation) + (size ? size : 1));
  if (!allocation) v_fail("allocation failed");
  allocation->next = allocations; allocations = allocation; allocated += size;
  return allocation + 1;
}
void v_reset(void) {
  while (allocations) { Allocation *next = allocations->next; free(allocations); allocations = next; }
  allocated = 0;
}
static V *make(VType type) { V *v = v_alloc(sizeof(*v)); v->type = type; return v; }
V *v_string_n(const char *text, size_t length) {
  if (length > MAX_JSON) v_fail("string byte bound");
  V *v = make(VT_STRING); v->s = v_alloc(length + 1); memcpy(v->s, text, length); v->n = length; return v;
}
V *v_string(const char *text) { return v_string_n(text, strlen(text)); }
V *v_number(long number) { char text[64]; snprintf(text, sizeof(text), "%ld", number); V *v=v_string(text); v->type=VT_NUMBER; return v; }
V *v_array(void) { return make(VT_ARRAY); }
V *v_object(void) { return make(VT_OBJECT); }
static void grow(V *v) {
  if (v->n >= MAX_NODES) v_fail("collection node bound");
  if (v->n < v->capacity) return;
  size_t cap = v->capacity ? v->capacity * 2 : 8;
  V **items = v_alloc(cap * sizeof(*items));
  if (v->items) memcpy(items,v->items,v->n*sizeof(*items)); v->items=items;
  if (v->type == VT_OBJECT) {
    char **keys=v_alloc(cap*sizeof(*keys));
    if (v->keys) memcpy(keys,v->keys,v->n*sizeof(*keys)); v->keys=keys;
  }
  v->capacity=cap;
}
void v_push(V *array, V *value) {
  if (!array || array->type!=VT_ARRAY) v_fail("expected array");
  grow(array); array->items[array->n++]=value;
}
void v_put(V *object, const char *key, V *value) {
  if (!object || object->type!=VT_OBJECT) v_fail("expected object");
  for(size_t i=0;i<object->n;i++) if(!strcmp(object->keys[i],key)) { object->items[i]=value; return; }
  grow(object); object->keys[object->n]=v_string(key)->s; object->items[object->n++]=value;
}
V *v_get(V *object, const char *key) {
  if (!object || object->type!=VT_OBJECT) return NULL;
  for(size_t i=0;i<object->n;i++) if(!strcmp(object->keys[i],key)) return object->items[i]; return NULL;
}
int v_isstr(V *value, const char *text) { return value && value->type==VT_STRING && value->n==strlen(text) && !memcmp(value->s,text,value->n); }
V *v_record_get(V *record, const char *key) {
  if (!record || record->type!=VT_ARRAY) return NULL;
  for(size_t i=0;i<record->n;i++) { V *entry=record->items[i]; if(entry && entry->type==VT_ARRAY && entry->n==2 && v_isstr(entry->items[0],key)) return entry->items[1]; }
  return NULL;
}
V *v_record_set(V *record, const char *key, V *value) {
  if(record && record->type!=VT_ARRAY) v_fail("expected association list");
  V *next=v_array();
  if(record) for(size_t i=0;i<record->n;i++) {
    V *entry=record->items[i];
    if(!entry || entry->type!=VT_ARRAY || entry->n!=2) v_fail("invalid association list");
    if(!v_isstr(entry->items[0],key)) v_push(next,entry);
  }
  V *entry=v_array();v_push(entry,v_string(key));v_push(entry,value);v_push(next,entry);return next;
}

typedef struct { char *data; size_t n, cap; } Buffer;
static void append(Buffer *buffer, const char *data, size_t n) {
  if(n>MAX_JSON || buffer->n>MAX_JSON-n) v_fail("serialized byte bound");
  if(buffer->n+n+1>buffer->cap) {
    size_t cap=buffer->cap?buffer->cap:64; while(cap<buffer->n+n+1) cap*=2;
    char *next=v_alloc(cap); if(buffer->data) memcpy(next,buffer->data,buffer->n);buffer->data=next;buffer->cap=cap;
  }
  memcpy(buffer->data+buffer->n,data,n);buffer->n+=n;buffer->data[buffer->n]=0;
}
static void byte(Buffer *buffer, char c) { append(buffer,&c,1); }
uint32_t v_scalar(const char *text, size_t length, size_t *position) {
  if(*position>=length) v_fail("truncated UTF-8");
  unsigned char a=(unsigned char)text[(*position)++];
  if(a<128) return a;
  unsigned count;uint32_t cp,min;
  if(a>=0xc2 && a<=0xdf) {count=1;cp=a&31;min=0x80;}
  else if(a>=0xe0 && a<=0xef) {count=2;cp=a&15;min=0x800;}
  else if(a>=0xf0 && a<=0xf4) {count=3;cp=a&7;min=0x10000;}
  else v_fail("invalid UTF-8");
  for(unsigned i=0;i<count;i++) {if(*position>=length) v_fail("truncated UTF-8");unsigned char b=(unsigned char)text[(*position)++];if((b&0xc0)!=0x80)v_fail("invalid UTF-8");cp=(cp<<6)|(b&63);}
  if(cp<min || cp>0x10ffff || (cp>=0xd800&&cp<=0xdfff))v_fail("invalid Unicode scalar");return cp;
}
static void utf8(Buffer *buffer, uint32_t cp) {
  char out[4];size_t n;
  if(cp>0x10ffff || (cp>=0xd800&&cp<=0xdfff))v_fail("invalid Unicode scalar");
  if(cp<128){out[0]=(char)cp;n=1;}else if(cp<0x800){out[0]=(char)(0xc0|(cp>>6));out[1]=(char)(0x80|(cp&63));n=2;}
  else if(cp<0x10000){out[0]=(char)(0xe0|(cp>>12));out[1]=(char)(0x80|((cp>>6)&63));out[2]=(char)(0x80|(cp&63));n=3;}
  else{out[0]=(char)(0xf0|(cp>>18));out[1]=(char)(0x80|((cp>>12)&63));out[2]=(char)(0x80|((cp>>6)&63));out[3]=(char)(0x80|(cp&63));n=4;}append(buffer,out,n);
}

typedef struct {const char *text;size_t length,pos,nodes;} Parser;
static void whitespace(Parser *p){while(p->pos<p->length && (p->text[p->pos]==' '||p->text[p->pos]=='\n'||p->text[p->pos]=='\r'||p->text[p->pos]=='\t'))p->pos++;}
static unsigned hex4(Parser *p) {
  unsigned result=0;for(unsigned i=0;i<4;i++){if(p->pos>=p->length)v_fail("truncated JSON escape");unsigned char c=(unsigned char)p->text[p->pos++];unsigned d;if(c>='0'&&c<='9')d=c-'0';else if(c>='a'&&c<='f')d=c-'a'+10;else if(c>='A'&&c<='F')d=c-'A'+10;else v_fail("invalid JSON escape");result=(result<<4)|d;}return result;
}
static V *parse_string(Parser *p) {
  if(p->pos>=p->length||p->text[p->pos++]!='"')v_fail("expected JSON string");Buffer b={0};
  while(p->pos<p->length){unsigned char c=(unsigned char)p->text[p->pos++];
    if(c=='"')return v_string_n(b.data?b.data:"",b.n);
    if(c<32)v_fail("control character in JSON string");
    if(c=='\\'){
      if(p->pos>=p->length)v_fail("truncated JSON escape");c=(unsigned char)p->text[p->pos++];
      switch(c){case '"':case '\\':case '/':byte(&b,(char)c);break;case 'b':byte(&b,'\b');break;case 'f':byte(&b,'\f');break;case 'n':byte(&b,'\n');break;case 'r':byte(&b,'\r');break;case 't':byte(&b,'\t');break;
        case 'u':{uint32_t cp=hex4(p);if(cp>=0xd800&&cp<=0xdbff){if(p->length-p->pos<2||p->text[p->pos]!='\\'||p->text[p->pos+1]!='u')v_fail("unpaired surrogate");p->pos+=2;uint32_t low=hex4(p);if(low<0xdc00||low>0xdfff)v_fail("unpaired surrogate");cp=0x10000+((cp-0xd800)<<10)+(low-0xdc00);}utf8(&b,cp);break;}
        default:v_fail("invalid JSON escape");}
    }else if(c<128)byte(&b,(char)c);else {p->pos--;size_t begin=p->pos;(void)v_scalar(p->text,p->length,&p->pos);append(&b,p->text+begin,p->pos-begin);}
  }v_fail("unclosed JSON string");
}
static V *parse_value(Parser *p,unsigned depth) {
  if(depth>MAX_DEPTH||++p->nodes>MAX_NODES)v_fail("JSON resource bound");whitespace(p);if(p->pos>=p->length)v_fail("missing JSON value");char c=p->text[p->pos];
  if(c=='"')return parse_string(p);
  if(c=='['||c=='{'){
    p->pos++;V *v=c=='['?v_array():v_object();char close=c=='['?']':'}';whitespace(p);if(p->pos<p->length&&p->text[p->pos]==close){p->pos++;return v;}
    for(;;){
      V *key=NULL;if(c=='{'){whitespace(p);key=parse_string(p);if(memchr(key->s,0,key->n))v_fail("NUL object key");for(size_t i=0;i<v->n;i++)if(!strcmp(v->keys[i],key->s))v_fail("duplicate JSON key");whitespace(p);if(p->pos>=p->length||p->text[p->pos++]!=':')v_fail("missing JSON colon");}
      V *item=parse_value(p,depth+1);if(c=='[')v_push(v,item);else v_put(v,key->s,item);whitespace(p);if(p->pos>=p->length)v_fail("unclosed JSON collection");char next=p->text[p->pos++];if(next==close)return v;if(next!=',')v_fail("missing JSON comma");
    }
  }
  for(unsigned i=0;i<3;i++){const char *word=i==0?"null":i==1?"true":"false";size_t n=strlen(word);if(p->length-p->pos>=n&&!memcmp(p->text+p->pos,word,n)){p->pos+=n;if(!i)return NULL;V *v=make(VT_BOOL);v->n=i==1;return v;}}
  size_t begin=p->pos;if(c=='-')p->pos++;
  if(p->pos>=p->length)v_fail("invalid JSON number");
  if(p->text[p->pos]=='0')p->pos++;else{if(p->text[p->pos]<'1'||p->text[p->pos]>'9')v_fail("invalid JSON value");while(p->pos<p->length&&p->text[p->pos]>='0'&&p->text[p->pos]<='9')p->pos++;}
  if(p->pos<p->length&&p->text[p->pos]=='.'){p->pos++;size_t digits=p->pos;while(p->pos<p->length&&p->text[p->pos]>='0'&&p->text[p->pos]<='9')p->pos++;if(p->pos==digits)v_fail("invalid JSON fraction");}
  if(p->pos<p->length&&(p->text[p->pos]=='e'||p->text[p->pos]=='E')){p->pos++;if(p->pos<p->length&&(p->text[p->pos]=='+'||p->text[p->pos]=='-'))p->pos++;size_t digits=p->pos;while(p->pos<p->length&&p->text[p->pos]>='0'&&p->text[p->pos]<='9')p->pos++;if(p->pos==digits)v_fail("invalid JSON exponent");}
  V *v=v_string_n(p->text+begin,p->pos-begin);v->type=VT_NUMBER;return v;
}
V *v_parse(const char *text,size_t length){if(length>MAX_JSON)v_fail("JSON byte bound");Parser p={text,length,0,0};V *v=parse_value(&p,0);whitespace(&p);if(p.pos!=length)v_fail("trailing JSON input");return v;}
static void quote(Buffer *b,const char *s,size_t length){byte(b,'"');for(size_t i=0;i<length;){size_t begin=i;uint32_t cp=v_scalar(s,length,&i);if(cp=='"'||cp=='\\'){byte(b,'\\');byte(b,(char)cp);}else if(cp<32){char escape[7];snprintf(escape,sizeof(escape),"\\u%04x",cp);append(b,escape,6);}else append(b,s+begin,i-begin);}byte(b,'"');}
static void render(Buffer *b,V *v,unsigned depth,size_t *nodes){if(depth>MAX_DEPTH||++*nodes>MAX_NODES)v_fail("JSON output resource bound");if(!v){append(b,"null",4);return;}switch(v->type){case VT_STRING:quote(b,v->s,v->n);break;case VT_NUMBER:append(b,v->s,v->n);break;case VT_BOOL:append(b,v->n?"true":"false",v->n?4:5);break;case VT_ARRAY:case VT_OBJECT:byte(b,v->type==VT_ARRAY?'[':'{');for(size_t i=0;i<v->n;i++){if(i)byte(b,',');if(v->type==VT_OBJECT){quote(b,v->keys[i],strlen(v->keys[i]));byte(b,':');}render(b,v->items[i],depth+1,nodes);}byte(b,v->type==VT_ARRAY?']':'}');break;}}
char *v_json(V *value){Buffer b={0};size_t nodes=0;render(&b,value,0,&nodes);return b.data;}
V *v_chars_n(const char *text,size_t length){V *v=v_array();for(size_t i=0;i<length;){size_t begin=i;(void)v_scalar(text,length,&i);V *c=v_object();v_put(c,"char",v_string_n(text+begin,i-begin));v_push(v,c);}return v;}
V *v_chars(const char *text){return v_chars_n(text,strlen(text));}
char *v_text_n(V *characters,size_t *length){if(characters&&characters->type!=VT_ARRAY)v_fail("action input must be text characters");Buffer b={0};if(characters)for(size_t i=0;i<characters->n;i++){V *c=characters->items[i];V *s=v_get(c,"char");if(!c||c->type!=VT_OBJECT||c->n!=1||!s||s->type!=VT_STRING)v_fail("action input must be text characters");size_t position=0;(void)v_scalar(s->s,s->n,&position);if(position!=s->n)v_fail("text character must be one scalar");append(&b,s->s,s->n);}if(!b.data)append(&b,"",0);if(length)*length=b.n;return b.data;}
char *v_text(V *characters){return v_text_n(characters,NULL);}

V *v_character(uint32_t cp){Buffer b={0};utf8(&b,cp);V *v=v_object();v_put(v,"char",v_string_n(b.data,b.n));return v;}
