# Workforce v1 conformance checkpoint

This directory defines the first executable, target-neutral checkpoint for the
FULL-SDK capability manifest. It deliberately covers only six capabilities and
nine runtime proof rows. It is not the complete workforce journey and it does
not promote any binding or capability status.

`catalog-v1.json` maps every manifest case to one disposition. Binding status
remains owned exclusively by `../manifest-v1.json`. `journey-v1.json` supplies
the shared generated-model input and the exact stable observations expected
from Python, Node, and Rust. `report-schema-v1.json` documents the closed report
wire accepted by `scripts/ci/compare_workforce_conformance.py`.

The catalog also owns the exact semantic fingerprint and the Python,
TypeScript/Node, and Rust projection fingerprints for this fixture. Reports
must carry those identities; merely agreeing with one another is insufficient.

Reports are evidence records, not model serialization. Producers may translate
language-native generated values into the committed observation objects only
after asserting the corresponding public operation succeeded. Reports must not
contain provider IIDs, database names, addresses, ports, timestamps, random
nonces, or binding-specific display text.

When `TYPE_BRIDGE_WORKFORCE_REPORT` is present, producers require an absolute
UTF-8 path of at most 4096 bytes, an existing non-symlink parent directory, and
an absent destination. They publish compact, key-sorted JSON atomically from a
temporary file in that parent only after all assertions and cleanup succeed.
When the variable is absent, the existing live suite writes no report.

The comparator consumes exactly one report from each current generated SDK:

```bash
python scripts/ci/compare_workforce_conformance.py \
  tmp/workforce/python.json \
  tmp/workforce/node.json \
  tmp/workforce/rust.json
```

Each input must be a regular non-symlink file no larger than 1 MiB, encoded in
the canonical report spelling described above. Input order is irrelevant;
binding identity is carried inside the report.

The successful summary lists the shared runtime proofs, current manifest gaps,
and every required proof not covered by this checkpoint. Uniform gaps and
uncovered proofs are never converted to accepted evidence.
