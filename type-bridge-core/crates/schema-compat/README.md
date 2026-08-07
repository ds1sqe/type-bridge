# type-bridge-schema-compat

This release-lockstep supporting crate compares the frozen V1 schema lane with
the Rust-SSOT V2 lane. A matching report is evidence about their shared domain,
not evidence that V1 can represent the complete V2 contract. Most applications
should depend on [`type-bridge`](https://crates.io/crates/type-bridge) instead.

```toml
[dependencies]
type-bridge-schema-compat = "2.1.0"
```

Integrators should start with the released-syntax parsers and shadow-report
types in the [crate API](https://docs.rs/type-bridge-schema-compat/2.1.0). Treat
a matching shadow report only as evidence for the explicitly shared domain.

This crate is released in lockstep with TypeBridge 2.1.0, requires Rust 1.88+,
and evaluates schemas against the TypeDB 3.12.1 V2 semantic baseline. The wider
runtime supports TypeDB 3.11.x–3.12.x.

## Shadow policy

## Corpus completion criterion

The schema shadow corpus is complete enough for a cutover decision only when
all of the following are true:

1. Both effective lanes accept every fixture in the declared overlap corpus.
2. Every comparison has `ShadowVerdict::Matched` and no findings.
3. `ShadowCoverage::unimplemented()` is empty.
4. `ShadowCoverage::not_representable()` is exactly the frozen set below.
5. Every non-representable dimension has independent V2 acceptance coverage.

Any additional blind spot blocks cutover. A rejection in both lanes is not a
match and cannot satisfy this criterion.

The frozen V1-inexpressible set is:

- `FunctionBodiesAndAnnotations`
- `StructFields`
- `SourceCommentsAndSpans`
- `OmittedVersusExplicitIdentity`
- `IndependentAnnotationIdentityAndRemoval`
- `SubAnnotations`
- `ExtensionsAndCapabilities`
- `ResolverGraphsAndOrigins`
- `CardinalityOutsideV1U32`

Changing this set requires evidence that the frozen V1 representation actually
gained or lost the relevant information. It must not be changed merely to make
`is_cutover_evidence()` return true.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-schema-compat/2.1.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
