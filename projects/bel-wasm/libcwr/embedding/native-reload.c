/* Explicitly rebuild a local source coworker; protocol state stays in the host. */
#define _DARWIN_C_SOURCE 1
#define _POSIX_C_SOURCE 200809L
#include "native-reload.h"
#include "native-values.h"
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define TEXT_BOUND (256u * 1024u)
#define BUILD_SECONDS 120
#define RUN_SECONDS 10
static char *source_path, *compiler_path;
static char build_directory[PATH_MAX];
static char candidate_path[PATH_MAX], active_path[PATH_MAX];
static int active, resource_mode;
static char *resource_descriptor;
static unsigned long resource_version;
static int (*resource_validator)(V *, V *);
static V *resource_query(const char *path,V *request);
static int validate_candidate(char **descriptor);
void coworker_resource_validator(int (*validator)(V *,V *)){resource_validator=validator;}
int coworker_resource_mode(void){return resource_mode;}
const char *coworker_source_path(void){return source_path;}
const char *coworker_build_path(void){return active?active_path:NULL;}
unsigned long coworker_build_version(void){return resource_version;}
V *coworker_resource_descriptor(void){return resource_descriptor?v_parse(resource_descriptor,strlen(resource_descriptor)):NULL;}

#define DIAGNOSTIC_BOUND 8192u
static char diagnostics[DIAGNOSTIC_BOUND + 1];
static size_t diagnostic_size;
static int diagnostic_truncated;
const char *coworker_diagnostics(void) { return diagnostics; }
static void diagnostic_append(const char *text, size_t size) {
  size_t room = DIAGNOSTIC_BOUND - diagnostic_size;
  if (size > room) { size = room; diagnostic_truncated = 1; }
  memcpy(diagnostics + diagnostic_size, text, size);
  diagnostic_size += size; diagnostics[diagnostic_size] = 0;
}
static void diagnostic_text(const char *text) { diagnostic_append(text, strlen(text)); }
/* Compiler output is a byte stream; malformed UTF-8 must not strand an action. */
static void diagnostic_utf8(void) {
  unsigned char *s = (unsigned char *)diagnostics;
  for (size_t i = 0; i < diagnostic_size;) {
    unsigned c = s[i]; size_t width = c < 128 ? 1 : c >= 194 && c <= 223 ? 2 : c >= 224 && c <= 239 ? 3 : c >= 240 && c <= 244 ? 4 : 0;
    int valid = width && width <= diagnostic_size - i && c;
    for (size_t j = 1; valid && j < width; j++) if ((s[i+j] & 192) != 128) valid = 0;
    if (valid && width == 3 && ((c == 224 && s[i+1] < 160) || (c == 237 && s[i+1] >= 160))) valid = 0;
    if (valid && width == 4 && ((c == 240 && s[i+1] < 144) || (c == 244 && s[i+1] >= 144))) valid = 0;
    if (!valid) { s[i] = '?'; i++; } else i += width;
  }
  if (diagnostic_truncated) {
    const char *notice = "\n[compiler output truncated]\n"; size_t n = strlen(notice);
    size_t start = diagnostic_size > n ? diagnostic_size - n : 0;
    while (start && (s[start] & 192) == 128) start--;
    memcpy(diagnostics + start, notice, n); diagnostics[start+n] = 0;
  }
}
static volatile sig_atomic_t child_pid;
static int cleanup_registered;

void coworker_interrupt(void) {
  pid_t pid = (pid_t)child_pid;
  if (pid > 0) { kill(-pid, SIGKILL); kill(pid, SIGKILL); }
}
static void block_interrupts(sigset_t *previous) {
  sigset_t signals; sigemptyset(&signals);
  sigaddset(&signals, SIGINT); sigaddset(&signals, SIGTERM);
  sigprocmask(SIG_BLOCK, &signals, previous);
}
static void prepare_child(const sigset_t *previous) {
  child_pid = 0; setpgid(0, 0);
  struct sigaction action = {0}; action.sa_handler = SIG_DFL; sigemptyset(&action.sa_mask);
  sigaction(SIGINT, &action, NULL); sigaction(SIGTERM, &action, NULL);
  sigprocmask(SIG_SETMASK, previous, NULL);
}
static void track_child(pid_t pid, const sigset_t *previous) {
  child_pid = pid > 0 ? (sig_atomic_t)pid : 0;
  if (pid > 0) setpgid(pid, pid);
  sigprocmask(SIG_SETMASK, previous, NULL);
}

