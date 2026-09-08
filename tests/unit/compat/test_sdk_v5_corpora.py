"""Fail-closed tests for provider-free Sdk V5 corpus comparison."""

from __future__ import annotations

import base64
import importlib.util
import json
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts/ci/compare_sdk_v5_corpora.py"
SPEC = importlib.util.spec_from_file_location("sdk_v5_corpora", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
corpora = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(corpora)


def corpus(binding: str) -> dict[str, Any]:
    return {
        "archive_b64": base64.b64encode(b"archive").decode("ascii"),
        "binding": binding,
        "format": corpora.FORMAT,
        "record_b64": [
            base64.b64encode(f"record-{index}".encode()).decode("ascii") for index in range(9)
        ],
    }


def write(path: Path, value: dict[str, Any]) -> None:
    path.write_bytes(corpora.canonical_json_bytes(value))


def paths(tmp_path: Path) -> dict[str, Path]:
    result = {binding: tmp_path / f"{binding}.json" for binding in corpora.BINDINGS}
    for binding, path in result.items():
        write(path, corpus(binding))
    return result


def test_comparison_requires_exact_equal_nine_record_corpora(tmp_path: Path) -> None:
    result = corpora.compare(paths(tmp_path))
    assert result["format"] == "typebridge.sdk-v5-provider-free-comparison/v1"
    assert len(result["record_sha256"]) == 9
    assert len(result["archive_sha256"]) == 64


@pytest.mark.parametrize(
    ("mutation", "code"),
    [
        (lambda value: value.update({"binding": "node"}), "corpus_identity_mismatch"),
        (lambda value: value["record_b64"].pop(), "invalid_corpus_shape"),
        (lambda value: value["record_b64"].__setitem__(0, "%%%"), "invalid_corpus_bytes"),
        (lambda value: value.update({"extra": True}), "invalid_corpus_shape"),
    ],
)
def test_loader_rejects_hostile_corpus(
    tmp_path: Path,
    mutation: Callable[[dict[str, Any]], None],
    code: str,
) -> None:
    value = corpus("python")
    mutation(value)
    path = tmp_path / "python.json"
    write(path, value)
    with pytest.raises(corpora.CorpusError) as rejected:
        corpora.load_corpus(path, "python")
    assert rejected.value.code == code


def test_loader_rejects_duplicate_noncanonical_and_symlink_inputs(tmp_path: Path) -> None:
    duplicate = tmp_path / "duplicate.json"
    duplicate.write_text('{"binding":"python","binding":"python"}', encoding="utf-8")
    with pytest.raises(corpora.CorpusError) as rejected:
        corpora.load_corpus(duplicate, "python")
    assert rejected.value.code == "duplicate_corpus_key"

    noncanonical = tmp_path / "noncanonical.json"
    noncanonical.write_text(json.dumps(corpus("python")), encoding="utf-8")
    with pytest.raises(corpora.CorpusError) as rejected:
        corpora.load_corpus(noncanonical, "python")
    assert rejected.value.code == "noncanonical_corpus_json"

    target = tmp_path / "target.json"
    write(target, corpus("python"))
    symlink = tmp_path / "corpus.json"
    symlink.symlink_to(target)
    with pytest.raises(corpora.CorpusError) as rejected:
        corpora.load_corpus(symlink, "python")
    assert rejected.value.code == "invalid_corpus_file"


def test_comparison_rejects_record_and_archive_divergence(tmp_path: Path) -> None:
    inputs = paths(tmp_path)
    node = corpus("node")
    node["record_b64"][4] = base64.b64encode(b"different").decode("ascii")
    write(inputs["node"], node)
    with pytest.raises(corpora.CorpusError) as rejected:
        corpora.compare(inputs)
    assert rejected.value.code == "record_bytes_mismatch"

    write(inputs["node"], corpus("node"))
    c_value = corpus("c")
    c_value["archive_b64"] = base64.b64encode(b"different").decode("ascii")
    write(inputs["c"], c_value)
    with pytest.raises(corpora.CorpusError) as rejected:
        corpora.compare(inputs)
    assert rejected.value.code == "archive_bytes_mismatch"
