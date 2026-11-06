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
static int active;
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
}
int coworker_active(void) { return active; }
static int error(const char *message) {
  fprintf(stderr, "libcwr: %s\n", message);
  return 1;
}
int coworker_configure(const char *source, const char *compiler) {
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
  if (coworker_reload()) { coworker_close(); return 1; }
  return 0;
}
int coworker_reload(void) {
  if (!source_path || !compiler_path || !*build_directory)
    return error("no source coworker configured; launch chat with --source and --rbel");
  double current = now();
  if (current < 0) return error("cannot read monotonic clock");
  unlink(candidate_path);
  sigset_t previous_signals; block_interrupts(&previous_signals);
  pid_t pid = fork();
  if (!pid) {
    prepare_child(&previous_signals);
    int empty = open("/dev/null", O_RDONLY);
    if (empty < 0 || dup2(empty, STDIN_FILENO) < 0) _exit(126);
    if (empty != STDIN_FILENO) close(empty);
    char *arguments[] = {compiler_path, "build", source_path, "-o", candidate_path, "--quiet", NULL};
    execv(compiler_path, arguments);
    fprintf(stderr, "libcwr: cannot execute rbel compiler: %s\n", strerror(errno));
    _exit(127);
  }
  track_child(pid, &previous_signals);
  if (pid < 0) return error("cannot start coworker compiler");
  int status = 0;
  for (;;) {
    pid_t done = waitpid(pid, &status, WNOHANG);
    if (done == pid) { child_pid = 0; break; }
    if (done < 0 && errno != EINTR) { stop(pid, &status); unlink(candidate_path); return error("cannot wait for coworker compiler"); }
    if (deadline_passed(current + BUILD_SECONDS)) { stop(pid, &status); unlink(candidate_path); return error("coworker build timed out; last working version retained"); }
    struct timespec pause = {0, 20000000}; nanosleep(&pause, NULL);
  }
  struct stat info;
  if (!WIFEXITED(status) || WEXITSTATUS(status) || lstat(candidate_path, &info) ||
      !S_ISREG(info.st_mode) || access(candidate_path, X_OK)) {
    unlink(candidate_path);
    return error("coworker build failed; last working version retained");
  }
  if (rename(candidate_path, active_path)) { unlink(candidate_path); return error("cannot activate coworker build; last working version retained"); }
  active = 1;
  return 0;
}
static int make_pipe(int descriptors[2]) {
  if (pipe(descriptors)) return -1;
  if (fcntl(descriptors[0], F_SETFD, FD_CLOEXEC) < 0 || fcntl(descriptors[1], F_SETFD, FD_CLOEXEC) < 0) {
    close(descriptors[0]); close(descriptors[1]); return -1;
  }
  return 0;
}
char *coworker_transform(const char *input, size_t size, size_t *output_size) {
  if (!active) v_fail("no source coworker is active");
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
    char *arguments[] = {active_path, NULL}; execv(active_path, arguments); _exit(127);
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
