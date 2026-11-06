/* Workspace terminal transport. Membership, activation and action outcomes are
 * decisions of the compiled workspace coworker, not this parser or scheduler. */
#define _DARWIN_C_SOURCE 1
#define _POSIX_C_SOURCE 200809L
#include "native-workspace.h"
#include "native-host.h"
#include "native-reload.h"
#include <ctype.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define TEXT_LIMIT (256u * 1024u)
static volatile sig_atomic_t worker_pid;
static int worker_fd=-1, worker_cancelled;
static char *worker_record;
static unsigned char worker_bytes[TEXT_LIMIT+1024];
static size_t worker_size;
static char selection[65];
static unsigned long serial;
static int prompt_shown;
static void prompt(void);
static V *at(V *v,size_t i){return v&&v->type==VT_ARRAY&&i<v->n?v->items[i]:NULL;}
static void field(V *v,const char *key,V *value){V *p=v_array();v_push(p,v_string(key));v_push(p,value);v_push(v,p);}
static const char *symbol(V *v){return v&&v->type==VT_STRING?v->s:"";}
static const char *tail(V *v){const char *s=symbol(v),*last=strrchr(s,'/');return last?last+1:s;}
static V *record(const char *op){V *v=v_array();field(v,"op",v_string(op));return v;}
static void fresh(char out[96],const char *prefix){struct timespec now;clock_gettime(CLOCK_REALTIME,&now);snprintf(out,96,"%s-%ld-%ld-%ld-%lu",prefix,(long)getpid(),(long)now.tv_sec,now.tv_nsec,++serial);}
static void valid_name(const char *s){size_t n=strlen(s);if(!n||n>64)v_fail("names require 1-64 letters, digits, hyphens or underscores");for(size_t i=0;i<n;i++)if(!((s[i]>='a'&&s[i]<='z')||(s[i]>='A'&&s[i]<='Z')||(s[i]>='0'&&s[i]<='9')||s[i]=='-'||s[i]=='_'))v_fail("invalid name");}
/* Resolve display aliases against the actual directory identity. Imported and
 * person coworkers need not live under this workspace's /coworkers/ path. */