static double now(void) {
  struct timespec time;
  if (clock_gettime(CLOCK_MONOTONIC, &time)) return -1;
  return (double)time.tv_sec + (double)time.tv_nsec / 1000000000.0;
}
static int deadline_passed(double deadline) {
  double current = now();
  return current < 0 || current >= deadline;
}
static void reap(pid_t pid, int *status) {
  while (waitpid(pid, status, 0) < 0 && errno == EINTR) { }
}
static void stop(pid_t pid, int *status) {
  /* The compiler's WABT/C children belong to this private process group too. */
  kill(-pid, SIGKILL);
  kill(pid, SIGKILL);
  reap(pid, status);
  child_pid = 0;
}
static void remove_contents(const char *directory) {
  DIR *dir = opendir(directory);
  if (!dir) return;
  struct dirent *entry;
  while ((entry = readdir(dir))) {
    if (!strcmp(entry->d_name, ".") || !strcmp(entry->d_name, "..")) continue;
    char path[PATH_MAX];
    if (snprintf(path, sizeof(path), "%s/%s", directory, entry->d_name) >= (int)sizeof(path)) continue;
    struct stat info;
    if (!lstat(path, &info) && S_ISDIR(info.st_mode)) { remove_contents(path); rmdir(path); }
    else unlink(path);
  }
  closedir(dir);
}
void coworker_close(void) {
  if (child_pid > 0) { int status; stop((pid_t)child_pid, &status); }
  if (*build_directory) { remove_contents(build_directory); rmdir(build_directory); }
  *build_directory = *candidate_path = *active_path = 0;
  free(source_path); free(compiler_path);
  source_path = compiler_path = NULL; active = 0;
  free(resource_descriptor);resource_descriptor=NULL;resource_mode=0;resource_version=0;
}
int coworker_active(void) { return active; }
static int error(const char *message) {
  diagnostic_text("libcwr: "); diagnostic_text(message); diagnostic_text("\n");
  diagnostic_utf8();
  fprintf(stderr, "libcwr: %s\n", message);
  return 1;
}
int coworker_prepare(const char *source, const char *compiler) {
  diagnostic_size = 0; diagnostic_truncated = 0; diagnostics[0] = 0;
  coworker_close();
  if (!cleanup_registered) {
    if (atexit(coworker_close)) return error("cannot register coworker cleanup");
    cleanup_registered = 1;
  }
  char resolved_source[PATH_MAX], resolved_compiler[PATH_MAX];
  struct stat info;
  if (!source || !compiler || !realpath(source, resolved_source) ||
      stat(resolved_source, &info) || !S_ISREG(info.st_mode))
    return error("coworker source must be an existing regular file");
  if (!realpath(compiler, resolved_compiler) || stat(resolved_compiler, &info) ||
      !S_ISREG(info.st_mode) || access(resolved_compiler, X_OK))
    return error("rbel compiler must be an executable regular file");
  source_path = strdup(resolved_source); compiler_path = strdup(resolved_compiler);
  if (!source_path || !compiler_path) { coworker_close(); return error("allocation failed"); }
  /* This directory holds products only; no source or saved workspace is changed. */
  snprintf(build_directory, sizeof(build_directory), "/tmp/libcwr-coworker-XXXXXX");
  if (!mkdtemp(build_directory)) { *build_directory = 0; coworker_close(); return error("cannot create coworker build directory"); }
  if (snprintf(candidate_path, sizeof(candidate_path), "%s/candidate", build_directory) >= (int)sizeof(candidate_path) ||
      snprintf(active_path, sizeof(active_path), "%s/active", build_directory) >= (int)sizeof(active_path)) {
    coworker_close(); return error("coworker build path too long");
  }
  return 0;
}
int coworker_configure(const char *source, const char *compiler) {
  if (coworker_prepare(source, compiler)) return 1;
  if (coworker_reload()) { coworker_close(); return 1; }
  return 0;
}
int coworker_resource_prepare(const char *source,const char *compiler){
  int failed=coworker_prepare(source,compiler);if(!failed)resource_mode=1;return failed;
}
int coworker_resource_configure(const char *source,const char *compiler){
  if(coworker_resource_prepare(source,compiler))return 1;
  if(coworker_reload()){coworker_close();return 1;}return 0;
}
static int compile_source(int checking) {
  diagnostic_size = 0; diagnostic_truncated = 0; diagnostics[0] = 0;
  diagnostic_text(checking ? "Check source: " : "Reload source: ");
  diagnostic_text(source_path ? source_path : "(unconfigured)"); diagnostic_text("\n");
  if (!source_path || !compiler_path || !*build_directory)
    return error("no source coworker configured; launch with --source and --rbel");
  double current = now();
  if (current < 0) return error("cannot read monotonic clock");
  if (!checking || resource_mode) unlink(candidate_path);
  int output[2];
  if (pipe(output)) return error("cannot capture compiler diagnostics");
  if (fcntl(output[0], F_SETFL, O_NONBLOCK) < 0) { close(output[0]); close(output[1]); return error("cannot configure compiler diagnostics"); }
  sigset_t previous_signals; block_interrupts(&previous_signals);
  pid_t pid = fork();
  if (!pid) {
    prepare_child(&previous_signals);
    int empty = open("/dev/null", O_RDONLY);
    if (empty < 0 || dup2(empty, STDIN_FILENO) < 0) _exit(126);
    if (empty != STDIN_FILENO) close(empty);
    close(output[0]);
    if (dup2(output[1], STDOUT_FILENO) < 0 || dup2(output[1], STDERR_FILENO) < 0) _exit(126);
    if (output[1] != STDOUT_FILENO && output[1] != STDERR_FILENO) close(output[1]);
    char *build_arguments[] = {compiler_path, "build", source_path, "-o", candidate_path, "--quiet", NULL};
    char *resource_arguments[] = {compiler_path,"build",source_path,"-o",candidate_path,"--quiet","--value-interface",NULL};
    char *check_arguments[] = {compiler_path, "check", source_path, NULL};
    execv(compiler_path, resource_mode ? resource_arguments : checking ? check_arguments : build_arguments);
    fprintf(stderr, "libcwr: cannot execute rbel compiler: %s\n", strerror(errno));
    _exit(127);
  }
  track_child(pid, &previous_signals);
  close(output[1]);
  if (pid < 0) { close(output[0]); return error("cannot start coworker compiler"); }
  int status = 0, reaped = 0, eof = 0;
  while (!reaped || !eof) {
    char buffer[1024]; ssize_t n = read(output[0], buffer, sizeof(buffer));
    if (n > 0) diagnostic_append(buffer, (size_t)n);
    else if (!n) eof = 1;
    else if (errno != EINTR && errno != EAGAIN && errno != EWOULDBLOCK) {
      stop(pid, &status); close(output[0]); return error("cannot read compiler diagnostics");
    }
    if (!reaped) {
      pid_t done = waitpid(pid, &status, WNOHANG);
      if (done == pid) reaped = 1;
      else if (done < 0 && errno != EINTR) { stop(pid, &status); close(output[0]); return error("cannot wait for coworker compiler"); }
    }
    if (reaped && eof) break;
    if (deadline_passed(current + BUILD_SECONDS)) {
      if (!reaped) stop(pid, &status); else kill(-pid, SIGKILL);
      child_pid = 0; close(output[0]); if (!checking || resource_mode) unlink(candidate_path);
      return error("coworker build timed out; last working version retained");
    }
    if (n <= 0) { struct pollfd ready = {eof ? -1 : output[0], POLLIN, 0}; poll(&ready, 1, 20); }
  }
  child_pid = 0; close(output[0]); diagnostic_utf8();
  /* Diagnostics go to stderr; JSONL stdout remains exclusively protocol data. */
  if (diagnostic_size) fputs(diagnostics, stderr);
  if (checking && !resource_mode) {
    if (!WIFEXITED(status) || WEXITSTATUS(status)) return error("source check failed; last working version retained");
    diagnostic_text("Source checks passed; active coworker unchanged.\n");
    return 0;
  }
  struct stat info;
  if (!WIFEXITED(status) || WEXITSTATUS(status) || lstat(candidate_path, &info) ||
      !S_ISREG(info.st_mode) || access(candidate_path, X_OK)) {
    unlink(candidate_path);
    return error("coworker build failed; last working version retained");
  }
  char *candidate_descriptor=NULL;
  if(resource_mode && validate_candidate(&candidate_descriptor)){unlink(candidate_path);return 1;}
  if(checking){free(candidate_descriptor);unlink(candidate_path);diagnostic_text("Source and resource checks passed; active coworker unchanged.\n");return 0;}
  if (rename(candidate_path, active_path)) { free(candidate_descriptor);unlink(candidate_path); return error("cannot activate coworker build; last working version retained"); }
  active = 1;resource_version++;
  if(resource_mode){free(resource_descriptor);resource_descriptor=candidate_descriptor;}
  diagnostic_text("Reloaded coworker source.\n");
  return 0;
}
int coworker_reload(void) { return compile_source(0); }
int coworker_check(void) { return compile_source(1); }
static int make_pipe(int descriptors[2]) {
  if (pipe(descriptors)) return -1;
  if (fcntl(descriptors[0], F_SETFD, FD_CLOEXEC) < 0 || fcntl(descriptors[1], F_SETFD, FD_CLOEXEC) < 0) {
    close(descriptors[0]); close(descriptors[1]); return -1;
  }
  return 0;
}
static char *transform_path(const char *path,const char *input, size_t size, size_t *output_size) {
  if (size > TEXT_BOUND) v_fail("source coworker input exceeds 256 KiB");
  /* Allocate before spawning: a value-arena failure cannot strand a child. */
  char *output = v_alloc(TEXT_BOUND + 1);
  int incoming[2], outgoing[2];
  if (make_pipe(incoming)) { v_fail("cannot create coworker input pipe"); }
  if (make_pipe(outgoing)) { close(incoming[0]); close(incoming[1]); v_fail("cannot create coworker output pipe"); }
  double started = now();
  if (started < 0) { close(incoming[0]); close(incoming[1]); close(outgoing[0]); close(outgoing[1]); v_fail("cannot read monotonic clock"); }
  /* A failed child must never deliver SIGPIPE to the interactive host. */
  struct sigaction ignored = {0}, previous;
  ignored.sa_handler = SIG_IGN; sigemptyset(&ignored.sa_mask);
  if (sigaction(SIGPIPE, &ignored, &previous)) { close(incoming[0]); close(incoming[1]); close(outgoing[0]); close(outgoing[1]); v_fail("cannot protect coworker pipe"); }
  sigset_t previous_signals; block_interrupts(&previous_signals);
  pid_t pid = fork();
  if (!pid) {
    prepare_child(&previous_signals); sigaction(SIGPIPE, &previous, NULL);
    if (dup2(incoming[0], STDIN_FILENO) < 0 || dup2(outgoing[1], STDOUT_FILENO) < 0) _exit(126);
    close(incoming[0]); close(incoming[1]); close(outgoing[0]); close(outgoing[1]);
    char *arguments[] = {(char *)path, NULL}; execv(path, arguments); _exit(127);
  }
  track_child(pid, &previous_signals);
  close(incoming[0]); close(outgoing[1]);
  if (pid < 0) {
    close(incoming[1]); close(outgoing[0]); sigaction(SIGPIPE, &previous, NULL);
    v_fail("cannot start source coworker");
  }
  const char *failure = NULL;
  if (fcntl(incoming[1], F_SETFL, O_NONBLOCK) < 0 || fcntl(outgoing[0], F_SETFL, O_NONBLOCK) < 0)
    failure = "cannot configure coworker pipes";
  size_t sent = 0, received = 0; int status = 0, reaped = 0;
  if (!size) { close(incoming[1]); incoming[1] = -1; }
  while (!failure && (!reaped || outgoing[0] >= 0)) {
    if (deadline_passed(started + RUN_SECONDS)) { failure = "source coworker execution timed out"; break; }
    struct pollfd pipes[2] = {{incoming[1], POLLOUT, 0}, {outgoing[0], POLLIN, 0}};
    int result = poll(pipes, 2, 50);
    if (result < 0 && errno != EINTR) { failure = "source coworker pipe polling failed"; break; }
    if (result > 0 && incoming[1] >= 0 && pipes[0].revents) {
      ssize_t n = write(incoming[1], input + sent, size - sent);
      if (n > 0) sent += (size_t)n;
      else if (n < 0 && errno != EINTR && errno != EAGAIN && errno != EWOULDBLOCK) {
        if (errno != EPIPE) failure = "source coworker input failed";
        close(incoming[1]); incoming[1] = -1;
      }
      if (incoming[1] >= 0 && sent == size) { close(incoming[1]); incoming[1] = -1; }
    }
    if (result > 0 && outgoing[0] >= 0 && pipes[1].revents) {
      /* Read one extra byte to distinguish exactly-at-limit output from overflow. */
      ssize_t n = read(outgoing[0], output + received, TEXT_BOUND + 1 - received);
      if (n > 0) {
        received += (size_t)n;
        if (received > TEXT_BOUND) failure = "source coworker output exceeds 256 KiB";
      } else if (!n) { close(outgoing[0]); outgoing[0] = -1; }
      else if (errno != EINTR && errno != EAGAIN && errno != EWOULDBLOCK) failure = "source coworker output failed";
    }
    if (!reaped) {
      pid_t done = waitpid(pid, &status, WNOHANG);
      if (done == pid) reaped = 1;
      else if (done < 0 && errno != EINTR) failure = "cannot wait for source coworker";
    }
  }
  if (incoming[1] >= 0) close(incoming[1]);
  if (outgoing[0] >= 0) close(outgoing[0]);
  if (failure) { if (!reaped) stop(pid, &status); else kill(-pid, SIGKILL); }
  child_pid = 0;
  sigaction(SIGPIPE, &previous, NULL);
  if (!failure && (!WIFEXITED(status) || WEXITSTATUS(status))) failure = "source coworker execution failed";
  if (failure) { v_fail("%s", failure); }
  output[received] = 0;
  *output_size = received;
  return output;
}

