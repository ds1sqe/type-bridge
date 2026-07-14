"""Regression coverage for the Rust bindgen surfaces.

The temporary ``tmp/bindgen_surface_parity.py`` probe proved that Rust, Python,
and TypeScript all route through the same bindgen engine. These tests keep that
coverage permanent and add a live TypeDB round trip from a TOML-origin schema.
"""

from __future__ import annotations

import difflib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from types import ModuleType
from typing import Any, Literal

import pytest
from type_bridge_core import render_models_json, toml_to_typeql

from type_bridge import Database, Entity, Relation, SchemaManager
from type_bridge.generator import generate_models

REPO_ROOT = Path(__file__).resolve().parents[3]
NODE_DIR = REPO_ROOT / "type-bridge-core" / "crates" / "node"
CORE_MANIFEST = REPO_ROOT / "type-bridge-core" / "Cargo.toml"
BindgenTarget = Literal["python", "typescript", "rust"]
TARGETS: tuple[BindgenTarget, ...] = (
    "python",
    "typescript",
    "rust",
)
TOML_FIXTURES = [
    REPO_ROOT / "tests" / "unit" / "generator" / "fixtures" / "annotations_inheritance.toml",
    REPO_ROOT / "tests" / "unit" / "generator" / "fixtures" / "attributes_owns.toml",
    REPO_ROOT / "tests" / "unit" / "generator" / "fixtures" / "bookstore_corpus.toml",
    REPO_ROOT / "tests" / "unit" / "generator" / "fixtures" / "doc_meta.toml",
    REPO_ROOT / "tests" / "unit" / "generator" / "fixtures" / "functions_structs.toml",
    REPO_ROOT / "tests" / "unit" / "generator" / "fixtures" / "relations_roles.toml",
    REPO_ROOT / "tests" / "unit" / "generator" / "fixtures" / "role_cardinality.toml",
    REPO_ROOT / "tests" / "unit" / "generator" / "fixtures" / "social_media.toml",
    REPO_ROOT / "tests" / "unit" / "generator" / "fixtures" / "type_theoretic.toml",
    REPO_ROOT / "examples" / "basic" / "schema.toml",
]
ROLE_CARDINALITY_TOML = (
    REPO_ROOT / "tests" / "unit" / "generator" / "fixtures" / "role_cardinality.toml"
)


NODE_RENDER = r"""
const fs = require("fs");
const pkg = require(process.cwd());
const native = pkg.loadNative();
const input = fs.readFileSync(process.argv[1], "utf8");
const target = process.argv[2];
const options = fs.readFileSync(process.argv[3], "utf8");
process.stdout.write(native.renderModelsJson(input, target, options));
"""

NODE_WRITE = r"""
const fs = require("fs");
const pkg = require(process.cwd());
const input = fs.readFileSync(process.argv[1], "utf8");
const target = process.argv[2];
const output = process.argv[3];
pkg.generateModelsForTarget(input, output, {
  native: pkg.loadNative(),
  target,
  schemaVersion: "1.0.0",
  schemaFilename: null,
  schemaText: input,
});
"""


def _require_bindgen_toolchain() -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo executable is not installed")
    if shutil.which("node") is None:
        pytest.skip("node executable is not installed")
    if not (NODE_DIR / "dist" / "index.js").exists():
        pytest.skip("compiled Node package not built; run `npm run build` in the node crate")
    if not list(NODE_DIR.glob("type_bridge_node*.node")):
        pytest.skip("native node module not built; run `npm run build:native`")


def _node_env() -> dict[str, str]:
    env = dict(os.environ)
    candidates = list(NODE_DIR.glob("type_bridge_node*.node"))
    if candidates:
        env["TYPE_BRIDGE_NODE_NATIVE_PATH"] = str(candidates[0])
    return env


def _render_options(typeql: str) -> dict[str, object]:
    return {
        "schema_version": "1.0.0",
        "schema_filename": None,
        "schema_text": typeql,
        "implicit_key_attributes": [],
    }


def _package_map(payload: str | dict[str, Any]) -> dict[str, str]:
    package: dict[str, Any] = json.loads(payload) if isinstance(payload, str) else payload
    files = package["files"]
    assert isinstance(files, list)
    return {str(file_info["path"]): str(file_info["contents"]) for file_info in files}


