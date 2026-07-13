# Legacy packed-package compatibility fixture

`run.cjs` verifies the current-major root package from the artifact that would
actually be published. It does not run `npm install` or `npm ci`.

After the ordinary Node build has produced `dist/` and the platform `.node`
artifact, the runner:

1. creates a tarball with `npm pack --ignore-scripts`, or accepts one exact
   prebuilt `.tgz` with `--artifact`;
2. extracts that tarball into an isolated consumer's
   `node_modules/@type-bridge/node` directory;
3. compiles `consumer.ts` against only the packed declarations;
4. runs the legacy `TypedQuery<T, Row>` recording-manager fixture;
5. compiles and runs an exact-inference `@type-bridge/node/typed` consumer
   without opening a database; and
6. loads the packed native module, verifies opaque result symbols, and rejects
   source-tree module leakage.

The release workflow uses prebuilt mode so the consumer accepts the same
tarball later passed to `npm publish`:

```bash
npm run smoke:legacy-package -- --artifact /path/to/type-bridge-node.tgz
```

Release matrices can instead pass a directory containing exactly one tarball:

```bash
npm run smoke:legacy-package -- --artifact-directory /path/to/release-artifact
```

Prebuilt mode never packs, builds, or installs the package. The caller must
prepare the local TypeScript compiler used to compile the isolated consumer.

Set `TYPE_BRIDGE_KEEP_COMPAT_TMP=1` to retain the ignored `tmp/` consumer for
diagnosis.
