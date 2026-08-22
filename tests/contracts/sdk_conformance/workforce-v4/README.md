# Workforce conformance v4

This additive contract owns Plan06 administration and migration evidence for
Python, Node, generated Rust, and internal C. It does not change the preserved
Workforce V1–V3 wires and does not make C supported or publishable.

The Phase-0 catalog is deliberately `phase0_unfinalized`. Finalization requires
real source-bound producers, exact TypeDB 3.12.3 observations, frozen catalog
and journey digests, and a passing fail-closed comparator. Expected journey
data may be compared only after observation and may never seed a report.
The journey's `shared_fixture_oracles` are independently reproduced by the
exact-live TypeDB runner/provider tests. Binding producers must compute their
own public-facade observations before comparing them with these values; the
oracle objects are never copied into producer output.

The eight selected rows have two disjoint dispositions:

- G04, G09, G10, and G11 are the exact manifest transition candidates.
- G05, G06, G08, and G13 are administration/migration evidence only and must
  remain manifest gaps until Plan08 evaluates their complete cross-slice proof.

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
