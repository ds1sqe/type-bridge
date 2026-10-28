# Query plan twins

These twins contain the ordered `plans` entries of actual `/v3/staged`
requests made by the four generated SDK query programs. Each entry retains
its decoded canonical `plan_bytes` as `plan` and its invocation `rows`.
The live runner keeps full raw requests and capability advertisements in
separate per-binding witnesses. It compares the four witnesses with each
other and with these committed bytes. Answer observations must still equal
both `journey.json` and the frozen V2 answers.

Each profile has its own directory. TypeDB 3.12.3 uses `typedb-3.12.1/v1`;
TypeDB 3.13.6 and 3.14.0 both use `typedb-3.13.6/v1`. The shared server/profile
selector requires exactly those measured pairs.

Two substitutions apply before comparison:

- A value equal to an actual IID recorded for a named journey record becomes
  `@record/<journey-name>`. Recreated records can retain several recorded
  IIDs. Every unknown IID-shaped value fails, including values under a
  caller-input field. Schema identities, operators and other caller values
  stay unchanged.
- Binding names are local to each plan. References are visited in canonical
  object-key order and original array order, before declarations. The first
  distinct referenced binding becomes 0, then 1, and so on; unreferenced
  declarations follow. References and declaration names change bijectively,
  and declarations are listed by the resulting ID. Duplicate IDs, duplicate
  names and undeclared references fail. Input columns, scalar values, local
  schema-function bodies and all statement order stay unchanged.

The plan comparison excludes the transport envelope: advertisement identity,
nonce, preparation/expiry timestamps, remaining deadline, kind and resource
budgets. Each raw envelope is checked independently:

- kind is exactly `read` and the advertisement digest matches the retained
  canonical advertisement under the contract's fingerprint domain;
- deadline is positive and no greater than the caller deadline or the minimum
  of caller, advertised and session timeout policies; expiry equals
  preparation time plus that exact remaining deadline;
- each other budget equals the minimum of caller, advertised and session
  policies, with the protocol's 64-plan and 256-write ceilings retained. An
  omitted statement policy resolves to the static stack length with a minimum
  of 3.

The entire plan identity remains compared, including `managed_semantics`,
required capabilities, input values, predicates, stages, windows, fetches and
output shape. Local fixture authorities share `schema-codegen-acceptance`;
foreign-package authority scopes remain distinct for refusal tests.
