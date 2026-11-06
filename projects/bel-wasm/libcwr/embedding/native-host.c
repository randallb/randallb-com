/* Local storage/action mechanisms for libcwr. All protocol decisions execute in
 * the generated rbel core; this host supplies trusted context and effects. */
#define _DARWIN_C_SOURCE 1
#define _POSIX_C_SOURCE 200809L
#include "native-host.h"
#include "runtime-license.h"
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#include "wasm-rt.h"

#define MAX_LINE (2u * 1024u * 1024u)
#define MAX_FILE (32u * 1024u * 1024u)
#define MAX_TRACE 262144u
static int directory_fd = -1;
static volatile sig_atomic_t lock_owned;
static int fixtures;
static int runtime_ready;
static unsigned restarts;
static char *environment_json;
static V *environment, *state;
static char fault[32];
static char **trace;
static size_t trace_count, trace_bytes;

/* SHA-256 is used for deterministic filenames and action-result digests. */
static uint32_t rotr(uint32_t x, unsigned n) { return (x >> n) | (x << (32-n)); }
static void sha256_block(uint32_t h[8], const unsigned char block[64]) {
  static const uint32_t k[64] = {
    0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,
    0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,
    0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,
    0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,
    0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,
    0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,
    0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,
    0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2
  };
  uint32_t w[64];
  for (unsigned i=0;i<16;i++) w[i]=(uint32_t)block[i*4]<<24|(uint32_t)block[i*4+1]<<16|(uint32_t)block[i*4+2]<<8|block[i*4+3];
  for (unsigned i=16;i<64;i++) {
    uint32_t a=w[i-15],b=w[i-2];
    w[i]=w[i-16]+(rotr(a,7)^rotr(a,18)^(a>>3))+w[i-7]+(rotr(b,17)^rotr(b,19)^(b>>10));
  }
  uint32_t a=h[0],b=h[1],c=h[2],d=h[3],e=h[4],f=h[5],g=h[6],z=h[7];
  for(unsigned i=0;i<64;i++) {
    uint32_t t=z+(rotr(e,6)^rotr(e,11)^rotr(e,25))+((e&f)^(~e&g))+k[i]+w[i];
    uint32_t u=(rotr(a,2)^rotr(a,13)^rotr(a,22))+((a&b)^(a&c)^(b&c));
    z=g;g=f;f=e;e=d+t;d=c;c=b;b=a;a=t+u;
  }
  h[0]+=a;h[1]+=b;h[2]+=c;h[3]+=d;h[4]+=e;h[5]+=f;h[6]+=g;h[7]+=z;
}
static void hash_bytes(const char *data,size_t n,char out[65]) {
  uint32_t h[8]={0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19};
  const unsigned char *p=(const unsigned char*)data;
  size_t length=n;
  while(n>=64){sha256_block(h,p);p+=64;n-=64;}
  unsigned char tail[128]={0};memcpy(tail,p,n);tail[n]=0x80;
  size_t blocks=n<56?64:128;uint64_t bits=(uint64_t)length*8;
  for(unsigned i=0;i<8;i++)tail[blocks-1-i]=(unsigned char)(bits>>(8*i));
  sha256_block(h,tail);if(blocks==128)sha256_block(h,tail+64);
  for(unsigned i=0;i<8;i++) {
    snprintf(out+i*8,9,"%08x",h[i]);
  }
  out[64]=0;
}
static V *at(V *v,size_t n) { return v&&v->type==VT_ARRAY&&n<v->n?v->items[n]:NULL; }
static V *pair(const char *key,V *value) { V *v=v_array();v_push(v,v_string(key));v_push(v,value);return v; }
static void field(V *record,const char *key,V *value) { v_push(record,pair(key,value)); }
static int symbol(V *v) { return v&&v->type==VT_STRING&&v->n>0; }
static int symbols(V *v) { if(!v)return 1;if(v->type!=VT_ARRAY)return 0;for(size_t i=0;i<v->n;i++)if(!symbol(v->items[i]))return 0;return 1; }
static int target_value(V *v) { return v&&v->type==VT_ARRAY&&v->n==6&&symbols(v); }
static int equal_string(V *a,V *b) { return a&&b&&a->type==VT_STRING&&b->type==VT_STRING&&a->n==b->n&&!memcmp(a->s,b->s,a->n); }
static void record_shape(V *v,const char *name,const char **required,size_t n) {
  if(!v||v->type!=VT_ARRAY)v_fail("%s must be an association list",name);
  for(size_t i=0;i<v->n;i++) {
    V *entry=v->items[i];if(!entry||entry->type!=VT_ARRAY||entry->n!=2||!at(entry,0)||at(entry,0)->type!=VT_STRING)v_fail("%s requires unique string keys and two-field entries",name);
    for(size_t j=0;j<i;j++)if(equal_string(at(entry,0),at(v->items[j],0)))v_fail("%s requires unique string keys and two-field entries",name);
  }
  for(size_t i=0;i<n;i++) {
    int found=0;for(size_t j=0;j<v->n;j++)if(v_isstr(at(v->items[j],0),required[i]))found=1;
    if(!found)v_fail("%s is missing a required field",name);
  }
}
static void validate_environment(V *env) {
  const char *required[]={"runtime","caller","ingress","context","available","allow","binding"};
  record_shape(env,"trusted environment",required,7);
  for(size_t i=0;i<4;i++)if(!symbol(v_record_get(env,required[i])))v_fail("trusted identity fields must contain symbols");
  if(!symbols(v_record_get(env,"available")))v_fail("trusted identity fields must contain symbols");
  V *allow=v_record_get(env,"allow");
  if(allow) {if(allow->type!=VT_ARRAY)v_fail("allow must contain six-field targets");for(size_t i=0;i<allow->n;i++)if(!target_value(allow->items[i]))v_fail("allow must contain six-field targets");}
  V *binding=v_record_get(env,"binding");
  const char *keys[]={"target","required","grants","capabilities","route","repeat-safe"};record_shape(binding,"binding",keys,6);
  if(!target_value(v_record_get(binding,"target"))||!symbol(v_record_get(binding,"route")))v_fail("invalid trusted binding structure");
  for(size_t i=1;i<4;i++)if(!symbols(v_record_get(binding,keys[i])))v_fail("invalid trusted binding structure");
  V *safe=v_record_get(binding,"repeat-safe");if(safe&&!v_isstr(safe,"t"))v_fail("invalid trusted binding structure");
}
static char *retain(const char *s) { char *p=strdup(s);if(!p)v_fail("allocation failed");return p; }
static void replace_json(char **saved,V *v) { char *next=retain(v_json(v));free(*saved);*saved=next; }
static void unlock(void) { if(lock_owned){lock_owned=0;unlinkat(directory_fd,".libcwr-lock",0);} }
static void signal_stop(int number) { unlock();_exit(number==SIGINT?130:143); }
static void write_all(int fd,const char *data,size_t n) {
  while(n) {ssize_t written=write(fd,data,n);if(written<0&&errno==EINTR)continue;if(written<=0){int error=errno;close(fd);v_fail("write failed: %s",strerror(error));}data+=written;n-=(size_t)written;}
}
static void make_directory(const char *path) {
  char *copy=retain(path);size_t n=strlen(copy);
  if(!n){free(copy);v_fail("empty state directory");}
  for(size_t i=1;i<=n;i++)if(copy[i]=='/'||copy[i]==0){char c=copy[i];copy[i]=0;if(mkdir(copy,0700)<0&&errno!=EEXIST){int e=errno;free(copy);v_fail("cannot create state directory: %s",strerror(e));}copy[i]=c;}
  free(copy);
}
static void lock_directory(const char *path) {
  make_directory(path);directory_fd=open(path,O_RDONLY|O_DIRECTORY|O_NOFOLLOW);
  if(directory_fd<0)v_fail("cannot open state directory: %s",strerror(errno));
  int fd=openat(directory_fd,".libcwr-lock",O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW,0600);
  if(fd<0) {if(errno==EEXIST)v_fail("state directory is locked; after confirming its owner is stopped, remove .libcwr-lock before recovery");v_fail("cannot lock state directory: %s",strerror(errno));}
  lock_owned=1;char pid[64];int n=snprintf(pid,sizeof(pid),"%ld\n",(long)getpid());write_all(fd,pid,(size_t)n);if(close(fd)<0)v_fail("cannot close lock file");
}
static char *read_descriptor(int fd,size_t *size) {
  struct stat st;if(fstat(fd,&st)<0||!S_ISREG(st.st_mode)||st.st_size<0||(uint64_t)st.st_size>MAX_FILE){close(fd);v_fail("invalid or oversized local file");}
  size_t cap=(size_t)st.st_size;char *data=v_alloc(cap+1);size_t n=0;
  while(n<cap) {ssize_t got=read(fd,data+n,cap-n);if(got<0&&errno==EINTR)continue;if(got<0){int error=errno;close(fd);v_fail("read failed: %s",strerror(error));}if(!got)break;n+=(size_t)got;}
  char extra;ssize_t more;do{more=read(fd,&extra,1);}while(more<0&&errno==EINTR);
  int error=errno;if(close(fd)<0)v_fail("cannot close local file");if(more<0)v_fail("read failed: %s",strerror(error));if(more>0)v_fail("local file changed while reading");data[n]=0;*size=n;return data;
}
static char *read_file(const char *name,size_t *size,int optional) {
  int fd=openat(directory_fd,name,O_RDONLY|O_NOFOLLOW);if(fd<0){if(optional&&errno==ENOENT){*size=0;return NULL;}v_fail("cannot read local file: %s",strerror(errno));}return read_descriptor(fd,size);
}
static V *read_value(const char *name) { size_t n;char *data=read_file(name,&n,1);return data?v_parse(data,n):NULL; }
static void random_bytes(unsigned char *bytes,size_t n) {
  int fd=open("/dev/urandom",O_RDONLY);if(fd<0)v_fail("cannot obtain claim entropy");
  size_t offset=0;while(offset<n){ssize_t got=read(fd,bytes+offset,n-offset);if(got<0&&errno==EINTR)continue;if(got<=0){close(fd);v_fail("cannot obtain claim entropy");}offset+=(size_t)got;}if(close(fd)<0)v_fail("cannot close entropy source");
}
static V *claim(void) { unsigned char bytes[16];random_bytes(bytes,sizeof(bytes));bytes[6]=(bytes[6]&15)|64;bytes[8]=(bytes[8]&63)|128;char text[96];snprintf(text,sizeof(text),"cwp://org.example.libcwr/claims/%02x%02x%02x%02x-%02x%02x-%02x%02x-%02x%02x-%02x%02x%02x%02x%02x%02x",bytes[0],bytes[1],bytes[2],bytes[3],bytes[4],bytes[5],bytes[6],bytes[7],bytes[8],bytes[9],bytes[10],bytes[11],bytes[12],bytes[13],bytes[14],bytes[15]);return v_string(text); }
static void atomic_bytes(const char *name,const char *data,size_t n) {
  unsigned char nonce[8];random_bytes(nonce,sizeof(nonce));char temporary[192];int count=snprintf(temporary,sizeof(temporary),".%s.new-%ld-%02x%02x%02x%02x%02x%02x%02x%02x",name,(long)getpid(),nonce[0],nonce[1],nonce[2],nonce[3],nonce[4],nonce[5],nonce[6],nonce[7]);
  if(count<0||(size_t)count>=sizeof(temporary))v_fail("local filename bound");
  int fd=openat(directory_fd,temporary,O_WRONLY|O_CREAT|O_EXCL|O_NOFOLLOW,0600);if(fd<0)v_fail("cannot create temporary file: %s",strerror(errno));
  /* write_all closes on failure; an unsuccessful private temporary may remain. */
  write_all(fd,data,n);if(close(fd)<0){unlinkat(directory_fd,temporary,0);v_fail("cannot close temporary file");}
  if(renameat(directory_fd,temporary,directory_fd,name)<0){int e=errno;unlinkat(directory_fd,temporary,0);v_fail("cannot commit local file: %s",strerror(e));}
}
static void atomic_value(const char *name,V *value) { char *json=v_json(value);atomic_bytes(name,json,strlen(json)); }
static void append_bytes(const char *name,const char *data,size_t n) { int fd=openat(directory_fd,name,O_WRONLY|O_CREAT|O_APPEND|O_NOFOLLOW,0600);if(fd<0)v_fail("cannot append local file: %s",strerror(errno));write_all(fd,data,n);if(close(fd)<0)v_fail("cannot close appended file"); }
static void hashed_name(V *reference,const char *prefix,const char *suffix,char out[96]) { if(!reference||reference->type!=VT_STRING)v_fail("invocation reference must be a symbol");char digest[65];hash_bytes(reference->s,reference->n,digest);snprintf(out,96,"%s%s%s",prefix,digest,suffix); }
static V *receipt(V *reference) {char name[96];hashed_name(reference,"receipt-",".json",name);return read_value(name);}
static int consume_fault(const char *name) {if(strcmp(fault,name))return 0;fault[0]=0;return 1;}
static void add_trace(V *event,V *response,V *effect) {
  if(!fixtures)return;
  V *row=v_object();v_put(row,"event",v_record_get(event,"op"));v_put(row,"response",at(response,0));v_put(row,"effect",at(effect,0));char *text=v_json(row);size_t n=strlen(text);
  if(trace_count>=MAX_TRACE||trace_bytes+n>MAX_FILE)v_fail("fixture trace resource bound");
  char **next=realloc(trace,(trace_count+1)*sizeof(*trace));if(!next)v_fail("allocation failed");trace=next;trace[trace_count++]=retain(text);trace_bytes+=n;
}
static V *transition(V *event,V *trusted,V **effect) {
  V *value=core_transition(state,event,trusted);if(!value||value->type!=VT_ARRAY||value->n!=3)v_fail("bad transition result");
  if(consume_fault("fail-save"))v_fail("injected snapshot commit failure");
  atomic_value("state.json",at(value,0));state=at(value,0);V *response=at(value,1);*effect=at(value,2);add_trace(event,response,*effect);return response;
}
static V *complete(V *evidence,V *replacement) {
  V *event=v_array();field(event,"op",v_string("complete"));field(event,"ref",v_record_get(evidence,"ref"));field(event,"claim",v_record_get(evidence,"claim"));field(event,"result",replacement?replacement:v_record_get(evidence,"result"));V *effect=NULL;return transition(event,v_record_set(environment,"executor",v_record_get(evidence,"target")),&effect);
}
static V *execute(V *invocation) {
  V *reference=v_record_get(invocation,"ref"),*exact=v_record_get(invocation,"target"),*input=v_record_get(invocation,"input");size_t n;char *body=v_text_n(input,&n),name[96],digest[65];hash_bytes(body,n,digest);hashed_name(reference,"artifact-",".txt",name);V *action=at(exact,3);
  if(v_isstr(action,"store--text"))atomic_bytes(name,body,n);else if(v_isstr(action,"append--text"))append_bytes(name,body,n);else v_fail("unsupported fixture action");
  V *entry=v_object();v_put(entry,"reference",reference);v_put(entry,"action",action);v_put(entry,"sha256",v_string(digest));char *json=v_json(entry);append_bytes("executions.jsonl",json,strlen(json));append_bytes("executions.jsonl","\n",1);
  V *evidence=v_array();field(evidence,"ref",reference);field(evidence,"claim",v_record_get(invocation,"claim"));field(evidence,"target",exact);field(evidence,"input",input);field(evidence,"result",v_chars(digest));
  if(consume_fault("lose-receipt")) {
    return NULL;
  }
  hashed_name(reference,"receipt-",".json",name);
  atomic_value(name,evidence);
  return evidence;
}
static V *dispatch(V *event) {
  V *trusted=v_record_set(environment,"claim",claim());
  if(v_isstr(v_record_get(event,"op"),"reconcile")) {V *evidence=receipt(v_record_get(event,"ref"));if(evidence&&consume_fault("tamper-receipt"))evidence=v_record_set(evidence,"input",v_chars("different"));trusted=v_record_set(trusted,"evidence",evidence);}
  V *effect=NULL,*response=transition(event,trusted,&effect);if(!effect)return response;
  if(!v_isstr(at(effect,0),"execute")||effect->type!=VT_ARRAY||effect->n!=2)v_fail("unknown mechanism effect");
  if(consume_fault("skip-execution")) {
    return response;
  }
  V *evidence=execute(at(effect,1));
  if(!evidence||consume_fault("lose-completion")) {
    return response;
  }
  return complete(evidence,NULL);
}
static V *restart(void) { state=read_value("state.json");restarts++;V *event=v_array();field(event,"op",v_string("recover"));V *effect=NULL;return transition(event,v_record_set(environment,"system",v_string("t")),&effect); }
static V *observe(V *reference) {
  size_t n;char *data=read_file("executions.jsonl",&n,1);V *executions=v_array();if(data)for(size_t i=0;i<n;i++)if(data[i]=='\n')v_push(executions,v_string("t"));V *restart_values=v_array();for(unsigned i=0;i<restarts;i++)v_push(restart_values,v_string("t"));char name[96];hashed_name(reference,"artifact-",".txt",name);data=read_file(name,&n,1);V *artifact=data?v_chars_n(data,n):NULL;V *result=v_array();field(result,"executions",executions);field(result,"restarts",restart_values);field(result,"artifact",artifact);field(result,"receipt",receipt(reference));return result;
}
static V *fixture_request(V *operation,V *arguments) {
  if(v_isstr(operation,"dispatch"))return dispatch(arguments);
  if(v_isstr(operation,"restart"))return restart();
  if(v_isstr(operation,"configure")) {if(arguments&&arguments->type!=VT_ARRAY)v_fail("configuration changes must be a list");for(size_t i=0;arguments&&i<arguments->n;i++){V *entry=arguments->items[i],*key=at(entry,0);if(!entry||entry->type!=VT_ARRAY||entry->n!=2||!key||key->type!=VT_STRING||memchr(key->s,0,key->n))v_fail("invalid configuration entry");environment=v_record_set(environment,key->s,at(entry,1));}replace_json(&environment_json,environment);return NULL;}
  if(v_isstr(operation,"fault")) {const char *allowed[]={"fail-save","skip-execution","lose-completion","lose-receipt","tamper-receipt"};for(size_t i=0;i<5;i++)if(v_isstr(arguments,allowed[i])){strcpy(fault,allowed[i]);return NULL;}v_fail("unknown failure injection");}
  if(v_isstr(operation,"snapshot"))return read_value("state.json");
  if(v_isstr(operation,"redeliver")||v_isstr(operation,"redeliver-different")){V *evidence=receipt(arguments);if(!evidence)v_fail("no durable receipt");return complete(evidence,v_isstr(operation,"redeliver-different")?v_chars("attempted overwrite"):NULL);}
  if(v_isstr(operation,"observe"))return observe(arguments);
  if(v_isstr(operation,"try-dispatch")) {jmp_buf local; jmp_buf *outer=v_handler;v_handler=&local;if(setjmp(local)){v_handler=outer;V *result=v_array();v_push(result,v_string("host-error"));v_push(result,v_chars(v_error));return result;}V *answer=dispatch(arguments);v_handler=outer;V *result=v_array();v_push(result,v_string("ok"));v_push(result,answer);return result;}
  v_fail("unknown test host operation");
}
static V *fixture_evidence(void) {V *rows=v_array();for(size_t i=0;i<trace_count;i++)v_push(rows,v_parse(trace[i],strlen(trace[i])));V *evidence=v_object();v_put(evidence,"trace",rows);v_put(evidence,"restarts",v_number(restarts));return evidence;}
static const char fixture_json[]="[[\"runtime\",\"cwp://org.example.libcwr/runtime\"],[\"caller\",\"cwp://org.example.libcwr/alice\"],[\"ingress\",\"cwp://org.example.libcwr/ingress\"],[\"context\",\"cwp://org.example.libcwr/workspace\"],[\"available\",[\"cwp://org.example.libcwr/alice\",\"cwp://org.example.libcwr/worker\"]],[\"allow\",[[\"cwp://org.example.libcwr/runtime\",\"cwp://org.example.libcwr/worker\",\"cwp://org.example.libcwr/role\",\"store--text\",\"cwp://org.example.libcwr/binding\",\"cwp://org.example.libcwr/contract\"]]],[\"binding\",[[\"target\",[\"cwp://org.example.libcwr/runtime\",\"cwp://org.example.libcwr/worker\",\"cwp://org.example.libcwr/role\",\"store--text\",\"cwp://org.example.libcwr/binding\",\"cwp://org.example.libcwr/contract\"]],[\"required\",[\"workspace.write\"]],[\"grants\",[\"workspace.write\"]],[\"capabilities\",[\"workspace.write\"]],[\"route\",\"cwp://org.example.libcwr/route-one\"],[\"repeat-safe\",\"t\"]]]]";
static void prepare_environment(V *value) {
  validate_environment(value);
  V *filtered = v_array();
  for (size_t i = 0; i < value->n; i++) {
    V *entry = value->items[i];
    V *key = at(entry, 0);
    if (!v_isstr(key, "claim") && !v_isstr(key, "executor") &&
        !v_isstr(key, "evidence") && !v_isstr(key, "system")) {
      v_push(filtered, entry);
    }
  }
  environment = filtered;
  replace_json(&environment_json, environment);
}
static void install_session_signals(void) {
  static int installed;
  if (installed) return;
  installed = 1;
  atexit(unlock);
  signal(SIGINT, signal_stop);
  signal(SIGTERM, signal_stop);
  signal(SIGPIPE, SIG_IGN);
}
void host_open(const char *directory, const char *context) {
  if (directory_fd >= 0) {
    v_fail("a local session is already open");
  }
  if (!fixtures && !context) {
    int existing = open(directory, O_RDONLY | O_DIRECTORY | O_NOFOLLOW);
    if (existing < 0) {
      v_fail("workspace is not initialized; run libcwr init --directory DIR");
    }
    close(existing);
  }
  if (fixtures) {
    prepare_environment(v_parse(fixture_json, strlen(fixture_json)));
  } else if (context) {
    int fd = open(context, O_RDONLY | O_NOFOLLOW);
    if (fd < 0) {
      v_fail("cannot read trusted context: %s", strerror(errno));
    }
    size_t n;
    char *data = read_descriptor(fd, &n);
    prepare_environment(v_parse(data, n));
  }
  lock_directory(directory);
  install_session_signals();
  if (!fixtures && !context) {
    size_t n;
    char *data = read_file("context.json", &n, 0);
    prepare_environment(v_parse(data, n));
  }
  wasm_rt_init();
  runtime_ready = 1;
  state = read_value("state.json");
  if (!fixtures) {
    restart();
  }
}
V *host_event(V *event) { return dispatch(event); }
V *host_state(void) { return state; }
V *host_environment(void) { return environment; }
V *host_artifact(V *reference) {
  V *event = v_array();
  field(event, "op", v_string("read"));
  field(event, "kind", v_string("invocation"));
  field(event, "ref", reference);
  V *response = dispatch(event);
  if (!v_isstr(at(response, 0), "found")) {
    v_fail("invocation is unavailable");
  }
  char name[96];
  hashed_name(reference, "artifact-", ".txt", name);
  size_t n;
  char *data = read_file(name, &n, 1);
  if (!data) {
    v_fail("invocation has no local artifact yet; use run or inspect its status");
  }
  return v_string_n(data, n);
}
static V *localize(V *value, const char *base) {
  const char prefix[] = "cwp://org.example.libcwr/";
  if (value && value->type == VT_STRING && value->n >= sizeof(prefix) - 1 &&
      !memcmp(value->s, prefix, sizeof(prefix) - 1)) {
    const char *tail = value->s + sizeof(prefix) - 1;
    size_t n = strlen(base) + strlen(tail) + 2;
    char *text = v_alloc(n);
    snprintf(text, n, "%s/%s", base, tail);
    return v_string(text);
  }
  if (value && value->type == VT_ARRAY) {
    V *result = v_array();
    for (size_t i = 0; i < value->n; i++) {
      v_push(result, localize(value->items[i], base));
    }
    return result;
  }
  return value;
}
V *host_init(const char *directory) {
  lock_directory(directory);
  install_session_signals();
  int duplicate = dup(directory_fd);
  if (duplicate < 0) {
    v_fail("cannot inspect workspace directory");
  }
  DIR *entries = fdopendir(duplicate);
  if (!entries) {
    close(duplicate);
    v_fail("cannot inspect workspace directory");
  }
  struct dirent *entry;
  int nonempty = 0;
  errno = 0;
  while ((entry = readdir(entries))) {
    if (strcmp(entry->d_name, ".") && strcmp(entry->d_name, "..") &&
        strcmp(entry->d_name, ".libcwr-lock")) {
      nonempty = 1;
      break;
    }
  }
  int scan_error = errno;
  closedir(entries);
  if (scan_error) {
    v_fail("cannot inspect workspace directory: %s", strerror(scan_error));
  }
  if (nonempty) {
    v_fail("init requires a new or empty directory; existing files were preserved");
  }
  V *fresh = claim();
  const char *uuid = strrchr(fresh->s, '/');
  char base[128];
  snprintf(base, sizeof(base), "cwp://org.example.libcwr/local/%s", uuid + 1);
  prepare_environment(localize(v_parse(fixture_json, strlen(fixture_json)), base));
  atomic_value("context.json", environment);
  V *info = v_object();
  v_put(info, "directory", v_string(directory));
  v_put(info, "context", v_record_get(environment, "context"));
  v_put(info, "caller", v_record_get(environment, "caller"));
  v_put(info, "action", v_string("store--text"));
  return info;
}
void host_close(void) {
  unlock();
  if (directory_fd >= 0) {
    close(directory_fd);
    directory_fd = -1;
  }
  free(environment_json);
  environment_json = NULL;
  for (size_t i = 0; i < trace_count; i++) {
    free(trace[i]);
  }
  free(trace);
  trace = NULL;
  trace_count = trace_bytes = 0;
  state = environment = NULL;
  v_reset();
  if (runtime_ready) {
    wasm_rt_free();
    runtime_ready = 0;
  }
}
static void request(const char *data,size_t length) {
  environment=v_parse(environment_json,strlen(environment_json));state=read_value("state.json");V *value=v_parse(data,length),*response;
  if(fixtures) {if(!value||value->type!=VT_OBJECT||!v_get(value,"operation")||v_get(value,"operation")->type!=VT_STRING)v_fail("fixture request must contain operation and optional arguments");for(size_t i=0;i<value->n;i++)if(strcmp(value->keys[i],"operation")&&strcmp(value->keys[i],"arguments"))v_fail("fixture request must contain operation and optional arguments");V *answer=fixture_request(v_get(value,"operation"),v_get(value,"arguments"));response=v_object();v_put(response,"value",answer);v_put(response,"evidence",fixture_evidence());}
  else response=dispatch(value);
  char *output=v_json(response);size_t n=strlen(output);if(n>MAX_FILE)v_fail("response exceeds line bound");if(fwrite(output,1,n,stdout)!=n||fputc('\n',stdout)==EOF||fflush(stdout)!=0)v_fail("response write failed");
}
int main(int argc,char **argv) {
  if (argc == 1 || argv[1][0] != '-' || !strcmp(argv[1], "--version") ||
      (argc == 2 && !strcmp(argv[1], "--help"))) {
    return cli_main(argc, argv);
  }
  jmp_buf top;
  v_handler=&top;
  volatile int status=0;
  if(setjmp(top)){fprintf(stderr,"libcwr: %s\n",v_error);status=1;goto done;}
  const char *directory=NULL,*context=NULL;int help=0,license=0;
  for(int i=1;i<argc;i++) {
    if(!strcmp(argv[i],"--test-host")){if(fixtures)v_fail("duplicate option --test-host");fixtures=1;}
    else if(!strcmp(argv[i],"--help")){if(help)v_fail("duplicate option --help");help=1;}
    else if(!strcmp(argv[i],"--runtime-license")){if(license)v_fail("duplicate option --runtime-license");license=1;}
    else if(!strcmp(argv[i],"--directory")||!strcmp(argv[i],"--context")){const char **destination=!strcmp(argv[i],"--directory")?&directory:&context;if(*destination||i+1>=argc||!strncmp(argv[i+1],"--",2))v_fail("invalid option %s",argv[i]);*destination=argv[++i];}
    else v_fail("unknown option %s",argv[i]);
  }
  if(license){if(fputs(NATIVE_RUNTIME_LICENSE,stdout)==EOF||fflush(stdout))v_fail("license write failed");goto done;}
  if(help){puts("Usage: libcwr --directory STATE --context TRUSTED.json\nReads one event as JSON per line; writes one protocol response per line.\n--test-host enables isolated acceptance fixture controls.\nCompiled rbel core via WABT wasm2c; no Node or separate Wasm engine required.");goto done;}
  if(!directory) {
    v_fail("--directory is required");
  }
  if(fixtures&&context) {
    v_fail("--test-host cannot be combined with --context");
  }
  if(!fixtures&&!context) {
    v_fail("--context is required");
  }
  host_open(directory, context);
  v_reset();
  char *line=malloc(MAX_LINE+1);if(!line)v_fail("allocation failed");size_t n=0;int c;
  while((c=fgetc(stdin))!=EOF){if(c=='\n'){request(line,n);v_reset();n=0;}else{if(n==MAX_LINE){free(line);v_fail("request exceeds line bound");}line[n++]=(char)c;}}
  if(ferror(stdin)){free(line);v_fail("request read failed");}if(n)request(line,n);free(line);
 done:
  host_close();
  return status;
}
