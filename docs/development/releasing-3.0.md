# Releasing 3.0

This is the maintainer procedure for the 3.0.0 release. Local preparation
leaves the changelog undated and does not publish anything. Run the outward
steps only after release authorization. Keep the source SHA, workflow run IDs,
artifact hashes and public-consumer reports together as release evidence.

## Integrate and accept the release source

1. Review the release branch, including the merge of master's 2.3.3 publication
   record. Push the reviewed branch and open a PR targeting `master`:

   ```bash
   git push origin v3
   gh pr create --base master --head v3 --title 'Release TypeBridge 3.0.0' --body-file release-pr.md
   ```

   Write `release-pr.md` with the reviewed changes and validation first. Merge
   through the PR after required checks and review; master uses PRs and branch
   protection may reject direct pushes. Preserve the reviewed release history.
2. Immediately before the tagging phase, change `3.0.0 - Unreleased` in
   `CHANGELOG.md` to the actual publication date in a final PR. Merge that PR
   before accepting the final source. Do not date the changelog in a new commit
   after acceptance: every source-bound gate must cover the exact dated SHA.
3. Require complete attempt-one master CI on that SHA:

   ```bash
   gh run list --workflow ci.yml --commit "$RELEASE_SHA"
   gh run watch "$CI_RUN_ID" --exit-status
   gh workflow run release.yml --ref master -f release_channel=candidate
   gh run watch "$CANDIDATE_RUN_ID" --exit-status
   gh workflow run c-release.yml --ref master -f mode=verify -f ci_run_id="$CI_RUN_ID"
   gh run watch "$C_VERIFY_RUN_ID" --exit-status
   ```

   Set each run ID from the newly created run, and verify its head SHA equals
   `RELEASE_SHA`. Candidate acceptance covers the full Python/native npm
   platform matrix, Cargo, multi-platform OCI and live artifact consumers.
   C verification consumes same-source master artifacts and must produce
   `c-verified-release-3.0.0`, including all SDK suites and FULL-C evidence.
   Partial reruns do not establish acceptance. If master moves, explicitly
   verify the dispatched source or repeat acceptance on the intended SHA.

## Tag and publish

Recheck the changelog date immediately before tagging. If the date needs to
change, repeat the final PR and same-source acceptance above. Create the tag
only on the accepted dated commit:

```bash
git tag -a v3.0.0 "$RELEASE_SHA" -m 'TypeBridge 3.0.0'
git show v3.0.0
git push origin v3.0.0
```

Never move an existing tag. The tag push invokes stable `release.yml`.
**The workflow dependency order is authoritative:** npm, then crates.io and
GHCR, then core PyPI, then facade PyPI, then the GitHub draft, then C promotion.
Cargo follows the inventory's dependency order. Wait for every required job;
do not publish a later registry manually to bypass a failed predecessor.

After the ordinary draft exists, promote the exact verified C bytes:

```bash
gh workflow run c-release.yml --ref v3.0.0 -f mode=publish -f verify_run_id="$C_VERIFY_RUN_ID"
gh run watch "$C_PROMOTION_RUN_ID" --exit-status
```

Require twelve absent-or-identical assets and six verified Sigstore bundles.
Inspect the ordinary and C draft inventory, provenance, checksums and release
notes before making the draft public. Record the new 3.0.0 OCI digest in the
release notes; `3.0`, `3`, and `latest` alias that accepted manifest.

## Independently consume public packages

Candidate workflow downloads are not public-registry evidence. The
`accept-live-artifact-parity` job in `release.yml` supplies the Python/Node
consumer tests, but runs candidate bytes on 3.12.3 only. The commands below
reuse those tests with public PyPI/npm bytes across all three measured servers.
They require the tagged checkout, its documented developer tools, three
already-running servers, and assigned database names and proxy ports.

Use a new disk-backed evidence directory outside the checkout. Set
`PUBLIC_CHECK_ROOT` to that directory, `RELEASE_RUN_ID` to the successful stable
release run, and `PUBLIC_SERVER_DIGEST` to the verified published OCI digest.
Retain downloads and compare their SHA-256 values with the accepted release
records before running consumers. Never substitute a local wheel or npm build.

