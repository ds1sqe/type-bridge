# C release procedure

The C release path selects product version **2.2.0**, native ABI **1.6.0**,
and the `x86_64-unknown-linux-gnu` shared runtime built and consumed on
Ubuntu 24.04. C17 and C++17 applications use the installed runtime through
CMake or pkg-config. This preparation does not establish public C support;
that requires the public verification below.

The standalone CLI, native runtime and generated sdk example are
separate archives. Application owners generate and distribute their own
schema packages. Static libraries, macOS, Windows and other architectures
are outside this release's selected C distribution matrix.

Connected migration acceptance requires exactly TypeDB **3.12.3**. The
canonical schema and query semantic profile remains `typedb-3.12.1/v1`;
that profile identifier is not the connected server version requirement.

## Source and artifact acceptance

The committed policy in `.github/release/c-2.2.0.json` enumerates every CI
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
It then runs the real Sdk V1–V6 producers and independent comparators.
The terminal FULL-C audit must accept all 44 C capabilities and every
predecessor and current-SDK obligation.

The successful attempt-one verification run produces
`c-verified-release-2.2.0`. It contains the three accepted archives, a complete
evidence archive and a verification receipt binding their hashes to the
source, CI run and policy. Public filenames are assigned by copying the
accepted archive bytes; archive members and embedded artifact provenance
remain unchanged. The artifact's nonpublishing disposition is preserved.

## Protected promotion

Complete the ordinary 2.2.0 release preflight and immutable annotated tag
procedure. `release.yml` owns Python, npm, Cargo, OCI and the ordinary GitHub
draft. Preserve successful publisher outputs and independently verify their
public bytes before final publication.

On the exact `v2.2.0` tag, dispatch `c-release.yml` with `mode=publish` and
the successful same-source `verify_run_id`. This job uses the `release`
environment and rechecks the verification run, every required step, the master
CI run, accepted artifact digests, policy and annotated tag object. Promotion
installs no build toolchain and performs no compilation or archive rebuild.

Cosign 3.0.6 signs each of the six payloads: the three archives, evidence,
verification receipt and separate promotion record. The six Sigstore bundles
must verify with this exact certificate identity:

```text
https://github.com/ds1sqe/type-bridge/.github/workflows/c-release.yml@refs/tags/v2.2.0
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

Only that complete public result establishes FULL C support for the selected
matrix. Update the support documentation with the actual release date and
verified runtime/compiler requirements, preserve the public evidence record,
and complete the C handoff before starting Kotlin/JVM.
