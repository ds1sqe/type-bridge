# C release procedure

The C release path selects product version **2.3.2**, native ABI **1.7.0**,
and the `x86_64-unknown-linux-gnu` shared runtime built and consumed on
Ubuntu 24.04. C17 and C++17 applications use the installed runtime through
CMake or pkg-config. Public C support for this matrix was established with
2.2.0 on 2026-09-14. The 2.2.2 artifacts completed promotion and independent public
verification on 2026-09-14. The 2.2.3 artifacts completed promotion and
independent public verification on 2026-10-07. The 2.2.4 artifacts completed
promotion and independent public verification on 2026-10-11. The 2.2.5
artifacts completed promotion and independent public verification on
2026-10-15. The 2.2.6 artifacts completed promotion and independent public
verification on 2026-10-20.

The standalone CLI, native runtime and generated sdk example are
separate archives. Application owners generate and distribute their own
schema packages. Static libraries, macOS, Windows and other architectures
are outside this release's selected C distribution matrix.

Connected migration acceptance requires exactly TypeDB **3.12.3**. The
canonical schema and query semantic profile remains `typedb-3.12.1/v1`;
that profile identifier is not the connected server version requirement.

## Source and artifact acceptance

The committed policy in `.github/release/c-2.3.2.json` enumerates every CI
job and explicit step, including the permitted platform and failure-log
skips. It also binds the CI workflow digest, exact Actions artifact names,
public filename mapping and signing identity. After reviewing a CI change,
regenerate its policy with `uv run python scripts/ci/c_release_policy.py`
and review the resulting diff before committing.

First require the complete CI run on the exact master commit to pass. The
run may start from a master push or a manual `ci.yml` dispatch on master.
If GitHub infrastructure interrupts a run, dispatch the complete workflow
again on the same commit. Each accepted run must succeed on its first
attempt with every required job and step; partial reruns remain rejected.
No source change is needed to restart verification.

The five C distribution jobs build deterministic artifact archives, validate
their dependency closure, notices, SBOMs and provenance, and exercise clean
compiler-free installation plus compiled provider-free and live consumers.
The live journey includes direct and caller-transport queries, custom-root
TLS, migrations, serialization, cancellation, limits and cleanup.

Dispatch `c-release.yml` with `mode=verify` and that run's `ci_run_id` on
the same source. Verification has read-only GitHub permissions. It checks
every CI job and step, downloads exact Actions artifact IDs, verifies API
archive digests and sizes, and rejects unexpected, linked or unsafe members.
It then runs the queries, models, administration, serialization and artifacts
suite producers and independent comparators. Reports are retained under
`reports/<suite>/<binding>.json`; artifacts binds the other four suites by name.
The terminal FULL-C audit must accept all 44 C capabilities and every
predecessor and current-SDK obligation.

The successful attempt-one verification run produces
`c-verified-release-2.3.2`. It contains the three accepted archives, a complete
evidence archive and a verification receipt binding their hashes to the
source, CI run and policy. Public filenames are assigned by copying the
accepted archive bytes; archive members and embedded artifact provenance
remain unchanged. The artifact's nonpublishing disposition is preserved.

## Protected promotion

Complete the ordinary 2.3.2 release preflight and immutable annotated tag
procedure. `release.yml` owns Python, npm, Cargo, OCI and the ordinary GitHub
draft. Preserve successful publisher outputs and independently verify their
public bytes before final publication.

