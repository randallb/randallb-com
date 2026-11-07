/* Source imports are orchestration directives, never runtime expressions. */
static size_t import_space(const char *s, size_t n, size_t p) {
  while (p < n) {
    if (isspace((unsigned char)s[p])) { ++p; continue; }
    if (s[p] == ';') { while (p < n && s[p] != '\n') ++p; continue; }
    break;
  }
  return p;
}
static void import_path(const char *file, const char *s, size_t n, size_t *position) {
  size_t p = import_space(s, n, *position); Buffer path = {0};
  if (p == n || s[p++] != '"') fail("%s: import requires one quoted relative path", file);
  int closed = 0;
  while (p < n) {
    char c = s[p++];
    if (c == '"') { closed = 1; break; }
    if (c == '\\') { if (p == n) break; c = s[p++]; }
    if ((unsigned char)c < 32 || c == 127) fail("%s: invalid import path", file);
    append(&path, &c, 1);
  }
  p = import_space(s, n, p);
  if (!closed || p == n || s[p++] != ')') fail("%s: import requires one quoted relative path", file);
  if (!path.size || path.data[0] == '/' || path.data[0] == '~' || strchr(path.data, ':'))
    fail("%s: import requires a relative path (no absolute paths or refs)", file);
  if (!discovering) { free(path.data); *position = p; return; }
  if (extracting->import_count == MAX_FILES) fail("%s: too many imports", file);
  const char *slash = strrchr(file, '/');
  extracting->imports[extracting->import_count++] = formatted("%.*s/%s", (int)(slash - file), file, path.data);
  free(path.data); *position = p;
}
static void import_definitions(const char *file, const char *s, size_t n) {
  size_t p = 0, kept = 0; int depth = 0, quoted = 0;
  while (p < n) {
    char c = s[p];
    if (c == ';') { while (p < n && s[p] != '\n') ++p; continue; }
    if (isspace((unsigned char)c)) { ++p; continue; }
    if (!depth && !quoted && c == '(') {
      size_t word = import_space(s, n, p + 1), end = word;
      while (end < n && !isspace((unsigned char)s[end]) && s[end] != ')' && s[end] != '(' && s[end] != ';' && s[end] != '"' && s[end] != '\\' && s[end] != '\'') ++end;
      if (end - word == 6 && !memcmp(s + word, "import", 6)) {
        append(&definitions, s + kept, p - kept);
        p = end; import_path(file, s, n, &p); kept = p;
        text(&definitions, "\n"); continue;
      }
    }
    if (c == '\'') { quoted = 1; ++p; continue; }
    if (c == '"') {
      ++p;
      while (p < n) { char q = s[p++]; if (q == '\\' && p < n) ++p; else if (q == '"') break; }
      quoted = 0; continue;
    }
    if (c == '\\') {
      ++p; if (p < n) ++p;
      while (p < n && isalpha((unsigned char)s[p])) ++p;
      quoted = 0; continue;
    }
    if (c == '(') { ++depth; quoted = 0; ++p; }
    else if (c == ')') { --depth; quoted = 0; ++p; }
    else {
      /* Consume a complete atom. Adjacent list/string forms are separate. */
      do { ++p; } while (p < n && !isspace((unsigned char)s[p]) &&
        !strchr("();\"\\'", s[p]));
      quoted = 0;
    }
  }
  append(&definitions, s + kept, n - kept); text(&definitions, "\n");
}
