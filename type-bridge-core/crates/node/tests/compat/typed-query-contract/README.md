# Public typed Query contract

These compile-only consumers pin #171's TypeScript row-shape contract against
the real `@type-bridge/node/typed` source and packed public facade.

The fixtures import real model factories, connection types, owner-aware
references, page shapes, and the immutable Query facade. The packed-package
baseline in `../legacy-package/typed-consumer.ts` independently proves the same
subpath from an extracted npm tarball.

`documented-examples.typecheck.ts` is the ordered concatenation of all five
TypeScript blocks marked in `docs/development/typed-query-contract.md`. The
shared corpus test rejects source drift, and this directory's `tsc` project
compiles it with the other public contract consumers.

Run with:

```bash
npm run typecheck:query-contract
```
