"""Cache cleanup keeps what CI restores and deletes what no run can use."""

from __future__ import annotations

import datetime as dt
import importlib.util
import json
import sys
from collections.abc import Sequence
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location("cache_cleanup", ROOT / "scripts/ci/cache_cleanup.py")
assert SPEC and SPEC.loader
cleanup = importlib.util.module_from_spec(SPEC)
sys.modules["cache_cleanup"] = cleanup
SPEC.loader.exec_module(cleanup)

NOW = dt.datetime(2026, 10, 20, tzinfo=dt.UTC)
LOCK = "a" * 64


def cache(key: str, ref: str, created: str, used: str = "2026-10-19T00:00:00Z", id: int = 1):
    return {
        "id": id,
        "key": key,
        "ref": ref,
        "created_at": created,
        "last_accessed_at": used,
        "size_in_bytes": 1_000_000,
    }


def test_family_drops_only_a_numeric_run_id() -> None:
    assert cleanup.family(f"rust-Linux-main-{LOCK}-39352672726") == f"rust-Linux-main-{LOCK}"
    # npm and uv keys end in a hash, not a run id, and stay whole.
    assert (
        cleanup.family("node-cache-Linux-x64-npm-62224eb6") == "node-cache-Linux-x64-npm-62224eb6"
    )


def test_master_keeps_the_newest_entries_of_each_family() -> None:
    caches = [
        cache(f"rust-Linux-main-{LOCK}-{run}", cleanup.MASTER, f"2026-10-1{run}T00:00:00Z", id=run)
        for run in (1, 2, 3, 4)
    ] + [
        cache(f"rust-Windows-main-{LOCK}-9", cleanup.MASTER, "2026-10-10T00:00:00Z", id=9),
        cache(f"rust-Linux-main-{LOCK}-5", "refs/pull/7/merge", "2026-10-01T00:00:00Z", id=5),
    ]
    assert [c["id"] for c in cleanup.superseded(caches, keep=2)] == [2, 1]


def test_master_drops_families_of_a_superseded_lock_once_the_current_one_exists() -> None:
    old, new, npm = "b" * 64, "c" * 64, "d" * 64
    caches = [
        cache(f"rust-Linux-main-{old}-1", cleanup.MASTER, "2026-10-10T00:00:00Z", id=1),
        cache(f"rust-Linux-main-{new}-2", cleanup.MASTER, "2026-10-11T00:00:00Z", id=2),
        cache(f"Linux-cargo-perf-v1-{old}", cleanup.MASTER, "2026-10-10T00:00:00Z", id=3),
        cache(f"Linux-cargo-perf-v1-{new}", cleanup.MASTER, "2026-10-11T00:00:00Z", id=4),
        # No current-lock entry yet: the old one is still what CI restores.
        cache(f"rust-macOS-main-{old}-1", cleanup.MASTER, "2026-10-10T00:00:00Z", id=5),
        # A digest that is not the Cargo lock hash stays.
        cache(f"node-cache-Linux-x64-npm-{npm}", cleanup.MASTER, "2026-10-10T00:00:00Z", id=6),
        cache(f"rust-Linux-main-{old}-7", "refs/pull/7/merge", "2026-10-10T00:00:00Z", id=7),
    ]
    assert [c["id"] for c in cleanup.of_old_lock(caches, new)] == [1, 3]


def test_lock_hash_matches_github_hash_files() -> None:
    # hashFiles('x') of one file is sha256 over the file's sha256 digest.
    assert (
        cleanup.lock_hash(b"") == "5df6e0e2761359d30a8275058e299fcc0381534545f55cf43e41983f5d4c9456"
    )


def test_a_closed_pull_request_loses_every_cache_and_nothing_else() -> None:
    caches = [
        cache("a", "refs/pull/7/merge", "2026-10-01T00:00:00Z", id=1),
        cache("b", "refs/pull/70/merge", "2026-10-01T00:00:00Z", id=2),
        cache("c", cleanup.MASTER, "2026-10-01T00:00:00Z", id=3),
    ]
    assert [c["id"] for c in cleanup.of_pull_request(caches, 7)] == [1]


