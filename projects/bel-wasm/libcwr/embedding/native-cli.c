/* Readable local commands over the same compiled protocol and owning host. */
#include "native-host.h"
#include "native-reload.h"
#include "native-workspace.h"
#include "build-info.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <ctype.h>
#include <sys/stat.h>
#include <fcntl.h>
#include <errno.h>

#define CLI_TEXT_LIMIT (256u * 1024u)
static V *at(V *value, size_t index) {
  return value && value->type == VT_ARRAY && index < value->n ? value->items[index] : NULL;
}
static void field(V *record, const char *key, V *value) {
  V *entry = v_array();
  v_push(entry, v_string(key));
  v_push(entry, value);
  v_push(record, entry);
}
static V *event(const char *operation) {
  V *value = v_array();
  field(value, "op", v_string(operation));
  return value;
}
static void help(void) {
  puts("libcwr - a local, inspectable conversation/action runtime\n"
       "\n"
       "  workspace [--directory DIR]       Open the workspace home\n"
       "  chat [--directory DIR]            Chat with the local rbel coworker\n"
       "       [--source SOURCE --rbel COMPILER]  Enable source reload\n"
       "  repl [--directory DIR]            Start an interactive session\n"
       "  (no arguments on a terminal starts workspace)\n"
       "  Inside: use NAME, submit INVOCATION --text TEXT, history, exit\n"
       "  Quotes and backslashes group text; no shell expansion.\n"
       "\n"
       "Commands (workspace defaults to .libcwr):\n"
       "  init                              Create a new local demo workspace\n"
       "  conversation create NAME          Create a conversation\n"
       "  submit CONVERSATION INVOCATION --text TEXT\n"
       "                                    Admit a text action without running it\n"
       "  run INVOCATION                    Run the admitted action\n"
       "  show INVOCATION                   Inspect input, phase and result\n"
       "  list conversations|invocations    List visible work\n"
       "  status                            Show workspace progress\n"
       "  history CONVERSATION              Read conversation messages\n"
       "  artifact INVOCATION               Print its local output text\n"
       "  reconcile INVOCATION              Check its bound execution receipt\n"
       "  recover                           Recover and inspect interrupted work\n"
       "  version, --version                Show build and protocol-core hashes\n"
       "\n"
       "Options after a command: --directory DIR, --json, --help\n"
       "Names: 1-64 ASCII letters, digits, hyphens or underscores.\n"
       "\n"
       "Try it:\n"
       "  libcwr init --directory ./trial\n"
       "  libcwr conversation create notes --directory ./trial\n"
       "  libcwr submit notes first --text 'hello, libcwr' --directory ./trial\n"
       "  libcwr run first --directory ./trial\n"
       "  libcwr status --directory ./trial\n"
       "  libcwr artifact first --directory ./trial\n"
       "\n"
       "This version runs the deterministic local store--text action.\n"
       "No model/provider or network service is connected. Repeated run of a\n"
       "completed invocation returns its retained result without repeating it.\n"
       "Recovery does not assume a missing receipt means execution never happened.\n"
       "Keep the workspace context.json private: it is trusted host configuration.\n"
       "\n"
       "Developer transport remains available:\n"
       "  libcwr --directory STATE --context TRUSTED.json < events.jsonl");
}
static void valid_name(const char *name) {
  size_t n = strlen(name);
  if (n < 1 || n > 64) {
    v_fail("names require 1-64 ASCII letters, digits, hyphens or underscores");
  }
  for (size_t i = 0; i < n; i++) {
    unsigned char c = (unsigned char)name[i];
    if (!((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
          (c >= '0' && c <= '9') || c == '-' || c == '_')) {
      v_fail("names require 1-64 ASCII letters, digits, hyphens or underscores");
    }
  }
}
static V *reference(const char *kind, const char *name) {
  valid_name(name);
  V *context = v_record_get(host_environment(), "context");
  if (!context || context->type != VT_STRING || memchr(context->s, 0, context->n)) {
    v_fail("workspace context must be a nonempty text reference");
  }
  size_t n = context->n + strlen(kind) + strlen(name) + 3;
  char *text = v_alloc(n);
  snprintf(text, n, "%s/%s/%s", context->s, kind, name);
  return v_string(text);
}
static const char *display_name(V *reference_value) {
  if (!reference_value || reference_value->type != VT_STRING ||
      memchr(reference_value->s, 0, reference_value->n)) {
    return "?";
  }
  const char *tail = strrchr(reference_value->s, '/');
  return tail ? tail + 1 : reference_value->s;
}
static const char *phase(V *record) {
  V *value = v_record_get(record, "phase");
  return value && value->type == VT_STRING ? value->s : "?";
}
static V *read_record(const char *kind, V *ref) {
  V *request = event("read");
  field(request, "kind", v_string(kind));
  field(request, "ref", ref);
  return host_event(request);
}
static V *visible_records(const char *kind, const char *collection) {
  (void)collection;
  V *request = event("list");
  field(request, "kind", v_string(kind));
  V *response = host_event(request);
  if (!v_isstr(at(response, 0), "found")) v_fail("record listing unavailable");
  V *records = at(response, 1);
  return records ? records : v_array();
}
static V *overview(void) {
  V *result = v_object();
  v_put(result, "revision", v_string(LIBCWR_BUILD_REVISION));
  v_put(result, "core_sha256", v_string(LIBCWR_CORE_SHA256));
  v_put(result, "context", v_record_get(host_environment(), "context"));
  v_put(result, "conversations", visible_records("conversation", "conversations"));
  v_put(result, "invocations", visible_records("invocation", "invocations"));
  return result;
}
static int successful(const char *code) {
  const char *codes[] = {"created", "existing", "admitted", "running", "completed", "found",
                         "ok", "initialized", "recovered", "not-uncertain", "retry-ready"};
  for (size_t i = 0; i < sizeof(codes) / sizeof(codes[0]); i++) {
    if (!strcmp(code, codes[i])) {
      return 1;
    }
  }
  return 0;
}
static void print_invocation(V *record, int detail) {
  printf("%s  %s", display_name(v_record_get(record, "ref")), phase(record));
  if (v_record_get(record, "uncertain")) {
    fputs("  (uncertain; reconcile before retry)", stdout);
  }
  putchar('\n');
  if (detail) {
    printf("  Conversation: %s\n", display_name(v_record_get(record, "conversation")));
    printf("  Input: %s\n", v_json(v_string(v_text(v_record_get(record, "input")))));
    V *result = v_record_get(record, "result");
    if (result) {
      printf("  Result: %s\n", v_json(v_string(v_text(result))));
    }
  }
}
static void print_records(V *records, int invocations) {
  if (!records || records->type != VT_ARRAY || !records->n) {
    puts("  (none)");
    return;
  }
  for (size_t i = 0; i < records->n; i++) {
    if (invocations) {
      print_invocation(records->items[i], 0);
    } else {
      printf("%s\n", display_name(v_record_get(records->items[i], "ref")));
    }
  }
}
static void output(const char *command, const char *code, V *value, int json) {
  if (json) {
    V *response = v_object();
    v_put(response, "command", v_string(command));
    v_put(response, "code", v_string(code));
    v_put(response, "value", value);
    puts(v_json(response));
    return;
  }
  if (!successful(code)) {
    printf("%s: %s\n", command, code);
    return;
  }
  if (!strcmp(command, "version")) {
    printf("libcwr %s\nCore SHA256: %s\n", LIBCWR_BUILD_REVISION, LIBCWR_CORE_SHA256);
  } else if (!strcmp(command, "init")) {
    V *directory = v_get(value, "directory");
    printf("Initialized local demo workspace: %s\nAction: store--text (local text file; no provider)\n",
           directory->s);
    puts("Next: libcwr conversation create notes --directory DIR");
  } else if (!strcmp(command, "conversation create")) {
    printf("Conversation %s: %s\n", display_name(v_record_get(value, "ref")), code);
  } else if (!strcmp(command, "submit") || !strcmp(command, "run") ||
             !strcmp(command, "show") || !strcmp(command, "reconcile")) {
    printf("%s: %s\n", command, code);
    print_invocation(value, 1);
  } else if (!strcmp(command, "list conversations")) {
    print_records(value, 0);
  } else if (!strcmp(command, "list invocations")) {
    print_records(value, 1);
  } else if (!strcmp(command, "status") || !strcmp(command, "recover")) {
    if (!strcmp(command, "recover")) {
      puts("Recovery checked; uncertain actions are not automatically repeated.");
    }
    printf("Build: %s\nConversations:\n", LIBCWR_BUILD_REVISION);
    print_records(v_get(value, "conversations"), 0);
    puts("Invocations:");
    print_records(v_get(value, "invocations"), 1);
  } else if (!strcmp(command, "history")) {
    if (!value || value->type != VT_ARRAY || !value->n) {
      puts("No messages.");
    } else {
      for (size_t i = 0; i < value->n; i++) {
        V *message = value->items[i];
        printf("%s: %s\n", display_name(v_record_get(message, "ref")),
               v_json(v_string(v_text(v_record_get(message, "text")))));
      }
    }
  } else if (!strcmp(command, "artifact")) {
    if (value && value->type == VT_STRING) {
      if (fwrite(value->s, 1, value->n, stdout) != value->n) {
        v_fail("artifact output failed");
      }
    }
  }
}
/* REPL storage is independent of the per-command value arena. */
static int select_conversation(const char *directory, const char *name) {
  jmp_buf top;
  jmp_buf *previous = v_handler;
  v_handler = &top;
  if (setjmp(top)) {
    fprintf(stderr, "libcwr: %s\n", v_error);
    host_close();
    v_handler = previous;
    return 1;
  }
  host_open(directory, NULL);
  V *reply = read_record("conversation", reference("conversations", name));
  int ok = v_isstr(at(reply, 0), "found");
  if (!ok) fprintf(stderr, "use: conversation unavailable\n");
  host_close();
  v_handler = previous;
  return ok ? 0 : 1;
}
/* Tokenize without invoking a shell or performing any expansion. */
static int split_line(char *line, char **words, int limit) {
  char *read = line, *write = line;
  int count = 0;
  while (*read) {
    while (isspace((unsigned char)*read)) read++;
    if (!*read) break;
    if (count == limit) return -1;
    words[count++] = write;
    int quote = 0;
    while (*read) {
      unsigned char c = (unsigned char)*read++;
      if (!quote && isspace(c)) break;
      if (c == '\\' && quote != '\'') {
        if (!*read) return -1;
        *write++ = *read++;
      } else if (c == '\'' || c == '"') {
        if (!quote) quote = c;
        else if (quote == c) quote = 0;
        else *write++ = (char)c;
      } else *write++ = (char)c;
    }
    if (quote) return -1;
    *write++ = 0;
  }
  return count;
}
static void chat_name(char *name, size_t size, const char *prefix) {
  unsigned char random[12];
  int fd = open("/dev/urandom", O_RDONLY);
  if (fd < 0) v_fail("cannot generate chat identity");
  size_t got = 0;
  while (got < sizeof(random)) {
    ssize_t n = read(fd, random + got, sizeof(random) - got);
    if (n < 0 && errno == EINTR) continue;
    if (n <= 0) { close(fd); v_fail("cannot generate chat identity"); }
    got += (size_t)n;
  }
  close(fd);
  char hex[25];
  for (size_t i = 0; i < sizeof(random); i++) snprintf(hex + i * 2, 3, "%02x", random[i]);
  snprintf(name, size, "%s-%s", prefix, hex);
}
static void chat_text(V *text) {
  char *quoted = v_json(v_string(v_text(text)));
  size_t n = strlen(quoted);
  if (n >= 2 && fwrite(quoted + 1, 1, n - 2, stdout) != n - 2) v_fail("chat output failed");
}
/* Each operation reopens authoritative state and releases the owning lock. */
static int chat_action(const char *directory, const char *verb, const char *argument,
                       const char *text, char *selection) {
  jmp_buf top;
  jmp_buf *previous = v_handler;
  v_handler = &top;
  if (setjmp(top)) {
    fprintf(stderr, "libcwr: %s\n", v_error);
    if (!strcmp(verb, "send")) fputs("Use /status to inspect retained work before retrying.\n", stderr);
    host_close();
    v_handler = previous;
    return 1;
  }
  if (!strcmp(verb, "start")) {
    struct stat st;
    if (lstat(directory, &st) < 0) {
      if (errno != ENOENT) v_fail("cannot inspect chat workspace");
      host_init(directory);
      host_close();
    }
  }
  host_open(directory, NULL);
  host_chat_binding();
  V *reply = NULL;
  if (!strcmp(verb, "start") || !strcmp(verb, "new")) {
    char generated[65];
    const char *name = argument;
    if (!name) { chat_name(generated, sizeof(generated), "chat"); name = generated; }
    V *request = event("create");
    field(request, "conversation", reference("conversations", name));
    field(request, "owner", v_record_get(host_environment(), "context"));
    reply = host_event(request);
    if (v_isstr(at(reply, 0), "created") || v_isstr(at(reply, 0), "existing")) {
      snprintf(selection, 65, "%s", name);
      printf("Conversation: %s | coworker: uppercase (%s)\n", selection, coworker_active() ? "reloadable rbel source" : "local rbel");
    }
  } else if (!strcmp(verb, "use")) {
    reply = read_record("conversation", reference("conversations", argument));
    if (v_isstr(at(reply, 0), "found")) {
      snprintf(selection, 65, "%s", argument);
      printf("Conversation: %s\n", selection);
    }
  } else if (!strcmp(verb, "send")) {
    if (strlen(text) > CLI_TEXT_LIMIT) v_fail("text exceeds 256 KiB command bound");
    char name[65];
    chat_name(name, sizeof(name), "turn");
    reply = host_chat_send(reference("conversations", selection), name, text);
    if (v_isstr(at(reply, 0), "completed")) {
      fputs("uppercase: ", stdout);
      chat_text(v_record_get(at(reply, 1), "result"));
      putchar('\n');
    } else {
      printf("Work %s: %s; inspect /status before retrying.\n", name,
             at(reply, 0) && at(reply, 0)->type == VT_STRING ? at(reply, 0)->s : "unknown");
    }
  } else if (!strcmp(verb, "history")) {
    reply = read_record("conversation", reference("conversations", selection));
    if (v_isstr(at(reply, 0), "found")) {
      V *messages = v_record_get(at(reply, 1), "messages");
      if (!messages || !messages->n) puts("No messages yet.");
      for (size_t i = messages ? messages->n : 0; i > 0; i--) {
        V *message = messages->items[i - 1];
        V *author = v_record_get(message, "author");
        V *caller = v_record_get(host_environment(), "caller");
        int you = author && caller && author->type == VT_STRING && caller->type == VT_STRING && !strcmp(author->s, caller->s);
        printf("%s: ", you ? "you" : display_name(author));
        chat_text(v_record_get(message, "text"));
        putchar('\n');
      }
    }
  } else if (!strcmp(verb, "status") || !strcmp(verb, "recover")) {
    output(verb, !strcmp(verb, "recover") ? "recovered" : "ok", overview(), 0);
  } else if (!strcmp(verb, "reconcile") || !strcmp(verb, "run")) {
    V *request = event(verb);
    field(request, "ref", reference("invocations", argument));
    reply = host_event(request);
    output(verb, at(reply, 0)->s, at(reply, 1), 0);
    if (!strcmp(verb, "reconcile") && v_isstr(at(reply, 0), "reconciliation-required") &&
        v_isstr(v_record_get(at(reply, 1), "checked"), "t") &&
        v_isstr(v_record_get(v_record_get(host_environment(), "binding"), "repeat-safe"), "t") &&
        !strcmp(v_json(v_record_get(at(reply, 1), "target")),
                v_json(v_record_get(v_record_get(host_environment(), "binding"), "target")))) {
      printf("Receipt checked. Use /run %s to request a safe retry; current authority will be checked.\n", argument);
    }
  }
  int ok = !reply || (at(reply, 0) && at(reply, 0)->type == VT_STRING && successful(at(reply, 0)->s));
  if (!ok) printf("%s: %s\n", verb, at(reply, 0) && at(reply, 0)->type == VT_STRING ? at(reply, 0)->s : "invalid response");
  if (fflush(stdout) || ferror(stdout)) v_fail("chat output failed");
  host_close();
  v_handler = previous;
  return ok ? 0 : 1;
}
static void chat_help(void) {
  puts(coworker_active() ? "Type a message to run the configured rbel source coworker. No model is connected." :
       "Type a message and the local rbel coworker replies in uppercase.\nASCII a-z change; other characters stay unchanged. No model is connected.");
  puts(
       "/new [NAME]       Create/select a conversation\n"
       "/use NAME         Reopen a saved conversation\n"
       "/reload           Rebuild configured source; keep last good on failure\n"
       "/history          Read saved user and coworker messages\n"
       "/coworker [uppercase]  Show/select the local coworker\n"
       "/status, /recover, /reconcile INVOCATION  Inspect/recover work\n"
       "/run INVOCATION    Retry only after reconciliation permits it\n"
       "/help, /exit      Help or leave (EOF also exits)\n"
       "Prefix // to send a literal leading slash. Quotes in messages are literal.");
}
static int chat_main(const char *directory) {
  char selection[65] = "";
  if (chat_action(directory, "start", "chat", NULL, selection)) return 1;
  puts("Type /help for controls. Your messages and replies are saved here.");
  const size_t bound = CLI_TEXT_LIMIT + 4096u;
  char *line = malloc(bound + 1);
  if (!line) { fputs("libcwr: allocation failed\n", stderr); return 1; }
  int interactive = isatty(STDIN_FILENO), result = 0;
  for (;;) {
    if (interactive) {
      printf("you[%s]> ", selection);
      if (fflush(stdout)) { result = 1; break; }
    }
    size_t n = 0;
    int c, oversized = 0, nul = 0;
    while ((c = fgetc(stdin)) != EOF && c != '\n') {
      if (!c) nul = 1;
      if (n < bound) line[n++] = (char)c; else oversized = 1;
    }
    if (ferror(stdin)) { fputs("libcwr: input read failed\n", stderr); result = 1; break; }
    if (c == EOF && !n && !oversized) break;
    line[n] = 0;
    if (oversized || nul) { fputs("libcwr: message exceeds bound or contains NUL\n", stderr); continue; }
    if (!n) continue;
    if (line[0] != '/' || line[1] == '/') {
      chat_action(directory, "send", NULL, line + (line[0] == '/'), selection);
      continue;
    }
    char *words[4];
    int count = split_line(line + 1, words, 4);
    if (count < 1) { fputs("libcwr: invalid slash command; use /help\n", stderr); continue; }
    const char *verb = words[0];
    if (count == 1 && (!strcmp(verb, "exit") || !strcmp(verb, "quit"))) break;
    if (count == 1 && !strcmp(verb, "help")) { chat_help(); continue; }
    if (count == 1 && !strcmp(verb, "reload")) {
      if (!coworker_reload()) puts("Reloaded coworker source.");
      else fputs("libcwr: reload failed; last working coworker retained.\n", stderr);
      fflush(stdout); fflush(stderr); continue;
    }
    if (!strcmp(verb, "coworker") && (count == 1 || (count == 2 && !strcmp(words[1], "uppercase")))) {
      puts(coworker_active() ? "uppercase: reloadable rbel source coworker" : "uppercase: local rbel ASCII uppercase coworker"); continue;
    }
    int single = count == 1 && (!strcmp(verb, "history") || !strcmp(verb, "status") || !strcmp(verb, "recover"));
    int named = count == 2 && (!strcmp(verb, "use") || !strcmp(verb, "reconcile") || !strcmp(verb, "run"));
    int create = !strcmp(verb, "new") && count <= 2;
    if (single || named || create) chat_action(directory, verb, count == 2 ? words[1] : NULL, NULL, selection);
    else fputs("libcwr: unknown slash command or arguments; use /help\n", stderr);
  }
  free(line);
  return result;
}
static int cli_command(int argc, char **argv, char *selection);
static int repl_main(const char *directory) {
  const size_t bound = CLI_TEXT_LIMIT + 4096u;
  char *line = malloc(bound + 1);
  if (!line) { fputs("libcwr: allocation failed\n", stderr); return 1; }
  char selected[65] = "";
  int interactive = isatty(STDIN_FILENO);
  puts("libcwr REPL. Type help for commands, exit to leave.\n"
       "Start with init, then conversation create NAME; use NAME selects existing work.");
  int result = 0;
  for (;;) {
    if (interactive) {
      printf("libcwr[%s]> ", selected[0] ? selected : "-");
      if (fflush(stdout)) { result = 1; break; }
    }
    size_t n = 0;
    int c, oversized = 0, nul = 0;
    while ((c = fgetc(stdin)) != EOF && c != '\n') {
      if (!c) nul = 1;
      if (n < bound) line[n++] = (char)c;
      else oversized = 1;
    }
    if (ferror(stdin)) { fputs("libcwr: input read failed\n", stderr); result = 1; break; }
    if (c == EOF && !n && !oversized) break;
    line[n] = 0;
    if (oversized || nul) { fputs("libcwr: command exceeds bound or contains NUL\n", stderr); continue; }
    char *words[20];
    int count = split_line(line, words, 20);
    if (count < 0) { fputs("libcwr: unmatched quote, trailing escape, or too many arguments\n", stderr); continue; }
    if (!count) continue;
    if ((!strcmp(words[0], "exit") || !strcmp(words[0], "quit")) && count == 1) break;
    if (!strcmp(words[0], "use") && count == 2) {
      if (!select_conversation(directory, words[1])) {
        snprintf(selected, sizeof(selected), "%s", words[1]);
        printf("Selected conversation: %s\n", selected);
      }
      continue;
    }
    int forbidden = !strcmp(words[0], "repl") || !strcmp(words[0], "chat");
    for (int i = 0; i < count; i++) {
      /* The value after --text is literal, even if it resembles an option. */
      if (!strcmp(words[i], "--text") && i + 1 < count) { i++; continue; }
      if (!strcmp(words[i], "--directory") || !strncmp(words[i], "--directory=", 12)) forbidden = 1;
    }
    if (forbidden) { fputs("libcwr: workspace is fixed for this session; nested repl is unavailable\n", stderr); continue; }
    char *args[26];
    int argc = 0;
    args[argc++] = "libcwr";
    args[argc++] = words[0];
    int shorthand = !strcmp(words[0], "submit") && count >= 2 &&
                    (count == 2 || words[2][0] == '-');
    int history = !strcmp(words[0], "history") && (count == 1 || words[1][0] == '-');
    if (shorthand || history) {
      if (!selected[0]) { fputs("libcwr: select a conversation with use NAME or conversation create NAME\n", stderr); continue; }
      args[argc++] = selected;
    }
    for (int i = 1; i < count; i++) args[argc++] = words[i];
    args[argc++] = "--directory";
    args[argc++] = (char *)directory;
    cli_command(argc, args, selected);
    if (!strcmp(words[0], "artifact")) putchar('\n');
    if (fflush(stdout) || ferror(stdout)) { result = 1; break; }
  }
  free(line);
  return result;
}
static int cli_command(int argc, char **argv, char *selection) {
  if (argc == 1 && isatty(STDIN_FILENO)) return workspace_main(".libcwr", NULL, NULL);
  jmp_buf top;
  jmp_buf *previous = v_handler;
  volatile int status = 1;
  v_handler = &top;
  if (setjmp(top)) {
    fprintf(stderr, "libcwr: %s\n", v_error);
    host_close();
    v_handler = previous;
    return status;
  }
  const char *directory = ".libcwr";
  const char *text = NULL, *source = NULL, *compiler = NULL;
  const char *words[8];
  size_t count = 0;
  int directory_set = 0, json = 0, show_help = 0;
  for (int i = 1; i < argc; i++) {
    const char *arg = argv[i];
    if (!strcmp(arg, "--directory")) {
      if (directory_set || i + 1 >= argc || !strncmp(argv[i + 1], "--", 2)) {
        status = 2;
        v_fail("--directory requires one directory value");
      }
      directory = argv[++i];
      directory_set = 1;
    } else if (!strncmp(arg, "--directory=", 12)) {
      if (directory_set || !arg[12]) {
        status = 2;
        v_fail("--directory requires one directory value");
      }
      directory = arg + 12;
      directory_set = 1;
    } else if (!strcmp(arg, "--source") || !strcmp(arg, "--rbel")) {
      const char **value = !strcmp(arg, "--source") ? &source : &compiler;
      if (*value || i + 1 >= argc || !strncmp(argv[i + 1], "--", 2)) {
        status = 2; v_fail("%s requires one path", arg);
      }
      *value = argv[++i];
    } else if (!strcmp(arg, "--text")) {
      if (text || i + 1 >= argc) {
        status = 2;
        v_fail("--text requires one text value");
      }
      text = argv[++i];
    } else if (!strncmp(arg, "--text=", 7)) {
      if (text) {
        status = 2;
        v_fail("duplicate --text");
      }
      text = arg + 7;
    } else if (!strcmp(arg, "--json")) {
      if (json) {
        status = 2;
        v_fail("duplicate --json");
      }
      json = 1;
    } else if (!strcmp(arg, "--help")) {
      if (show_help) {
        status = 2;
        v_fail("duplicate --help");
      }
      show_help = 1;
    } else if (!strcmp(arg, "--version")) {
      if (count == sizeof(words) / sizeof(words[0])) {
        status = 2;
        v_fail("too many command arguments");
      }
      words[count++] = "version";
    } else if (arg[0] == '-') {
      status = 2;
      v_fail("unknown option %s", arg);
    } else {
      if (count == sizeof(words) / sizeof(words[0])) {
        status = 2;
        v_fail("too many command arguments");
      }
      words[count++] = arg;
    }
  }
  if (!count || show_help || (count == 1 && !strcmp(words[0], "help"))) {
    help();
    host_close();
    v_handler = previous;
    return 0;
  }
  const char *command = words[0];
  if (!strcmp(command, "workspace")) {
    if (count != 1 || text || json || (!!source != !!compiler)) {
      status = 2; v_fail("workspace accepts --directory and paired --source SOURCE --rbel COMPILER");
    }
    v_handler = previous;
    return workspace_main(directory, source, compiler);
  }
  if (!strcmp(command, "chat")) {
    if (count != 1 || text || json || (!!source != !!compiler)) {
      status = 2; v_fail("chat accepts --directory and paired --source SOURCE --rbel COMPILER");
    }
    v_handler = previous;
    host_prepare_signals();
    if (source && coworker_configure(source, compiler)) { coworker_close(); return 1; }
    int result = chat_main(directory);
    coworker_close();
    return result;
  }
  if (source || compiler) { status = 2; v_fail("--source and --rbel are workspace/chat options only"); }
  if (!strcmp(command, "repl")) {
    if (count != 1 || text || json) {
      status = 2;
      v_fail("repl accepts only --directory and --help");
    }
    v_handler = previous;
    return repl_main(directory);
  }
  int is_version = !strcmp(command, "version") && count == 1;
  int is_init = !strcmp(command, "init") && count == 1;
  int is_create = !strcmp(command, "conversation") && count == 3 && !strcmp(words[1], "create");
  int is_submit = !strcmp(command, "submit") && count == 3 && text;
  int is_list = !strcmp(command, "list") && count == 2 &&
                (!strcmp(words[1], "conversations") || !strcmp(words[1], "invocations"));
  int is_single = count == 2 && (!strcmp(command, "run") || !strcmp(command, "show") ||
                  !strcmp(command, "history") || !strcmp(command, "artifact") || !strcmp(command, "reconcile"));
  int is_overview = count == 1 && (!strcmp(command, "status") || !strcmp(command, "recover"));
  if (!(is_version || is_init || is_create || is_submit || is_list || is_single || is_overview) ||
      (text && !is_submit)) {
    status = 2;
    v_fail("invalid command; run libcwr --help for usage");
  }
  if (is_create) {
    valid_name(words[2]);
  } else if (is_submit) {
    valid_name(words[1]);
    valid_name(words[2]);
    if (strlen(text) > CLI_TEXT_LIMIT) {
      v_fail("text exceeds 256 KiB command bound");
    }
  } else if (is_single) {
    valid_name(words[1]);
  }
  V *value = NULL;
  const char *code = "ok";
  V *response = NULL;
  if (is_version) {
    value = v_object();
    v_put(value, "revision", v_string(LIBCWR_BUILD_REVISION));
    v_put(value, "core_sha256", v_string(LIBCWR_CORE_SHA256));
  } else if (is_init) {
    value = host_init(directory);
    code = "initialized";
  } else {
    host_open(directory, NULL);
    if (is_create) {
      V *request = event("create");
      field(request, "conversation", reference("conversations", words[2]));
      field(request, "owner", v_record_get(host_environment(), "context"));
      response = host_event(request);
      command = "conversation create";
    } else if (is_submit) {
      V *request = event("admit");
      field(request, "ref", reference("invocations", words[2]));
      field(request, "origin", reference("requests", words[2]));
      field(request, "conversation", reference("conversations", words[1]));
      field(request, "message", reference("messages", words[2]));
      field(request, "target", v_record_get(v_record_get(host_environment(), "binding"), "target"));
      field(request, "input", v_chars(text));
      field(request, "text", v_chars(text));
      response = host_event(request);
    } else if (is_list) {
      int invocations = !strcmp(words[1], "invocations");
      value = visible_records(invocations ? "invocation" : "conversation", words[1]);
      command = invocations ? "list invocations" : "list conversations";
    } else if (is_overview) {
      value = overview();
      if (!strcmp(command, "recover")) {
        code = "recovered";
      }
    } else if (!strcmp(command, "show")) {
      response = read_record("invocation", reference("invocations", words[1]));
    } else if (!strcmp(command, "history")) {
      response = read_record("conversation", reference("conversations", words[1]));
      if (v_isstr(at(response, 0), "found")) {
        V *messages = v_record_get(at(response, 1), "messages");
        V *ordered = v_array();
        for (size_t i = messages ? messages->n : 0; i > 0; i--) {
          v_push(ordered, messages->items[i - 1]);
        }
        value = ordered;
        response = NULL;
        code = "found";
      }
    } else if (!strcmp(command, "artifact")) {
      value = host_artifact(reference("invocations", words[1]));
      code = "found";
    } else {
      V *request = event(command);
      field(request, "ref", reference("invocations", words[1]));
      response = host_event(request);
    }
  }
  if (response) {
    V *response_code = at(response, 0);
    if (!response_code || response_code->type != VT_STRING) {
      v_fail("invalid protocol response");
    }
    code = response_code->s;
    value = at(response, 1);
  }
  output(command, code, value, json);
  if (fflush(stdout) || ferror(stdout)) {
    v_fail("command output failed");
  }
  status = successful(code) ? 0 : 1;
  if (selection && is_create && (!strcmp(code, "created") || !strcmp(code, "existing"))) {
    snprintf(selection, 65, "%s", words[2]);
  }
  host_close();
  v_handler = previous;
  return status;
}

int cli_main(int argc, char **argv) { return cli_command(argc, argv, NULL); }
