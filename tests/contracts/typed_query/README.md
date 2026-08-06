# Typed Query Semantic Contract

This directory is the language-neutral #170/#171 semantic corpus. The corpus
is a behavior manifest rather than a serialized query plan, and it is never
packaged.

- `schema-v1.json` defines the versioned fixture vocabulary and required
  coverage.
- `corpus-v1.json` records semantic cases and paired documentation examples.
- `expected-results-v1.json` pins selected-tuple, root, and collection identity
  outcomes for one shared solution set.

`type-bridge-core/crates/orm/tests/typed_query_semantic_corpus.rs` loads every
solution and expected value from `expected-results-v1.json`. Its recording
backend translates only the fixture's logical identities to provider IIDs;
the production Rust selected-result executor performs tuple/root distinctness,
page windows, collection multiplicity, hydration, count, and existence.

The ten marked documentation blocks are concatenated without semantic edits
into `python/documented_examples.py` and
`typescript/documented_examples.ts`. The contract test checks source parity;
the schema-codegen acceptance runners emit the canonical fixture package and
then compile both complete examples with Pyright and `tsc`.

Python and TypeScript bindings must consume the same Rust semantics. A
language-specific test may render or type a case, but it must not change the
expected identity, multiplicity, error, transaction, or statement behavior.
