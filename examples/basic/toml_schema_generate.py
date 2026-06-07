"""TOML Schema DSL: transpile and generate a TypeBridge package.

This example demonstrates:
- Authoring a schema in TOML (schema.toml in this directory)
- Transpiling the TOML schema to TypeQL and generating a TypeBridge package
- Locating and inspecting the generated package output

The TOML file is passed as a file path; the `.toml` suffix causes
generate_models to route it through the TOML transpiler automatically.
No additional flags are required.
"""

from pathlib import Path

from type_bridge.generator import generate_models

# Resolve paths relative to this file so the example works regardless of
# the working directory from which it is invoked.
HERE = Path(__file__).parent
SCHEMA_FILE = HERE / "schema.toml"
OUTPUT_DIR = HERE / "out" / "toml_demo"


def main() -> None:
    """Transpile schema.toml and write a TypeBridge package to out/toml_demo/."""
    print(f"Schema : {SCHEMA_FILE}")
    print(f"Output : {OUTPUT_DIR}")
    print()

    generate_models(SCHEMA_FILE, OUTPUT_DIR)

    # Show what was generated.
    generated = sorted(OUTPUT_DIR.iterdir())
    print("Generated files:")
    for path in generated:
        print(f"  {path.name}")

    print()
    print("Import the package from the output directory to use the models:")
    print("  from out.toml_demo import entities, relations, attributes")


if __name__ == "__main__":
    main()
