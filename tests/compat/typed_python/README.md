# Typed Python wheel acceptance

This consumer proves that the public `type_bridge.typed` runtime and its static
types ship in the actual root/native wheels. The runner extracts two prebuilt
wheels into a temporary directory outside the checkout; it never builds,
installs, or resolves a package.

Supply an already prepared Python interpreter containing the root package's
third-party runtime dependencies and an explicit Pyright executable:

```bash
python scripts/ci/run_typed_python_artifact.py \
  --root-wheel /artifacts/type_bridge-2.0.1-py3-none-any.whl \
  --core-wheel /artifacts/type_bridge_core-2.0.1-*.whl \
  --python /prepared/venv/bin/python \
  --pyright /prepared/venv/bin/pyright
```

The runtime probe launches the prepared interpreter with `-I`, removes ambient
import overrides and checkout package paths, constructs native-backed
positional/named/collected queries, checks immutable wrappers and fail-closed
terminal errors, and requires every loaded TypeBridge module and distribution
record to resolve inside the extracted wheels.

The positive and intentional-negative consumers run from the same isolated
directory. Pyright resolves `type_bridge` and `type_bridge_core` from the
extracted wheel contents, proving scalar, tuple, repeated, collected, named,
page, count, and exists types plus the public rejection boundaries. A
representative bindgen model also exercises an inherited owner-aware class
field directly, without a consumer cast. Hand-written and generated role
descriptors retain both the player and relation owner, so binding a role also
requires no cast or type-checker suppression.