static V *directory_coworker(const char *argument,int *available_out){
  const char *name=*argument=='@'?argument+1:argument;
  V *available=v_record_get(host_environment(),"available"),*match=NULL;
  *available_out=0;
  for(size_t i=0;available&&i<available->n;i++)
    if(v_isstr(available->items[i],name)){*available_out=1;return available->items[i];}
  if(!strncmp(name,"cwp://",6))return v_string(name);
  valid_name(name);
  for(size_t i=0;available&&i<available->n;i++)if(!strcmp(tail(available->items[i]),name)){
    if(match&&!v_isstr(match,symbol(available->items[i])))v_fail("ambiguous coworker name; use its canonical ref from /coworkers");
    match=available->items[i];
  }
  if(match){*available_out=1;return match;}
  /* Unknown aliases still pass through protocol validation and report unavailable. */
  return host_workspace_ref("coworkers",name);
}
static void print_text(V *value){size_t n;char *s=v_text_n(value,&n);if(n)fwrite(s,1,n,stdout);}
static V *input_conversation(void){V *input=v_array();if(selection[0])field(input,"conversation",host_workspace_ref("conversations",selection));return input;}
static V *control(const char *action,V *input){
  char name[96];fresh(name,"control");V *event=record("workspace-call");
  field(event,"ref",host_workspace_ref("invocations",name));field(event,"origin",host_workspace_ref("requests",name));
  field(event,"target",host_workspace_binding(action));field(event,"input",input);
  V *answer=host_workspace_step(event),*response=at(answer,0),*effect=at(answer,1);
  if(v_isstr(at(effect,0),"reload")){
    V *invocation=at(effect,1);int failed=coworker_reload();
    V *done=record("workspace-complete");field(done,"ref",v_record_get(invocation,"ref"));field(done,"claim",v_record_get(invocation,"claim"));
    field(done,"result",v_chars(failed?"reload failed; last working coworker retained":"Reloaded coworker source."));
    if(failed)field(done,"failed",v_string("t"));
    answer=host_workspace_finish(done,v_record_get(invocation,"target"));response=at(answer,0);
    if(!failed)puts("Reloaded coworker source.");else fputs("libcwr: reload failed; last working coworker retained.\n",stderr);
  }
  return response;
}
static void describe(const char *action,V *response){
  printf("%s: %s\n",action,symbol(at(response,0)));
}
static void participants(V *conversation){
  V *people=v_record_get(conversation,"participants"),*caller=v_record_get(host_environment(),"caller");
  fputs("Participants: ",stdout);
  for(size_t i=0;people&&i<people->n;i++){if(i)fputs(", ",stdout);fputs(v_isstr(people->items[i],symbol(caller))?"you":tail(people->items[i]),stdout);}
  putchar('\n');
}
static void history(V *conversation){
  V *messages=v_record_get(conversation,"messages");
  if(!messages||!messages->n){puts("No messages yet.");return;}
  for(size_t i=messages->n;i>0;i--){V *m=messages->items[i-1];V *who=v_record_get(m,"author");
    V *caller=v_record_get(host_environment(),"caller");printf("%s: ",v_isstr(who,symbol(caller))?"you":tail(who));print_text(v_record_get(m,"text"));putchar('\n');}
}
static void outer_stop(int signal_number){coworker_interrupt();_exit(128+signal_number);}
void workspace_interrupt(void){pid_t pid=(pid_t)worker_pid;if(pid>0)kill(pid,SIGTERM);}
static void send_bytes(int fd,const void *data,size_t n){const char *p=data;while(n){ssize_t count=write(fd,p,n);if(count<0&&errno==EINTR)continue;if(count<=0)_exit(125);p+=count;n-=(size_t)count;}}
static V *builtin_transform(V *input){
  V *result=v_array();for(size_t offset=0;input&&offset<input->n;offset+=256){
    V *chunk=v_array();size_t end=offset+256<input->n?offset+256:input->n;for(size_t i=offset;i<end;i++)v_push(chunk,input->items[i]);
    V *op=record("uppercase-value");field(op,"input",chunk);V *answer=core_transition(NULL,op,NULL),*reply=at(answer,1);
    if(!v_isstr(at(reply,0),"transformed"))v_fail("compiled coworker rejected input");
    V *chars=at(reply,1);for(size_t i=0;chars&&i<chars->n;i++)v_push(result,chars->items[i]);
  }return result;
}
static void start_worker(V *invocation){
  int descriptors[2];if(pipe(descriptors))v_fail("cannot create coworker result pipe");
  fcntl(descriptors[0],F_SETFD,FD_CLOEXEC);fcntl(descriptors[1],F_SETFD,FD_CLOEXEC);
  char *saved=strdup(v_json(invocation));if(!saved){close(descriptors[0]);close(descriptors[1]);v_fail("allocation failed");}
  sigset_t block,previous;sigemptyset(&block);sigaddset(&block,SIGINT);sigaddset(&block,SIGTERM);sigprocmask(SIG_BLOCK,&block,&previous);
  pid_t pid=fork();
  if(!pid){
    worker_pid=0;close(descriptors[0]);
    struct sigaction action={0};action.sa_handler=outer_stop;sigemptyset(&action.sa_mask);sigaction(SIGTERM,&action,NULL);sigaction(SIGINT,&action,NULL);sigprocmask(SIG_SETMASK,&previous,NULL);
    jmp_buf handler;v_handler=&handler;
    if(setjmp(handler)){unsigned char bad=1;send_bytes(descriptors[1],&bad,1);send_bytes(descriptors[1],v_error,strlen(v_error));_exit(1);}
    V *input=v_record_get(invocation,"input");size_t n;char *body=v_text_n(input,&n),*answer;
    if(coworker_active())answer=coworker_transform(body,n,&n);else answer=v_text_n(builtin_transform(input),&n);
    v_chars_n(answer,n); /* Validate complete UTF-8 before publishing a result. */
    unsigned char good=0;send_bytes(descriptors[1],&good,1);send_bytes(descriptors[1],answer,n);close(descriptors[1]);_exit(0);
  }
  close(descriptors[1]);worker_pid=pid>0?(sig_atomic_t)pid:0;sigprocmask(SIG_SETMASK,&previous,NULL);
  if(pid<0){close(descriptors[0]);free(saved);v_fail("cannot start coworker worker");}
  fcntl(descriptors[0],F_SETFL,O_NONBLOCK);worker_fd=descriptors[0];worker_record=saved;worker_size=0;worker_cancelled=0;
  printf("Work %s: running\n",tail(v_record_get(invocation,"ref")));fflush(stdout);
}
static void settle_worker(int status){
  /* The child was reaped before entering protocol code. Clear all live-child
   * tracking first: parse/resource failures must not leave a stale PID to kill. */
  char *saved=worker_record;size_t size=worker_size;int cancelled=worker_cancelled;
  worker_record=NULL;worker_pid=0;worker_size=0;worker_cancelled=0;
  if(worker_fd>=0)close(worker_fd);worker_fd=-1;
  jmp_buf parsing;jmp_buf *previous=v_handler;v_handler=&parsing;
  if(setjmp(parsing)){char error[sizeof(v_error)];memcpy(error,v_error,sizeof(error));free(saved);v_handler=previous;v_fail("%s",error);}
  V *invocation=v_parse(saved,strlen(saved));free(saved);v_handler=previous;
  V *target=v_record_get(invocation,"target");
  int failed=size<1||worker_bytes[0]!=0||!WIFEXITED(status)||WEXITSTATUS(status);
  V *done=record(cancelled?"workspace-stopped":failed?"workspace-failed":"complete");
  field(done,"ref",v_record_get(invocation,"ref"));field(done,"claim",v_record_get(invocation,"claim"));
  if(!cancelled&&!failed)field(done,"result",v_chars_n((char*)worker_bytes+1,size-1));
  if(failed&&!cancelled){field(done,"uncertain",v_string("t"));field(done,"result",v_chars(size>1?(char*)worker_bytes+1:"worker failed"));}
  V *answer=host_workspace_finish(done,target),*response=at(answer,0);
  int redraw=prompt_shown;if(redraw)putchar('\n');
  if(!cancelled&&!failed&&v_isstr(at(response,0),"completed")){
    printf("uppercase [%s]: ",tail(v_record_get(invocation,"conversation")));print_text(v_record_get(at(response,1),"result"));putchar('\n');
  }else{printf("Work %s: %s\n",tail(v_record_get(invocation,"ref")),symbol(at(response,0)));if(failed&&size>1)fprintf(stderr,"libcwr: %.*s\n",(int)(size-1),worker_bytes+1);}
  if(redraw)prompt();else fflush(stdout);
}
static void worker_tick(void){
  if(worker_pid>0){
    V *saved=v_parse(worker_record,strlen(worker_record)),*ref=v_record_get(saved,"ref"),*all=v_record_get(host_state(),"invocations");
    for(size_t i=0;all&&i<all->n;i++)if(v_isstr(v_record_get(all->items[i],"ref"),symbol(ref))&&v_isstr(v_record_get(all->items[i],"phase"),"cancelling")&&!worker_cancelled){workspace_interrupt();worker_cancelled=1;}
    for(;;){ssize_t n=read(worker_fd,worker_bytes+worker_size,sizeof(worker_bytes)-worker_size-1);if(n>0){worker_size+=(size_t)n;if(worker_size==sizeof(worker_bytes)-1){workspace_interrupt();}continue;}if(n<0&&errno==EINTR)continue;break;}
    int status=0;pid_t done=waitpid((pid_t)worker_pid,&status,WNOHANG);if(done==(pid_t)worker_pid){
      worker_pid=0;
      for(;;){ssize_t n=read(worker_fd,worker_bytes+worker_size,sizeof(worker_bytes)-worker_size-1);if(n>0){worker_size+=(size_t)n;continue;}if(n<0&&errno==EINTR)continue;break;}
      worker_bytes[worker_size]=0;settle_worker(status);
    }else if(done<0&&errno!=EINTR){
      worker_pid=0;worker_bytes[0]=1;
      const char *message="lost coworker process status; inspect retained work";
      worker_size=strlen(message)+1;memcpy(worker_bytes+1,message,worker_size);
      settle_worker(1<<8);
    }
    return;
  }
  V *all=v_record_get(host_state(),"invocations");
  for(size_t i=0;all&&i<all->n;i++){V *inv=all->items[i];
    if(v_record_get(inv,"control")||!v_isstr(v_record_get(inv,"phase"),"admitted")||!v_isstr(at(v_record_get(inv,"target"),3),"uppercase--text"))continue;
    host_workspace_binding("uppercase");V *run=record("run");field(run,"ref",v_record_get(inv,"ref"));V *answer=host_workspace_step(run),*effect=at(answer,1);
    if(v_isstr(at(effect,0),"execute")){start_worker(at(effect,1));break;}
  }
}
static void prompt(void){prompt_shown=1;if(selection[0])printf("you[%s]> ",selection);else fputs("workspace> ",stdout);fflush(stdout);}
static void help(void){puts("Workspace: /new NAME, /open NAME, /home, /conversations, /coworkers, /actions\nConversation: /invite NAME, /remove NAME, /history, /status\nUnaddressed text is shared history and activates nobody; @uppercase TEXT requests work.\n/reload rebuilds configured language source; /exit leaves. Commands invoke coworker actions.");}
static void conversation_required(void){if(!selection[0])v_fail("open a conversation first with /new NAME or /open NAME");}
static int command(char *line){
  if(!line[0])return 0;
  if(line[0]!='/'){
    conversation_required();
    if(line[0]=='@'){
      char *space=strchr(line,' ');if(!space||space==line+1)v_fail("mention requires @coworker followed by text");
      *space=0;int available=0;V *recipient=directory_coworker(line+1,&available);
      V *worker_target=host_workspace_binding("uppercase");
      if(!v_isstr(recipient,symbol(at(worker_target,1)))){
        if(available)v_fail("coworker has no conversational route; use /actions");
        v_fail("coworker is unavailable in this workspace");
      }
      char request_name[96];fresh(request_name,"turn");V *request=record("admit");field(request,"ref",host_workspace_ref("invocations",request_name));
      field(request,"origin",host_workspace_ref("requests",request_name));field(request,"conversation",host_workspace_ref("conversations",selection));
      field(request,"message",host_workspace_ref("messages",request_name));field(request,"response-message",host_workspace_ref("replies",request_name));
      field(request,"target",worker_target);field(request,"input",v_chars(space+1));*space=' ';field(request,"text",v_chars(line));
      V *response=at(host_workspace_step(request),0);printf("Work %s: %s\n",request_name,symbol(at(response,0)));
    }else{
      char message[96];fresh(message,"message");V *event=record("workspace-publish");field(event,"conversation",host_workspace_ref("conversations",selection));
      field(event,"message",host_workspace_ref("messages",message));field(event,"text",v_chars(line));V *response=at(host_workspace_step(event),0);describe("message",response);
    }return 0;
  }
  char *verb=line+1,*argument=NULL;
  for(char *p=verb;*p;p++)if(isspace((unsigned char)*p)){*p++=0;while(isspace((unsigned char)*p))p++;argument=*p?p:NULL;break;}
  if(!strcmp(verb,"exit")||!strcmp(verb,"quit")){
    if(argument)v_fail("unexpected command argument");
    V *response=control("exit",input_conversation());describe("exit",response);
    return v_isstr(at(response,0),"exited");
  }
  if(!strcmp(verb,"help")){
    if(argument)v_fail("unexpected command argument");
    V *response=control("help",input_conversation());describe("help",response);
    if(v_isstr(at(response,0),"found"))help();return 0;
  }
  const char *action=verb;
  if(!strcmp(verb,"conversations"))action="list";else if(!strcmp(verb,"use"))action="open";else if(!strcmp(verb,"coworkers"))action="coworkers";
  V *input=(!strcmp(action,"new")||!strcmp(action,"open"))?v_array():input_conversation();
  if(!strcmp(action,"new")||!strcmp(action,"open")){
    if(!argument)v_fail("%s requires a conversation name",verb);valid_name(argument);field(input,"conversation",host_workspace_ref("conversations",argument));
  }else if(!strcmp(action,"invite")||!strcmp(action,"remove")){
    conversation_required();if(!argument)v_fail("%s requires a coworker name",verb);int available=0;field(input,"coworker",directory_coworker(argument,&available));
  }else if(!strcmp(action,"history")){conversation_required();}
  else if(strcmp(action,"home")&&strcmp(action,"list")&&strcmp(action,"actions")&&strcmp(action,"coworkers")&&strcmp(action,"status")&&strcmp(action,"reload"))v_fail("unknown command; use /help");
  if(argument&&strcmp(action,"new")&&strcmp(action,"open")&&strcmp(action,"invite")&&strcmp(action,"remove"))v_fail("unexpected command argument");
  V *response=control(action,input);const char *code=symbol(at(response,0));describe(verb,response);V *value=at(response,1);
  if((!strcmp(action,"new")||!strcmp(action,"open"))&&(!strcmp(code,"created")||!strcmp(code,"existing")||!strcmp(code,"found")||!strcmp(code,"opened"))){snprintf(selection,sizeof(selection),"%s",argument);printf("Conversation: %s\n",selection);participants(value);}
  else if(!strcmp(action,"home")&&!strcmp(code,"found")){selection[0]=0;}
  else if(!strcmp(action,"history")&&value){participants(value);history(value);}
  else if(!strcmp(action,"invite")&&!strcmp(code,"invited"))participants(value);
  else if(!strcmp(action,"remove")&&!strcmp(code,"removed"))participants(v_record_get(value,"conversation"));
  else if(!strcmp(action,"coworkers")&&!strcmp(code,"found")){
    for(size_t i=0;value&&i<value->n;i++){
      int duplicate=0;for(size_t j=0;j<i;j++)if(v_isstr(value->items[j],symbol(value->items[i])))duplicate=1;
      if(!duplicate)printf("- %s (%s)\n",tail(value->items[i]),symbol(value->items[i]));
    }
  }
  else if(!strcmp(action,"actions")||!strcmp(action,"list")||!strcmp(action,"status")){puts(v_json(value));}
  return 0;
}
int workspace_main(const char *directory,const char *source,const char *compiler){
  jmp_buf startup;jmp_buf *previous=v_handler;v_handler=&startup;
  if(setjmp(startup)){fprintf(stderr,"libcwr: %s\n",v_error);host_close();coworker_close();v_handler=previous;return 1;}
  host_prepare_signals();if(source&&coworker_configure(source,compiler)){coworker_close();v_handler=previous;return 1;}
  struct stat info;if(lstat(directory,&info)<0){if(errno!=ENOENT)v_fail("cannot inspect workspace");host_init(directory);host_close();}
  host_open(directory,NULL);host_workspace_environment();selection[0]=0;worker_pid=0;worker_fd=-1;
  puts("Workspace home. /help lists commands; /new NAME creates a conversation.");prompt();
  char *line=malloc(TEXT_LIMIT+1);if(!line)v_fail("allocation failed");size_t length=0;int oversized=0,done=0;
  while(!done){
    jmp_buf turn;v_handler=&turn;
    if(setjmp(turn)){fprintf(stderr,"libcwr: %s\n",v_error);length=0;oversized=0;host_workspace_refresh();prompt();continue;}
    worker_tick();host_workspace_refresh();
    struct pollfd fds[2]={{STDIN_FILENO,POLLIN,0},{worker_fd,POLLIN,0}};
    int ready=poll(fds,2,50);if(ready<0){if(errno==EINTR)continue;v_fail("terminal polling failed");}
    if(fds[0].revents&(POLLIN|POLLHUP)){
      char bytes[4096];ssize_t count=read(STDIN_FILENO,bytes,sizeof(bytes));if(count<0){if(errno==EINTR||errno==EAGAIN)continue;v_fail("terminal input failed");}
      if(!count){if(length){line[length]=0;done=command(line);}break;}
      for(ssize_t i=0;i<count&&!done;i++){
        unsigned char c=(unsigned char)bytes[i];if(c=='\n'){
          prompt_shown=0;if(oversized)fputs("libcwr: input exceeds bound or contains NUL\n",stderr);else{if(length&&line[length-1]=='\r')--length;line[length]=0;done=command(line);worker_tick();}
          length=0;oversized=0;host_workspace_refresh();if(!done)prompt();
        }else if(!c)oversized=1;else if(length<TEXT_LIMIT)line[length++]=(char)c;else oversized=1;
      }
    }
  }
  v_handler=&startup;
  if(worker_pid>0){workspace_interrupt();int status;while(waitpid((pid_t)worker_pid,&status,0)<0&&errno==EINTR){}worker_pid=0;}
  if(worker_fd>=0)close(worker_fd);worker_fd=-1;free(worker_record);worker_record=NULL;
  free(line);host_close();coworker_close();v_handler=previous;return 0;
}