```bash
export GITHUB_WORKSPACE="$PWD"
export PUBLIC_CHECK_ROOT="$HOME/.cache/typebridge-public-3.0.0"
export RUNNER_TEMP="$PUBLIC_CHECK_ROOT"
export TMPDIR="$PUBLIC_CHECK_ROOT/tmp"
mkdir -p "$TMPDIR" "$PUBLIC_CHECK_ROOT/downloads"
findmnt -T "$TMPDIR"
uv venv --python 3.13 "$RUNNER_TEMP/release-generated-parity"
uv pip install --python "$RUNNER_TEMP/release-generated-parity/bin/python" \
  --index-url https://pypi.org/simple \
  'type-bridge==3.0.0' 'type-bridge-core==3.0.0' 'pytest>=9.0.1'
npm pack @type-bridge/node@3.0.0 --pack-destination "$PUBLIC_CHECK_ROOT/downloads"
npm install --ignore-scripts --no-audit --no-fund --no-package-lock --no-save \
  --prefix "$PUBLIC_CHECK_ROOT/node" "$PUBLIC_CHECK_ROOT/downloads/type-bridge-node-3.0.0.tgz"
export TYPE_BRIDGE_NODE_PACKAGE_ROOT="$PUBLIC_CHECK_ROOT/node/node_modules/@type-bridge/node"
gh run download "$RELEASE_RUN_ID" -n staged-smoke-server -D "$PUBLIC_CHECK_ROOT/server"
chmod +x "$PUBLIC_CHECK_ROOT/server/staged_smoke_server"
export TYPE_BRIDGE_SMOKE_SERVER="$PUBLIC_CHECK_ROOT/server/staged_smoke_server"
docker pull "ghcr.io/ds1sqe/type-bridge-server@$PUBLIC_SERVER_DIGEST"
gh release download v3.0.0 --dir "$PUBLIC_CHECK_ROOT/downloads"
```

The smoke server is test infrastructure from the accepted run, not a public
server-package check. Independently replay `accept-server-oci` with the public
OCI digest for server distribution acceptance. Verify C payload signatures
with the identities and procedure in [C release procedure](c-release.md).
The example below assumes the public archive filenames selected by
`.github/release/c-3.0.0.json`; locate exactly one of each, never a wildcard
that silently admits multiple versions.

Run the Python/Node workflow steps unchanged, including their import isolation
and exact two-test assertion. Fixture generation is test setup; installed
runtime modules must continue to resolve to public packages.

```bash
export USE_DOCKER=false
# Replace these names/ports with the resources assigned to this release run.
export TYPEDB_DATABASE=release_300_python
export TYPE_BRIDGE_NODE_INTG_DATABASE=release_300_node
export TB_STAGED_PROXY_PORT_RANGE=41550-41599
for entry in \
  '3.12.3 42796 42797 typedb-3.12.1/v1' \
  '3.13.6 43127 43129 typedb-3.13.6/v1' \
  '3.14.0 43130 43131 typedb-3.13.6/v1'; do
  read -r version grpc http profile <<< "$entry"
  export TYPEDB_VERSION="$version" TYPEDB_ADDRESS="127.0.0.1:$grpc" TYPEDB_HTTP_PORT="$http"
  export TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE="$profile"
  export TYPE_BRIDGE_GENERATED_PARITY_REPORT="$PUBLIC_CHECK_ROOT/python-$version.xml"
  export TYPE_BRIDGE_GENERATED_PYTHON_STAGE="$PUBLIC_CHECK_ROOT/python-$version"
  export TYPE_BRIDGE_GENERATED_NODE_STAGE="$PUBLIC_CHECK_ROOT/node-$version"
  bash scripts/ci/prepare_generated_live_fixture.sh python "$TYPE_BRIDGE_GENERATED_PYTHON_STAGE"
  bash scripts/ci/prepare_generated_live_fixture.sh node "$TYPE_BRIDGE_GENERATED_NODE_STAGE"
  uv run python - <<'PY'
import os
import subprocess
from pathlib import Path
import yaml
root = Path(os.environ['GITHUB_WORKSPACE'])
workflow = yaml.safe_load((root / '.github/workflows/release.yml').read_text())
selected = {
    'Run exact-wheel generated Python application',
    'Require both generated Python journeys',
    'Run exact-package generated Node application',
}
steps = [s for s in workflow['jobs']['accept-live-artifact-parity']['steps']
         if s.get('name') in selected]
assert len(steps) == len(selected)
for step in steps:
    # Keep the matrix environment above; the workflow step defaults are 3.12.3.
    subprocess.run(['bash', '-euo', 'pipefail', '-c', step['run']],
                   cwd=root / step.get('working-directory', '.'), check=True)
PY
 done
```

