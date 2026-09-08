# Sdk conformance v4

This additive contract owns Plan06 administration and migration evidence for
Python, Node, generated Rust, and internal C. It does not change the preserved
Sdk V1–V3 wires and does not make C supported or publishable.

The catalog is `finalized` by the acceptance checks. Four real
source-bound producers independently passed the artifact fan-in against one
isolated exact TypeDB 3.12.3 service before G04/G09/G10/G11 were promoted; the
same final comparator requires an empty pending-promotion list. Expected journey
data may be compared only after observation and may never seed a report.
The journey's `shared_fixture_oracles` are independently reproduced by the
exact-live TypeDB runner/provider tests. Binding producers must compute their
own public-facade observations before comparing them with these values; the
oracle objects are never copied into producer output.

The eight selected rows have two disjoint dispositions:

- G04, G09, G10, and G11 are the exact accepted manifest transitions.
- G05, G06, G08, and G13 are administration/migration evidence only and must
  remain manifest gaps until C distribution evaluates their complete cross-slice proof.

Persisted migration authority retains semantic profile `typedb-3.12.1/v1`;
the isolated live acceptance server is exactly TypeDB 3.12.3.

`workspace/` is the canonical V4 source workspace and immutable migration
fixture. Its four manifests were produced in order through the source-tree CLI:

1. `0001_initial` establishes keyed `person` rows with `legacy-name`.
2. `0002_expand-display-name` additively introduces `display-name`.
3. `0003_backfill-display-name` copies values through the retained closed YAML
   intent with one keyed row per transaction group.
4. `0004_contract-legacy-name` removes the legacy ownership and attribute.

The final Split-YAML source is the contracted head. The backfill intent remains
beside the manifests for review and deterministic regeneration; canonical
manifest JSON is the executable history authority.

`schema generate` projects the fixture into Python, TypeScript/Node, Rust, and
internal C packages. Every package embeds byte-identical canonical history at
`typebridge/migration-history.json`; the frozen resource is 48,758 bytes with
SHA-256 `54a97d4d43f81bf1a7af65f0683bab571dafaa0efc6b8a1fe2e7dc0a1914000a`.
The replay-verified catalog fingerprint observed through generated public
facades is `efa3249f25fb2decaa0fef24c99d3a9884942d2e7926b3545955eb21eee7accb`.
