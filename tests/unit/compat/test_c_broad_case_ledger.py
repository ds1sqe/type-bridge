"""Fail-closed tests for the Plan 08 cross-slice broad-case ledger."""

from __future__ import annotations

import copy
import importlib.util
import json
import shutil
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts/ci/validate_c_broad_case_ledger.py"
SPEC = importlib.util.spec_from_file_location("c_broad_case_ledger", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
VALIDATOR = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = VALIDATOR
SPEC.loader.exec_module(VALIDATOR)


def _stage(tmp_path: Path) -> tuple[Path, Path]:
    ledger = json.loads(VALIDATOR.DEFAULT_LEDGER.read_text(encoding="utf-8"))
    for source in ledger["source_slices"].values():
        for authority in source["authorities"]:
            target = tmp_path / authority["path"]
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / authority["path"], target)
    path = tmp_path / "ledger.json"
    path.write_text(json.dumps(ledger), encoding="utf-8")
    return path, tmp_path


def _rewrite(path: Path, value: dict[str, object]) -> None:
    path.write_text(json.dumps(value), encoding="utf-8")


def test_accepts_exact_digest_bound_five_slice_ledger() -> None:
    ledger = VALIDATOR.validate()
    assert tuple(ledger["ledgers"]) == VALIDATOR.CAPABILITIES
    assert all(len(rows) == 5 for rows in ledger["ledgers"].values())
    assert ledger["publication_authority"] is False


def test_rejects_stale_authority_digest(tmp_path: Path) -> None:
    path, root = _stage(tmp_path)
    ledger = json.loads(path.read_text(encoding="utf-8"))
    ledger["source_slices"]["plan04-query-remote"]["authorities"][0]["sha256"] = "0" * 64
    _rewrite(path, ledger)
    with pytest.raises(VALIDATOR.LedgerError) as raised:
        VALIDATOR.validate(path, root)
    assert raised.value.code == "stale_authority_digest"


@pytest.mark.parametrize("capability", VALIDATOR.CAPABILITIES)
def test_rejects_one_missing_slice(tmp_path: Path, capability: str) -> None:
    path, root = _stage(tmp_path)
    ledger = json.loads(path.read_text(encoding="utf-8"))
    ledger["ledgers"][capability].pop()
    _rewrite(path, ledger)
    with pytest.raises(VALIDATOR.LedgerError) as raised:
        VALIDATOR.validate(path, root)
    assert raised.value.code == "incomplete_capability_ledger"


def test_rejects_publication_authority(tmp_path: Path) -> None:
    path, root = _stage(tmp_path)
    ledger = json.loads(path.read_text(encoding="utf-8"))
    hostile = copy.deepcopy(ledger)
    hostile["publication_authority"] = True
    _rewrite(path, hostile)
    with pytest.raises(VALIDATOR.LedgerError) as raised:
        VALIDATOR.validate(path, root)
    assert raised.value.code == "authority_widening"
