# Phase 5 Cross-Language Parity Fixture

This fixture is the contract for Python/Node parity in #124. It is not a
Python-backend-vs-Rust-backend duplicate test corpus. Python user-space parity
continues to live in the existing integration suite under
`TYPE_BRIDGE_BACKEND=python|rust`.

The files here define one shared schema, write data, descriptor snapshots, and
expected canonical JSON for cross-language tests:

- `schema.tql`: TypeDB schema loaded into a fresh database.
- `metadata.json`: fixture identity, covered features, and canonical encoding
  rules.
- `descriptors.json`: canonical Rust descriptor shapes expected from both
  Python model metadata and Node descriptor construction.
- `write-data.json`: language-neutral rows each writer must insert.
- `expected-canonical.json`: sorted result rows each reader must emit after
  canonicalization.

Canonical encoding rules:

- `long` values are decimal strings in snapshots so JavaScript never loses
  `i64` precision.
- The TypeQL schema uses TypeDB's `integer` value type for these fields while
  descriptor and row snapshots keep the shared runtime's canonical `long`
  spelling.
- `double` values are JSON numbers.
- `decimal` and `duration` values are strings.
- `date`, `datetime`, and `datetime-tz` values are ISO 8601 strings.
- Optional absent attributes are omitted from the `attributes` object.
- Repeated attributes and repeated role players are sorted by value or
  `stable_id` before comparison.
- TypeDB IIDs are excluded from fixture equality; `stable_id` is the portable
  row identity.
