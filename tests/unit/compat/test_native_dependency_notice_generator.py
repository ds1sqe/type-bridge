"""Fail-closed contracts for the native Rust dependency notice generator."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts/ci/generate_native_dependency_notice.py"
SPEC = importlib.util.spec_from_file_location("native_dependency_notice_generator", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
generator = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = generator
SPEC.loader.exec_module(generator)


def package(name: str, expression: str, *, version: str = "1.0.0") -> dict[str, Any]:
    return {
        "name": name,
        "version": version,
        "id": f"registry+https://github.com/rust-lang/crates.io-index#{name}@{version}",
        "source": "registry+https://github.com/rust-lang/crates.io-index",
        "manifest_path": f"/registry/{name}-{version}/Cargo.toml",
        "license": expression,
    }


def payload(
    packages: list[dict[str, Any]],
    selections: list[tuple[str, str, list[str]]],
) -> dict[str, Any]:
    by_name = {item["name"]: item for item in packages}
    return {
        "overview": [],
        "crates": [{"package": item, "license": item["license"]} for item in packages],
        "licenses": [
            {
                "name": license_id,
                "id": license_id,
                "text": text,
                "source_path": "LICENSE",
                "used_by": [{"crate": by_name[name]} for name in names],
            }
            for license_id, text, names in selections
        ],
    }


def both_roots(value: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {"Python": value, "Node": value}


def policy(*accepted: str) -> Any:
    return generator.NoticePolicy(accepted=accepted, targets=generator.EXPECTED_TARGETS)


def merge(value: dict[str, Any], accepted: tuple[str, ...], tmp_path: Path) -> Any:
    return generator.merge_payloads(both_roots(value), policy=policy(*accepted), workspace=tmp_path)


def test_compound_and_expression_rejects_a_missing_license_body(tmp_path: Path) -> None:
    value = payload(
        [package("ring", "Apache-2.0 AND ISC")],
        [("Apache-2.0", "Apache terms\n", ["ring"])],
    )

    with pytest.raises(generator.ValidationError, match="compound SPDX expression"):
        merge(value, ("Apache-2.0", "ISC"), tmp_path)


def test_unknown_selected_license_id_is_rejected_before_rendering(tmp_path: Path) -> None:
    value = payload(
        [package("unexpected", "BSD-2-Clause")],
        [("BSD-2-Clause", "BSD terms\n", ["unexpected"])],
    )

    with pytest.raises(generator.ValidationError, match="unknown/unaccepted SPDX id"):
        merge(value, ("MIT",), tmp_path)


def test_unused_accepted_license_id_is_rejected_as_stale(tmp_path: Path) -> None:
    value = payload(
        [package("only-mit", "MIT")],
        [("MIT", "MIT terms\n", ["only-mit"])],
    )

    with pytest.raises(generator.ValidationError, match="stale accepted SPDX.*Apache-2.0"):
        merge(value, ("MIT", "Apache-2.0"), tmp_path)


def test_with_exception_requires_the_exception_text(tmp_path: Path) -> None:
    value = payload(
        [package("winx", "Apache-2.0 WITH LLVM-exception")],
        [("Apache-2.0", "Apache terms without the exception\n", ["winx"])],
    )

    with pytest.raises(generator.ValidationError, match="compound SPDX expression"):
        merge(value, ("Apache-2.0 WITH LLVM-exception",), tmp_path)

    value["licenses"][0]["text"] += "LLVM Exceptions to the Apache 2.0 License\n"
    packages, texts = merge(value, ("Apache-2.0 WITH LLVM-exception",), tmp_path)
    assert len(packages) == 1
    assert len(texts) == 1


def test_distinct_copyright_texts_survive_deterministic_union(tmp_path: Path) -> None:
    packages = [package("alpha", "MIT"), package("beta", "MIT")]
    selections = [
        ("MIT", "Copyright Alpha\nMIT terms\n", ["alpha"]),
        ("MIT", "Copyright Beta\nMIT terms\n", ["beta"]),
    ]
    first = payload(packages, selections)
    second = payload(list(reversed(packages)), list(reversed(selections)))

    first_packages, first_texts = merge(first, ("MIT",), tmp_path)
    second_packages, second_texts = merge(second, ("MIT",), tmp_path)
    first_render = generator.render_generated_block(first_packages, first_texts, policy("MIT"))
    second_render = generator.render_generated_block(second_packages, second_texts, policy("MIT"))

    assert first_render == second_render
    assert "Copyright Alpha\nMIT terms" in first_render
    assert "Copyright Beta\nMIT terms" in first_render
    assert first_render.count("#### `MIT` — `sha256:") == 2


def test_python_and_node_are_scanned_as_independent_union_roots(tmp_path: Path) -> None:
    python = payload(
        [package("python-only", "MIT")],
        [("MIT", "MIT terms\n", ["python-only"])],
    )
    node = payload(
        [package("node-only", "MIT")],
        [("MIT", "MIT terms\n", ["node-only"])],
    )

    packages, texts = generator.merge_payloads(
        {"Python": python, "Node": node}, policy=policy("MIT"), workspace=tmp_path
    )
    rendered = generator.render_generated_block(packages, texts, policy("MIT"))

    assert "`python-only` | `1.0.0` | Python |" in rendered
    assert "`node-only` | `1.0.0` | Node |" in rendered


def test_actual_policy_is_exact_and_checksum_clarified() -> None:
    loaded = generator.load_policy(ROOT / "type-bridge-core")

    assert loaded.targets == generator.EXPECTED_TARGETS
    assert "Apache-2.0 WITH LLVM-exception" in loaded.accepted


@pytest.mark.parametrize(
    ("before", "after"),
    [
        ("LICENSE-MIT.md", "LICENSE"),
        (generator.MINIZ_OXIDE_LICENSE_CHECKSUM, "0" * 64),
        ('license = "MIT OR Zlib OR Apache-2.0"', 'license = "MIT"'),
    ],
)
def test_miniz_clarification_rejects_ambiguous_or_changed_inputs(
    tmp_path: Path, before: str, after: str
) -> None:
    source = (ROOT / "type-bridge-core/about.toml").read_text()
    (tmp_path / "about.toml").write_text(source.replace(before, after))
    with pytest.raises(generator.ValidationError, match="miniz_oxide clarification"):
        generator.load_policy(tmp_path)


def test_cargo_about_uses_the_documented_rust_toolchain(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("RUSTUP_TOOLCHAIN", "nightly")
    captured = []

    def fake_run(command: list[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
        captured.append(kwargs["env"])
        return subprocess.CompletedProcess(command, 0, stdout=json.dumps({}), stderr="")

    monkeypatch.setattr(generator.subprocess, "run", fake_run)
    generator.run_cargo_about("cargo-about", generator.ROOT_SPECS[0], workspace=tmp_path)
    assert captured[0]["RUSTUP_TOOLCHAIN"] == generator.RUST_TOOLCHAIN == "1.94.1"


def test_write_repairs_generated_only_copy_divergence(tmp_path: Path) -> None:
    python_notice = tmp_path / generator.PYTHON_NOTICE
    node_notice = tmp_path / generator.NODE_NOTICE
    python_notice.parent.mkdir(parents=True)
    node_notice.parent.mkdir(parents=True)
    old_python = (
        f"custom provenance\n\n{generator.BEGIN_MARKER}\nold Python block\n{generator.END_MARKER}\n"
    )
    old_node = (
        f"custom provenance\n\n{generator.BEGIN_MARKER}\nold Node block\n{generator.END_MARKER}\n"
    )
    python_notice.write_text(old_python)
    node_notice.write_text(old_node)
    replacement = f"{generator.BEGIN_MARKER}\nreplacement block\n{generator.END_MARKER}"

    generator.check_or_write_notices(workspace=tmp_path, block=replacement, write=True)

    assert python_notice.read_bytes() == node_notice.read_bytes()
    assert "replacement block" in python_notice.read_text()