char *coworker_transform(const char *input,size_t size,size_t *output_size){
  if(!active)v_fail("no source coworker is active");
  return transform_path(active_path,input,size,output_size);
}
static V *resource_query(const char *path,V *request){
  char *input=v_json(request);size_t size;
  char *output=transform_path(path,input,strlen(input),&size);
  return v_parse(output,size);
}
V *coworker_resource_invoke(V *request){
  if(!resource_mode||!active)v_fail("no resource program is active");
  return resource_query(active_path,request);
}
static int validate_candidate(char **descriptor){
  jmp_buf local; jmp_buf *previous=v_handler;v_handler=&local;
  if(setjmp(local)){v_handler=previous;diagnostic_text(v_error);diagnostic_text("\n");return error("resource validation failed; last working version retained");}
  V *request=v_array();v_push(request,v_string("describe"));
  V *answer=resource_query(candidate_path,request);
  if(!answer||answer->type!=VT_ARRAY||answer->n!=2||!v_isstr(answer->items[0],"ok"))v_fail("resource describe failed");
  V *definition=answer->items[1],*old=coworker_resource_descriptor();
  if(!resource_validator||!resource_validator(definition,old))v_fail("resource interface or stable identity rejected");
  *descriptor=strdup(v_json(definition));if(!*descriptor)v_fail("resource allocation failed");
  v_handler=previous;return 0;
}
