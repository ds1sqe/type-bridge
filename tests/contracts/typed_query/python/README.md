# Python public static contract

These test-only fixtures import the real `type_bridge.typed` facade. They pin
the #171 contract against the surface activated by #174 instead of maintaining
a second declaration-only model.

`documented_examples.py` is the ordered concatenation of all five Python code
blocks marked in `docs/development/typed-query-contract.md`. The corpus unit
test rejects source drift, and the repository Pyright pass compiles the file.

Run the positive fixture with repository Pyright:

```bash
pyright tests/contracts/typed_query/python/positive.py
```

Run the intentional negative fixture through its diagnostic-checking runner:

```bash
python tests/contracts/typed_query/python/check_negative.py \
  --pyright "$(command -v pyright)"
```

`negative.py` is excluded from the root Pyright config. The runner uses the
dedicated config against the public facade, requires diagnostics exactly at the
marked zero- and seventeen-selection calls, and fails on extra or foreign
errors. Selecting the same `BoundVar[Person]` value twice is deliberately not
marked as a static error: Python types cannot distinguish runtime value
identity, so Rust rejects that case before executor invocation.
