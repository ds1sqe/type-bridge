# Pre-renderer workspace

The two canonical manifests and TypeQL previews were authored with the CLI at
26267be2. The initial migration includes a regex; the second includes a backfill.
The normal CLI test copies this workspace and proves that preview regeneration
changes neither manifest bytes nor their embedded plan bytes.

To reproduce the frozen artifacts, build the CLI from 26267be2, set `BASE_CLI`
to that executable, then run from the current repository root with a disk-backed
`TMPDIR` outside the checkout:

```sh
fixture=type-bridge-core/crates/cli/tests/fixtures/legacy-previews
scratch=$(mktemp -d "$TMPDIR/typebridge-legacy-previews.XXXXXXXX")
cp "$fixture/typebridge.yaml" "$scratch/typebridge.yaml"
cp -r "$fixture/schema" "$scratch/schema"
"$BASE_CLI" --manifest "$scratch/typebridge.yaml" migration make --name initial
cp "$fixture/migrations/v2/copy-name.backfill.yaml" "$scratch/migrations/v2/"
"$BASE_CLI" --manifest "$scratch/typebridge.yaml" migration make --name copy-name \
  --backfill-intent copy-name.backfill.yaml
cp "$scratch"/migrations/v2/*.tbmigration.json "$fixture/migrations/v2/"
cp "$scratch"/migrations/v2/*.typeql "$fixture/migrations/v2/"
```

Do not regenerate this compatibility fixture with the current renderer.
