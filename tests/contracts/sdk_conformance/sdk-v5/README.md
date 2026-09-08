# Sdk conformance v5

This additive Record/archive contract owns canonical generated-model record and archive
evidence for Python, Node, generated Rust, and internal C. It does not change
Sdk V1-V4 and does not make C supported or publishable.

The finalized catalog has five selected rows with two disjoint dispositions:

- G07 is the only exact artifact manifest transition.
- G05, G06, G08, and G13 are serialization/archive evidence only and remain
  manifest gaps until C distribution.

The canonical input fixture is the preserved Sdk V3 schema plus
`journey-v5.json`. The journey fixes semantic inputs and required relationships
but contains no implementation-authored canonical record, archive, fingerprint,
or report output. Each public generated producer must compute its bytes first.

The frozen wire and ABI decisions are
`tests/contracts/projected-record-v1.json` and
`tests/contracts/c-abi.json`.
