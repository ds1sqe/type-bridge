"""F1 model-IR subprocess entrypoint for the migration CLI.

Runnable as::

    python -m type_bridge.migration._ir_dump <models-module>

The command discovers the Entity/Relation classes in *models-module* via
``ModelRegistry.discover`` (which imports the module), registers each into the
Rust descriptor registry via ``register_model_descriptor``, calls
``type_bridge._rust_runtime.model_schema_info()``, and writes the resulting
serde SchemaInfo as a single JSON document to stdout.

This module is import-register-serialize ONLY.  It contains no diff logic,
no comparison, and no broken/schema-change detection (F1 invariant lock —
invariant 2: single canonical diff engine; `_ir_dump` is the model-side
bootstrap only and never touches the DB).

Exit codes
----------
0 — success; JSON written to stdout.
1 — import or registration error (module not found, metaclass failure); one-line
    diagnostic on stderr.
2 — JSON serialization error; one-line diagnostic on stderr.
"""

from __future__ import annotations

import argparse
import json
import sys
from typing import Any


def main(argv: list[str] | None = None) -> int:
    """Entry point for ``python -m type_bridge.migration._ir_dump``.

    Parameters
    ----------
    argv:
        Argument list to parse (defaults to ``sys.argv[1:]`` when *None*).

    Returns
    -------
    int
        Exit code: 0 success, 1 import/registration error, 2 serialization
        error.
    """
    parser = argparse.ArgumentParser(
        prog="python -m type_bridge.migration._ir_dump",
        description=(
            "Import a models module and emit the serde SchemaInfo JSON to stdout. "
            "Used by the Rust migration CLI to obtain the model-side IR for makemigrations."
        ),
    )
    parser.add_argument(
        "models_module",
        metavar="<models-module>",
        help="Dotted Python import path to the models module (e.g. myapp.models).",
    )
    # --app-label is accepted for forward-compatibility with future app-scoped output.
    # In v1 the full SchemaInfo (all registered models) is always emitted; app-label
    # scoping is reserved and not yet implemented.
    parser.add_argument(
        "--app-label",
        metavar="<label>",
        default=None,
        help=(
            "Reserved for future app-label scoped output. "
            "Accepted but ignored in v1; the full SchemaInfo is always emitted."
        ),
    )

    args = parser.parse_args(argv)

    # --- Step 1: discover the models module's Entity/Relation classes and
    # register each into the Rust descriptor registry. Defining a model class
    # does not auto-register it; discovery imports the module and registration
    # populates the registry that model_schema_info() reads.
    try:
        from type_bridge._rust_runtime import (
            model_schema_info,
            register_model_descriptor,
        )
        from type_bridge.migration.registry import ModelRegistry

        models = ModelRegistry.discover(args.models_module)
        for model in models:
            register_model_descriptor(model)
    except ImportError as exc:
        print(
            f"_ir_dump: import error: cannot import '{args.models_module}': {exc}",
            file=sys.stderr,
        )
        return 1
    except Exception as exc:  # noqa: BLE001 — discovery / registration failure
        print(
            f"_ir_dump: registration error for '{args.models_module}': {exc}",
            file=sys.stderr,
        )
        return 1

    # --- Step 2: obtain the serde SchemaInfo from the descriptor registry ----
    try:
        schema_info: dict[str, Any] = model_schema_info()
    except Exception as exc:  # noqa: BLE001 — runtime / extension error
        print(
            f"_ir_dump: error reading schema info: {exc}",
            file=sys.stderr,
        )
        return 1

    # --- Step 3: serialize to stdout ----------------------------------------
    # Nothing else goes to stdout; any logging or diagnostics go to stderr.
    try:
        json.dump(schema_info, sys.stdout)
    except (TypeError, ValueError) as exc:
        print(
            f"_ir_dump: serialization error: {exc}",
            file=sys.stderr,
        )
        return 2

    return 0


if __name__ == "__main__":
    sys.exit(main())
