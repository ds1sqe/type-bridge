# Schema V2 contract corpus

This corpus freezes the source-document boundary for Schema V2.

- `valid/` contains lossless YAML inputs whose exact UTF-8 source, comments,
  scalar styles, and spans must remain available to diagnostics and editors.
- `invalid/yaml/` contains YAML features rejected before schema normalization.

The accepted grammar has one mapping-root document with string keys. Anchors,
aliases, tags, directives, merge keys, duplicate keys, multiple documents,
non-string keys, and ambiguous YAML 1.1-style plain strings fail closed.