def _directory_map(path: Path) -> dict[str, str]:
    result: dict[str, str] = {}
    for file_path in sorted(path.rglob("*")):
        if file_path.is_file():
            result[file_path.relative_to(path).as_posix()] = file_path.read_text(encoding="utf-8")
    return result


def _run_python_native(typeql: str, target: BindgenTarget) -> dict[str, str]:
    return _package_map(render_models_json(typeql, target, json.dumps(_render_options(typeql))))


def _run_rust_native(typeql_path: Path, target: BindgenTarget) -> dict[str, str]:
    completed = subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "--manifest-path",
            str(CORE_MANIFEST),
            "-p",
            "type-bridge-core-lib",
            "--example",
            "bindgen_render",
            "--",
            str(typeql_path),
            target,
        ],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise AssertionError(
            f"Rust bindgen render failed\nstdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    return _package_map(completed.stdout)


def _run_typescript_native(
    typeql_path: Path,
    target: BindgenTarget,
    options_path: Path,
) -> dict[str, str]:
    completed = subprocess.run(
        ["node", "-e", NODE_RENDER, str(typeql_path), target, str(options_path)],
        cwd=NODE_DIR,
        env=_node_env(),
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise AssertionError(
            "TypeScript native bindgen render failed\n"
            f"stdout:\n{completed.stdout}\n"
            f"stderr:\n{completed.stderr}"
        )
    return _package_map(completed.stdout)


def _run_python_writer(
    toml_path: Path,
    output_dir: Path,
    target: BindgenTarget,
) -> dict[str, str]:
    generate_models(toml_path, output_dir, target=target, copy_schema=False)
    return _directory_map(output_dir)


def _run_typescript_writer(
    typeql_path: Path,
    output_dir: Path,
    target: BindgenTarget,
) -> dict[str, str]:
    completed = subprocess.run(
        ["node", "-e", NODE_WRITE, str(typeql_path), target, str(output_dir)],
        cwd=NODE_DIR,
        env=_node_env(),
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise AssertionError(
            "TypeScript bindgen writer failed\n"
            f"stdout:\n{completed.stdout}\n"
            f"stderr:\n{completed.stderr}"
        )
    return _directory_map(output_dir)


def _assert_same_package(label: str, expected: dict[str, str], actual: dict[str, str]) -> None:
    assert actual.keys() == expected.keys(), (
        f"{label} file set differs: "
        f"missing={sorted(set(expected) - set(actual))}, "
        f"extra={sorted(set(actual) - set(expected))}"
    )
    for path, expected_text in expected.items():
        actual_text = actual[path]
        if actual_text != expected_text:
            diff = "\n".join(
                difflib.unified_diff(
                    expected_text.splitlines(),
                    actual_text.splitlines(),
                    fromfile=f"rust/{path}",
                    tofile=f"{label}/{path}",
                    lineterm="",
                )
            )
            raise AssertionError(f"{label} changed {path}:\n{diff}")


def _import_generated_package(package_path: Path) -> dict[str, ModuleType]:
    parent = str(package_path.parent)
    package_name = package_path.name
    inserted = False
    if parent not in sys.path:
        sys.path.insert(0, parent)
        inserted = True

    try:
        modules: dict[str, ModuleType] = {}
        package_spec = importlib.util.spec_from_file_location(
            package_name,
            package_path / "__init__.py",
        )
        assert package_spec and package_spec.loader
        package = importlib.util.module_from_spec(package_spec)
        sys.modules[package_name] = package
        package_spec.loader.exec_module(package)

        for module_name in ("attributes", "entities", "relations"):
            module_path = package_path / f"{module_name}.py"
            spec = importlib.util.spec_from_file_location(
                f"{package_name}.{module_name}",
                module_path,
            )
            assert spec and spec.loader
            module = importlib.util.module_from_spec(spec)
            sys.modules[f"{package_name}.{module_name}"] = module
            spec.loader.exec_module(module)
            modules[module_name] = module
        return modules
    finally:
        for key in [key for key in sys.modules if key.startswith(package_name)]:
            del sys.modules[key]
        if inserted:
            sys.path.remove(parent)


def _generated_classes(module: ModuleType, base: type[Any]) -> list[type[Any]]:
    classes: list[type[Any]] = []
    for name in dir(module):
        value = getattr(module, name)
        if isinstance(value, type) and issubclass(value, base) and value is not base:
            classes.append(value)
    return sorted(classes, key=lambda cls: cls.get_type_name())


@pytest.mark.integration
def test_comprehensive_toml_bindgen_surface_parity(tmp_path: Path) -> None:
    """All public bindgen facades render the same package for TOML schemas."""
    _require_bindgen_toolchain()
    checked = 0

    for toml_path in TOML_FIXTURES:
        typeql = toml_to_typeql(toml_path.read_text(encoding="utf-8"))
        typeql_path = tmp_path / f"{toml_path.stem}.tql"
        options_path = tmp_path / f"{toml_path.stem}.options.json"
        typeql_path.write_text(typeql, encoding="utf-8")
        options_path.write_text(json.dumps(_render_options(typeql)), encoding="utf-8")

        for target in TARGETS:
            expected = _run_rust_native(typeql_path, target)
            _assert_same_package(
                f"{toml_path.name} {target} Python native",
                expected,
                _run_python_native(typeql, target),
            )
            _assert_same_package(
                f"{toml_path.name} {target} TypeScript native",
                expected,
                _run_typescript_native(typeql_path, target, options_path),
            )
            _assert_same_package(
                f"{toml_path.name} {target} Python writer",
                expected,
                _run_python_writer(
                    toml_path, tmp_path / "python-writer" / toml_path.stem / target, target
                ),
            )
            _assert_same_package(
                f"{toml_path.name} {target} TypeScript writer",
                expected,
                _run_typescript_writer(
                    typeql_path,
                    tmp_path / "typescript-writer" / toml_path.stem / target,
                    target,
                ),
            )
            checked += 1

    assert checked == len(TOML_FIXTURES) * len(TARGETS)


@pytest.mark.integration
def test_toml_generated_python_models_round_trip_real_db(
    tmp_path: Path,
    clean_db: Database,
) -> None:
    """A TOML-origin bindgen package syncs schema and round-trips live data."""
    output = tmp_path / "role_cardinality_from_toml"
    generate_models(ROLE_CARDINALITY_TOML, output, copy_schema=False)
    generated = _import_generated_package(output)

    entity_classes = _generated_classes(generated["entities"], Entity)
    relation_classes = _generated_classes(generated["relations"], Relation)
    assert {cls.get_type_name() for cls in entity_classes} >= {
        "document",
        "group",
        "memory",
        "person",
    }
    assert {cls.get_type_name() for cls in relation_classes} >= {
        "friendship",
        "group_membership",
        "is_similar_to",
        "review",
    }

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(*entity_classes, *relation_classes)
    schema_manager.sync_schema(force=True)

    schema = clean_db.get_schema()
    for type_name in ("person", "document", "review", "score"):
        assert type_name in schema

    attributes = generated["attributes"]
    entities = generated["entities"]
    relations = generated["relations"]

    person = entities.Person(name=attributes.Name("Alice"))
    document = entities.Document(
        name=attributes.Name("Design Doc"),
        content=attributes.Content("Architecture design"),
    )

    entities.Person.manager(clean_db).insert(person)
    entities.Document.manager(clean_db).insert(document)

    person_fetched = entities.Person.manager(clean_db).filter(name="Alice").first()
    document_fetched = entities.Document.manager(clean_db).filter(name="Design Doc").first()
    assert person_fetched is not None
    assert document_fetched is not None
    assert str(person_fetched.name) == "Alice"
    assert str(document_fetched.content) == "Architecture design"

    review = relations.Review(
        document=document_fetched,
        reviewer=person_fetched,
        score=attributes.Score(4.75),
    )
    relations.Review.manager(clean_db).insert(review)

    reviews = relations.Review.manager(clean_db).all()
    assert len(reviews) == 1
    assert float(reviews[0].score) == 4.75
