/* Small JSON output helpers. All parsed input comes from SDK JSON encoders. */
#ifndef WALKTHROUGH_JSON_H
#define WALKTHROUGH_JSON_H
#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void json_string_n(const char *s, size_t n) {
  putchar('"');
  for (size_t i = 0; i < n; ++i) {
    unsigned char c = (unsigned char)s[i];
    if (c == '"' || c == '\\') {
      putchar('\\');
      putchar(c);
    } else if (c < 32)
      printf("\\u%04x", c);
    else
      putchar(c);
  }
  putchar('"');
}
static void json_string(const char *s) {
  if (s == NULL)
    fputs("null", stdout);
  else
    json_string_n(s, strlen(s));
}
static const char *json_space(const char *s) {
  while (isspace((unsigned char)*s))
    ++s;
  return s;
}
static const char *json_end(const char *s) {
  s = json_space(s);
  if (*s == '"') {
    for (++s; *s && *s != '"'; ++s)
      if (*s == '\\' && s[1])
        ++s;
    return *s ? s + 1 : s;
  }
  if (*s == '{' || *s == '[') {
    char close = *s++ == '{' ? '}' : ']';
    while (*s && *s != close) {
      s = json_space(s);
      if (*s == ',' || *s == ':')
        ++s;
      else if (*s != close)
        s = json_end(s);
    }
    return *s ? s + 1 : s;
  }
  while (*s && !strchr(",}:] \n\r\t", *s))
    ++s;
  return s;
}
static const char *json_member(const char *s, const char *key) {
  s = json_space(s);
  if (*s++ != '{')
    return NULL;
  for (;;) {
    s = json_space(s);
    if (*s != '"')
      return NULL;
    const char *end = json_end(s);
    size_t length = (size_t)(end - s - 2);
    const char *value = json_space(end);
    if (*value++ != ':')
      return NULL;
    value = json_space(value);
    if (strlen(key) == length && memcmp(s + 1, key, length) == 0)
      return value;
    s = json_space(json_end(value));
    if (*s++ != ',')
      return NULL;
  }
}
static void json_value(const char *s) {
  if (s == NULL) {
    fputs("null", stdout);
    return;
  }
  fwrite(s, 1, (size_t)(json_end(s) - s), stdout);
}
static void json_without(const char *s, const char *key) {
  const char *value = json_member(s, key);
  if (value == NULL) {
    json_value(s);
    return;
  }
  const char *start = value;
  while (start > s && *start != ':')
    --start;
  --start;
  while (start > s && isspace((unsigned char)*start))
    --start;
  --start;
  while (start > s && *start != '"')
    --start;
  const char *end = json_space(json_end(value));
  if (*end == ',')
    ++end;
  else {
    while (start > s && isspace((unsigned char)start[-1]))
      --start;
    if (start[-1] == ',')
      --start;
  }
  fwrite(s, 1, (size_t)(start - s), stdout);
  fputs(end, stdout);
}
static void begin(const char *name, const char *step) {
  fputs("{\"case\":", stdout);
  json_string(name);
  fputs(",\"step\":", stdout);
  json_string(step);
  fputs(",\"value\":", stdout);
}
static void end(void) {
  puts("}");
  fflush(stdout);
}
#endif