The completed 2.2.1 release used an incident-specific recovery after
[run 34846118498](https://github.com/ds1sqe/type-bridge/actions/runs/34846118498)
stopped at a stale OCI certificate check. Its original source, tag, and bytes
were preserved. The retained `.github/release/recovery-2.2.1.json` policy and
read-only verifier support inspection of that historical evidence.
The 2.3.2 workflow builds new artifacts and has no historical-publication
recovery input.

The 2.2.1 OCI `publication-recovery/v1` attestations distinguish its original
build from recovery control revision and run. That release's metadata retains
the recovery and SBOM links. New ordinary releases produce build-provenance
attestations for their own accepted source and run.

On the exact `v2.3.2` tag, dispatch `c-release.yml` with `mode=publish` and
the successful same-source `verify_run_id`. This job uses the `release`
environment and rechecks the verification run, every required step, the master
CI run, accepted artifact digests, policy and annotated tag object. Promotion
installs no build toolchain and performs no compilation or archive rebuild.

Cosign 3.0.6 signs each of the six payloads: the three archives, evidence,
verification receipt and separate promotion record. The six Sigstore bundles
must verify with this exact certificate identity:

```text
https://github.com/ds1sqe/type-bridge/.github/workflows/c-release.yml@refs/tags/v2.3.2
```

The issuer is `https://token.actions.githubusercontent.com`; the certificate's
workflow SHA must equal the accepted source commit. The signed promotion
record binds the tag object, source tree, original artifact set, CI and
verification runs, public names and payload hashes. It is the separate release
authorization record for the previously nonpublishing artifact bytes.

Promotion adds twelve C assets to the existing GitHub draft and downloads
them again for byte and signature verification. It cannot create a release,
change its body, make it public, or publish another distribution.

## Partial upload recovery

Use a new attempt-one publish dispatch with the same accepted verification
run and unchanged tag. Existing payloads must match byte for byte. Existing
signature bundles must verify against the expected payload, workflow identity,
issuer and source; retain those bundles instead of replacing them. A conflict
stops the operation before signing or uploading any new file. Upload only
absent assets, then independently verify the complete draft again.

Never delete a conflicting asset to force this procedure through. Reconcile
its source and publication record before proceeding. Successful public bytes
and the release tag are immutable.

## Public verification and support

Before making the draft public, independently verify the complete ordinary
and C asset inventory and release notes. After publication, download every C
payload and signature through its public URL. Verify the signatures and the
promotion-to-verification-to-CI hash chain, then repeat the clean installation,
C17/C++17, loader, provider-free, TLS, migration and complete application
journeys using those public bytes.

The 2.2.0 public result on 2026-09-14 establishes FULL C support for the
selected matrix. It binds source `6c9f37c5c01cba25db62195e83af42f1a6a4d46d`,
[CI run 34790325709](https://github.com/ds1sqe/type-bridge/actions/runs/34790325709),
and [C verification run 34793581010](https://github.com/ds1sqe/type-bridge/actions/runs/34793581010).
The signed evidence, receipt, promotion record, and all six Sigstore bundles
are retained in the
[2.2.0 release](https://github.com/ds1sqe/type-bridge/releases/tag/v2.2.0).
Update the support documentation with the actual release date and
verified runtime/compiler requirements, preserve the public evidence record,
and complete the C handoff before starting Kotlin/JVM.

The 2.2.1 public artifacts were independently verified on 2026-09-14.
They bind source `5d1a1e17c0b41fb720a3a60040ddd215e453a899`,
[CI run 34834062772](https://github.com/ds1sqe/type-bridge/actions/runs/34834062772),
[C verification run 34839669075](https://github.com/ds1sqe/type-bridge/actions/runs/34839669075),
and [promotion run 34867352503](https://github.com/ds1sqe/type-bridge/actions/runs/34867352503).
The [2.2.1 release](https://github.com/ds1sqe/type-bridge/releases/tag/v2.2.1)
retains the signed evidence, verification receipt, promotion record, and
Sigstore bundles. Public installation and complete C application journeys
passed against those downloaded release bytes.

The 2.2.2 public artifacts were independently verified on 2026-09-14.
They bind source `ec5e0814feb65a43909f0cec0ae12d2a7a64323c`,
[CI run 34882563529](https://github.com/ds1sqe/type-bridge/actions/runs/34882563529),
[C verification run 34890264580](https://github.com/ds1sqe/type-bridge/actions/runs/34890264580),
and [promotion run 34906322357](https://github.com/ds1sqe/type-bridge/actions/runs/34906322357).
The [2.2.2 release](https://github.com/ds1sqe/type-bridge/releases/tag/v2.2.2)
retains the signed evidence, verification receipt, promotion record, and
Sigstore bundles. Public archive signatures, provenance, clean installation,
and the complete C application journeys passed against the downloaded bytes.
This release includes patched rustls 0.23.45 for RUSTSEC-2026-0285.

The 2.2.3 public artifacts were independently verified on 2026-10-07.
They bind source `50691fde386ecbd30d873f78a04cce69802ec995`,
[CI run 37570930469](https://github.com/ds1sqe/type-bridge/actions/runs/37570930469),
[C verification run 37577237787](https://github.com/ds1sqe/type-bridge/actions/runs/37577237787),
and [promotion run 37592022992](https://github.com/ds1sqe/type-bridge/actions/runs/37592022992).
The [2.2.3 release](https://github.com/ds1sqe/type-bridge/releases/tag/v2.2.3)
retains the signed evidence, verification receipt, promotion record, and
Sigstore bundles. Public archive signatures, provenance, clean installation,
and the complete C application journeys passed against the downloaded bytes.
This release keeps C ABI 1.6.0 and includes the #244 window-order fix.

The 2.2.4 public artifacts were independently verified on 2026-10-11.
They bind source `7050336375b90b5adc96d6741d4f3432e5216d34`,
[CI run 37750473800](https://github.com/ds1sqe/type-bridge/actions/runs/37750473800),
[C verification run 37761176229](https://github.com/ds1sqe/type-bridge/actions/runs/37761176229),
and [promotion run 37778892415](https://github.com/ds1sqe/type-bridge/actions/runs/37778892415).
The [2.2.4 release](https://github.com/ds1sqe/type-bridge/releases/tag/v2.2.4)
retains the signed evidence, verification receipt, promotion record, and
Sigstore bundles. Public archive signatures, provenance, clean installation,
and the complete C application journeys passed against the downloaded bytes.
This release keeps C ABI 1.6.0 and includes the #247 bounded-query fix.

The 2.2.5 public artifacts were independently verified on 2026-10-15.
They bind source `51f15f5f92c4b954de94aaf7a7f67e03e877e16c`,
[CI run 38670370556](https://github.com/ds1sqe/type-bridge/actions/runs/38670370556),
[C verification run 38676152137](https://github.com/ds1sqe/type-bridge/actions/runs/38676152137),
and [promotion run 38689373079](https://github.com/ds1sqe/type-bridge/actions/runs/38689373079).
The [2.2.5 release](https://github.com/ds1sqe/type-bridge/releases/tag/v2.2.5)
retains the signed evidence, verification receipt, promotion record, and
Sigstore bundles. Public archive signatures, provenance, clean installation,
and the complete C application journeys passed against the downloaded bytes.
This release keeps C ABI 1.6.0 and includes patched rustls 0.23.46 for
RUSTSEC-2026-0346 through RUSTSEC-2026-0349.

The 2.2.6 public artifacts were independently verified on 2026-10-20.
They bind source `af379d875dfb6b7156bb1b133a3af702d1bf1a66`,
[CI run 38868929656](https://github.com/ds1sqe/type-bridge/actions/runs/38868929656),
[C verification run 39338460623](https://github.com/ds1sqe/type-bridge/actions/runs/39338460623),
and [promotion run 39348322661](https://github.com/ds1sqe/type-bridge/actions/runs/39348322661).
The [2.2.6 release](https://github.com/ds1sqe/type-bridge/releases/tag/v2.2.6)
retains the signed evidence, verification receipt, promotion record, and
Sigstore bundles. Public archive signatures, provenance, and the CMake,
pkg-config, relocation and runtime-range checks passed against the downloaded
bytes. This release keeps C ABI 1.6.0 and includes the #251 commit certainty
fix.
