"""Python entrypoint for generating migrations.

Runnable as::

    python -m type_bridge.migration._generate --name <n> --models <module> ...

This module owns the generation half of the ``makemigrations`` verb.  It
connects to TypeDB via the Python ORM path, discovers the specified model
module, and delegates entirely to :class:`~type_bridge.migration.generator.
MigrationGenerator` (which in turn calls the shared Rust diff engine).

No diff or IR logic lives here — only the CLI shim and the generator call.
This satisfies invariant 2: there is one canonical diff engine
(``generator.py`` / Rust); ``_generate`` is the model-side bootstrap for
the bin's ``makemigrations`` verb, not a second generator.

Exit codes
----------
0 — migration file created or no changes detected.
1 — argument error, import error, connection error, or generation failure.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


def main(argv: list[str] | None = None) -> int:
    """Entry point for ``python -m type_bridge.migration._generate``.

    Parameters
    ----------
    argv:
        Argument list to parse (defaults to ``sys.argv[1:]`` when *None*).

    Returns
    -------
    int
        Exit code: 0 success, 1 error.
    """
    parser = argparse.ArgumentParser(
        prog="python -m type_bridge.migration._generate",
        description=(
            "Connect to TypeDB, discover models, and write a new .py+.json "
            "migration pair via MigrationGenerator.  Used by the Rust migration "
            "CLI's makemigrations verb (invariant 2: no parallel .py generator)."
        ),
    )
    parser.add_argument(
        "--name",
        required=True,
        metavar="<name>",
        help="Migration name suffix (e.g. 'add_phone').",
    )
    parser.add_argument(
        "--empty",
        action="store_true",
        default=False,
        help="Create an empty migration for manual editing.",
    )
    parser.add_argument(
        "--models",
        metavar="<module>",
        default=None,
        help=(
            "Dotted Python module path to discover models from "
            "(e.g. myapp.models).  If omitted, uses all models registered "
            "in the global ModelRegistry."
        ),
    )
    parser.add_argument(
        "--address",
        "-a",
        metavar="<address>",
        default="localhost:1730",
        help="TypeDB server address (default: localhost:1730).",
    )
    parser.add_argument(
        "--database",
        "-d",
        metavar="<database>",
        default="typedb",
        help="TypeDB database name (default: typedb).",
    )
    parser.add_argument(
        "--migrations-dir",
        metavar="<dir>",
        default="migrations",
        help="Directory to write the generated migration files (default: migrations).",
    )
    parser.add_argument(
        "--username",
        metavar="<username>",
        default="admin",
        help="TypeDB username (default: admin).",
    )
    parser.add_argument(
        "--password",
        metavar="<password>",
        default="password",
        help="TypeDB password (default: password).",
    )
    parser.add_argument(
        "--verbose",
        "-v",
        action="store_true",
        default=False,
        help="Enable verbose logging to stderr.",
    )

    args = parser.parse_args(argv)

    if args.verbose:
        import logging

        logging.basicConfig(level=logging.DEBUG, format="%(levelname)s: %(message)s")

    migrations_dir = Path(args.migrations_dir)

    # --- Connect to TypeDB via the Python ORM path ---------------------------
    try:
        from type_bridge.session import Database

        db = Database(
            address=args.address,
            database=args.database,
        )
        db.connect()
    except Exception as exc:  # noqa: BLE001
        print(
            f"_generate: connection error: {exc}",
            file=sys.stderr,
        )
        return 1

    # --- Discover models -----------------------------------------------------
    try:
        from type_bridge.migration.registry import ModelRegistry

        if args.models:
            model_list = ModelRegistry.discover(args.models, register=False)
            print(
                f"Discovered {len(model_list)} model(s) from {args.models}",
                file=sys.stderr,
            )
        else:
            model_list = ModelRegistry.get_all()
            if model_list:
                print(
                    f"Using {len(model_list)} registered model(s)",
                    file=sys.stderr,
                )

        if not model_list and not args.empty:
            print(
                "No models found.  Either:\n"
                "  1. Use --models to specify a module: --models myapp.models\n"
                "  2. Register models with ModelRegistry.register() before running\n"
                "  3. Use --empty to create an empty migration for manual editing",
                file=sys.stderr,
            )
            db.close()
            return 1

    except ImportError as exc:
        print(
            f"_generate: import error: cannot import '{args.models}': {exc}",
            file=sys.stderr,
        )
        db.close()
        return 1
    except Exception as exc:  # noqa: BLE001
        print(
            f"_generate: model discovery error: {exc}",
            file=sys.stderr,
        )
        db.close()
        return 1

    # --- Generate the migration -----------------------------------------------
    try:
        from type_bridge.migration.generator import MigrationGenerator

        generator = MigrationGenerator(db, migrations_dir)
        path = generator.generate(models=model_list, name=args.name, empty=args.empty)
        db.close()
    except Exception as exc:  # noqa: BLE001
        print(
            f"_generate: generation error: {exc}",
            file=sys.stderr,
        )
        try:
            db.close()
        except Exception:  # noqa: BLE001
            pass
        return 1

    # --- Report result to stdout (caller/bin reads this) ---------------------
    if path:
        print(f"Created: {path}")
    else:
        print("No changes detected")

    return 0


if __name__ == "__main__":
    sys.exit(main())