def test_sweep_removes_gone_and_idle_refs_but_never_master() -> None:
    live = {"refs/heads/feature", "refs/pull/8/merge"}
    caches = [
        cache("gone", "refs/heads/merged-away", "2026-10-18T00:00:00Z", id=1),
        cache("idle", "refs/heads/feature", "2026-10-01T00:00:00Z", "2026-10-01T00:00:00Z", 2),
        cache("fresh", "refs/heads/feature", "2026-10-18T00:00:00Z", id=3),
        cache("open", "refs/pull/8/merge", "2026-10-18T00:00:00Z", id=4),
        cache("tag", "refs/heads/refs/tags/v2.2.6", "2026-10-19T00:00:00Z", id=5),
        cache("old-master", cleanup.MASTER, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z", 6),
    ]
    assert [c["id"] for c in cleanup.orphaned(caches, live, NOW)] == [1, 2, 5]


class FakeGh:
    def __init__(self, caches: list[dict[str, object]]) -> None:
        self.caches = caches
        self.calls: list[list[str]] = []

    def __call__(self, arguments: Sequence[str]) -> list[str]:
        self.calls.append(list(arguments))
        if arguments[:2] == ["api", "--paginate"] and "actions/caches" in arguments[2]:
            return [json.dumps(c) for c in self.caches]
        if "branches" in arguments[2]:
            return ["master"]
        if "pulls" in arguments[2]:
            return []
        return []


def test_main_deletes_through_the_api_and_dry_run_deletes_nothing(
    capsys: pytest.CaptureFixture[str],
) -> None:
    caches = [
        cache(f"rust-Linux-main-{LOCK}-{run}", cleanup.MASTER, f"2026-10-1{run}T00:00:00Z", id=run)
        for run in (1, 2, 3)
    ]
    gh = FakeGh(caches)
    assert cleanup.main(["--repo", "o/r", "--mode", "master", "--dry-run"], run=gh) == 0
    assert not [call for call in gh.calls if "DELETE" in call]
    assert "1 caches, 0.00 GB would be freed" in capsys.readouterr().out

    gh = FakeGh(caches)
    assert cleanup.main(["--repo", "o/r", "--mode", "master"], run=gh) == 0
    assert [call for call in gh.calls if "DELETE" in call] == [
        ["api", "-X", "DELETE", "repos/o/r/actions/caches/1"]
    ]


def test_pr_mode_requires_a_number() -> None:
    with pytest.raises(SystemExit):
        cleanup.main(["--repo", "o/r", "--mode", "pr"], run=FakeGh([]))


def test_workflow_wires_each_event_to_its_mode() -> None:
    workflow = yaml.safe_load((ROOT / ".github/workflows/cache-cleanup.yml").read_text())
    events = workflow[True]
    assert events["pull_request_target"] == {"types": ["closed"]}
    assert events["workflow_run"]["workflows"] == ["CI"]
    assert events["workflow_run"]["branches"] == ["master"]
    assert "schedule" in events
    assert workflow["permissions"]["actions"] == "write"
    steps = workflow["jobs"]["cleanup"]["steps"]
    # Only base-branch code runs with the write token.
    assert steps[0]["with"]["ref"] == "${{ github.event.repository.default_branch }}"
    script = steps[-1]["run"]
    for mode in ("--mode pr", "--mode master", "--mode sweep"):
        assert mode in script


def test_ci_writes_caches_only_from_master_and_restores_by_prefix() -> None:
    workflow = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text())
    for job_id, job in workflow["jobs"].items():
        for step in job.get("steps", []):
            uses = step.get("uses", "")
            assert not uses.startswith("actions/cache@"), f"{job_id} uses read-write actions/cache"
            if uses.startswith("actions/cache/save@"):
                assert (
                    step["if"] == "github.event_name == 'push' && github.ref == 'refs/heads/master'"
                )
                assert step["with"]["key"].endswith("-${{ github.run_id }}")
            if uses.startswith("actions/cache/restore@"):
                assert step["with"]["key"].endswith("-${{ github.run_id }}")
                assert step["with"]["restore-keys"].strip()
                assert ".." not in step["with"]["path"]
