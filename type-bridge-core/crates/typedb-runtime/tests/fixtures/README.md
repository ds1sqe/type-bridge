# Commit error captures

Captured from TypeDB CE 3.12.3 and 3.13.6 on 2026-10-15. Each JSON string is the
unaltered driver Display text, including its root-first cause stack. Database
names and values belong to the isolated test probe.

The 3.12.3 probe used TypeDBRuntime with its normal HTTP version admission.
The 3.13.6 probe used typedb-driver 3.12.3 directly, with plaintext TLS policy,
solely to capture future-band fixture strings. TypeDB 3.13 remains outside the
2.2 runtime support window; these fixtures do not change admission.

Both probes used:

```typeql
define
attribute id, value string;
attribute score, value integer;
entity item, owns id @key, owns score @card(0..1);
```

- `stc2`: two concurrently opened write transactions updated the same
  existing item's score, then raced their commits. Exactly one succeeded.
- `cnt5`: one new item owned two scores under `@card(0..1)`; commit failed.
- `cnt9_query`: a new item reused the existing item's key; query execution failed.
- `paused`: after inserting a new item in a fresh write transaction, pause
  the owned server container, then commit. The driver's actual connection
  failure arrived before the 55-second probe limit. The container was always
  unpaused afterwards. No synthetic timeout is used as a driver fixture.

The exact rejection table uses the storage, commit and concept-error enums in
TypeDB tags 3.12.3 and 3.13.6. Classification uses the driver's public
`Error::code()` root code, independently of Display text. Fixture tests
reconstruct gRPC status details with that root code and the captured stack.
Unknown root codes stay unknown even when Display contains a known code.