Require both Python cases with zero skips and successful Node application
journeys on each server. Check exact-name database cleanup after each run;
remove only databases created by that run. Keep stdout, JUnit and package hashes.

For Rust, independently download the public crate versions using the release
inventory and run the existing external-consumer check. The validator extracts those downloaded bytes into an isolated consumer;
no dependency may point into the checkout:

```bash
mkdir -p "$PUBLIC_CHECK_ROOT/crates"
uv run python - <<'PY'
import os
import sys
import urllib.request
from pathlib import Path
sys.path.insert(0, 'scripts/ci')
from cargo_release_inventory import load_inventory
out = Path(os.environ['PUBLIC_CHECK_ROOT']) / 'crates'
for package in load_inventory().public_packages:
    url = f'https://crates.io/api/v1/crates/{package.name}/{package.version}/download'
    urllib.request.urlretrieve(url, out / f'{package.name}-{package.version}.crate')
PY
python scripts/ci/validate_cargo_external_consumers.py \
  --artifacts-dir "$PUBLIC_CHECK_ROOT/crates" --expected-release-version 3.0.0
```

This compile/consumer check does not substitute for the four-binding live
conformance record. Retain the same-source queries/models reports on 3.12.3,
3.13.6 and 3.14.0 alongside these independent public-distribution checks.
For C, run `c_artifact_journey.py provider-free` with the downloaded CLI,
runtime and generated archives, and `connected` on each measured server:

```bash
# Validators use the manifest's original candidate basename. Copy public bytes
# under that name without repacking; retain the public downloads and signatures.
uv run python - <<'PYCODE'
import json
import os
import shutil
from pathlib import Path
policy = json.loads(Path('.github/release/c-3.0.0.json').read_text())
root = Path(os.environ['PUBLIC_CHECK_ROOT'])
(root / 'c-inputs').mkdir()
for original, public in policy['public_files'].items():
    shutil.copyfile(root / 'downloads' / public, root / 'c-inputs' / original)
PYCODE
mapfile -t runtimes < <(find "$PUBLIC_CHECK_ROOT/c-inputs" -maxdepth 1 -name 'type-bridge-c-runtime-abi-2.0-*.tar.gz')
mapfile -t generated < <(find "$PUBLIC_CHECK_ROOT/c-inputs" -maxdepth 1 -name 'tb_sdk-c-*.tar.gz')
mapfile -t cli < <(find "$PUBLIC_CHECK_ROOT/c-inputs" -maxdepth 1 -name 'type-bridge-cli-artifact-*.tar.gz')
test "${#runtimes[@]}" -eq 1 && test "${#generated[@]}" -eq 1 && test "${#cli[@]}" -eq 1
python scripts/ci/c_artifact_journey.py provider-free \
  "${cli[0]}" "${runtimes[0]}" "${generated[0]}" > "$PUBLIC_CHECK_ROOT/c-provider-free.json"
for entry in \
  '3.12.3 42796 42797 typedb-3.12.1/v1' \
  '3.13.6 43127 43129 typedb-3.13.6/v1' \
  '3.14.0 43130 43131 typedb-3.13.6/v1'; do
  read -r version grpc http profile <<< "$entry"
  TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE="$profile" \
  TYPE_BRIDGE_EXPECTED_SERVER_VERSION="$version" \
    python scripts/ci/c_artifact_journey.py connected "${runtimes[0]}" "${generated[0]}" \
      --address "127.0.0.1:$grpc" --http-port "$http" \
      --database "release_300_c_${version//./_}" --remote-port 41550 \
      > "$PUBLIC_CHECK_ROOT/c-$version.json"
done
```

Complete the TLS, migration, relocation, C17/C++17 and full application
journeys required by [C release procedure](c-release.md) using the same public
archives. Connected migrations retain their exact 3.12.3 requirement.
Do not claim FULL C support from the shorter connected probe alone.

After publication, inspect the release-triggered Perf workflow and compare
retained metrics with the prior release. Retired compiler metric IDs remain
historical; never insert zeroes or remap them to new metrics.
