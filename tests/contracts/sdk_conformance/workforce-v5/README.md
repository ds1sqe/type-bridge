# Workforce conformance v5

This additive Plan07 contract owns canonical generated-model record and archive
evidence for Python, Node, generated Rust, and internal C. It does not change
Workforce V1-V4 and does not make C supported or publishable.

The Phase-0 catalog is deliberately `phase0_unfinalized`. Its five selected
rows have two disjoint dispositions:

- G07 is the only exact candidate manifest transition.
- G05, G06, G08, and G13 are serialization/archive evidence only and remain
  manifest gaps until Plan08.

The canonical input fixture is the preserved Workforce V3 schema plus
`journey-v5.json`. The journey fixes semantic inputs and required relationships
but contains no implementation-authored canonical record, archive, fingerprint,
or report output. Each public generated producer must compute its bytes first.

The frozen wire and ABI decisions are
`tests/contracts/projected-record-v1.json` and
`tests/contracts/c-abi-1-6-candidate.json`.
