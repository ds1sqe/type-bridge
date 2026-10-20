"""Delete GitHub Actions caches that no later run can use.

CI writes caches only from master, with keys ``<family>-<run id>`` (for
example ``rust-Linux-main-<lock hash>-<run id>``) and restores the newest one
by prefix. Older entries of a family, caches of closed pull requests and
caches of branches that are gone only fill the 10 GB quota and make GitHub
evict useful entries, so this removes them:

- ``--mode pr --pr N``: every cache of ``refs/pull/N/merge``.
- ``--mode master``: all but the newest ``--keep`` entries of each master
  family.
- ``--mode sweep``: the master rule, plus caches of refs that no longer
  exist (deleted branches, closed pull requests) and of any non-master ref
  idle for ``--max-idle-days``.

The selection functions are pure; ``main`` lists and deletes through ``gh``.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import subprocess
import sys
from collections import defaultdict
from collections.abc import Callable, Iterable, Sequence
from typing import Any

MASTER = "refs/heads/master"
RUN_ID = re.compile(r"-\d+$")

Cache = dict[str, Any]


def family(key: str) -> str:
    """The key without its trailing ``-<run id>``."""
    return RUN_ID.sub("", key)


def _created(cache: Cache) -> str:
    return str(cache["created_at"])


def superseded(caches: Iterable[Cache], keep: int = 2) -> list[Cache]:
    """Master caches beyond the newest ``keep`` of each family."""
    groups: dict[str, list[Cache]] = defaultdict(list)
    for cache in caches:
        if cache["ref"] == MASTER:
            groups[family(cache["key"])].append(cache)
    stale: list[Cache] = []
    for entries in groups.values():
        entries.sort(key=_created, reverse=True)
        stale.extend(entries[keep:])
    return stale


def of_pull_request(caches: Iterable[Cache], number: int) -> list[Cache]:
    """Every cache written by pull request ``number``."""
    ref = f"refs/pull/{number}/merge"
    return [cache for cache in caches if cache["ref"] == ref]


def orphaned(
    caches: Iterable[Cache], live_refs: set[str], now: dt.datetime, max_idle_days: int = 7
) -> list[Cache]:
    """Non-master caches whose ref is gone or that no run used recently."""
    cutoff = now - dt.timedelta(days=max_idle_days)
    stale: list[Cache] = []
    for cache in caches:
        if cache["ref"] == MASTER:
            continue
        used = dt.datetime.fromisoformat(str(cache["last_accessed_at"]).replace("Z", "+00:00"))
        if cache["ref"] not in live_refs or used < cutoff:
            stale.append(cache)
    return stale


def _gh_lines(arguments: Sequence[str]) -> list[str]:
    result = subprocess.run(["gh", *arguments], check=True, capture_output=True, text=True)
    return [line for line in result.stdout.splitlines() if line]


def list_caches(repo: str, run: Callable[[Sequence[str]], list[str]] = _gh_lines) -> list[Cache]:
    lines = run(
        [
            "api",
            "--paginate",
            f"repos/{repo}/actions/caches?per_page=100",
            "--jq",
            ".actions_caches[]",
        ]
    )
    return [json.loads(line) for line in lines]


def live_refs(repo: str, run: Callable[[Sequence[str]], list[str]] = _gh_lines) -> set[str]:
    branches = run(["api", "--paginate", f"repos/{repo}/branches?per_page=100", "--jq", ".[].name"])
    pulls = run(
        ["api", "--paginate", f"repos/{repo}/pulls?state=open&per_page=100", "--jq", ".[].number"]
    )
    return {f"refs/heads/{name}" for name in branches} | {f"refs/pull/{n}/merge" for n in pulls}


def main(
    argv: Sequence[str] | None = None,
    *,
    run: Callable[[Sequence[str]], list[str]] = _gh_lines,
    now: Callable[[], dt.datetime] = lambda: dt.datetime.now(dt.UTC),
) -> int:
    parser = argparse.ArgumentParser(description="Delete GitHub Actions caches no run can use.")
    parser.add_argument("--repo", required=True, help="owner/name")
    parser.add_argument("--mode", required=True, choices=["pr", "master", "sweep"])
    parser.add_argument("--pr", type=int, help="pull request number for --mode pr")
    parser.add_argument("--keep", type=int, default=2, help="newest master entries kept per family")
    parser.add_argument("--max-idle-days", type=int, default=7)
    parser.add_argument("--dry-run", action="store_true")
    arguments = parser.parse_args(argv)
    if arguments.mode == "pr" and arguments.pr is None:
        parser.error("--mode pr needs --pr")
    if arguments.keep < 1:
        parser.error("--keep must be at least 1")

    caches = list_caches(arguments.repo, run)
    if arguments.mode == "pr":
        doomed = of_pull_request(caches, arguments.pr)
    else:
        doomed = superseded(caches, arguments.keep)
        if arguments.mode == "sweep":
            doomed += orphaned(
                caches, live_refs(arguments.repo, run), now(), arguments.max_idle_days
            )

    freed = 0
    for cache in doomed:
        print(f"delete {cache['ref']} {cache['key']} ({cache['size_in_bytes']} bytes)")
        freed += int(cache["size_in_bytes"])
        if not arguments.dry_run:
            run(["api", "-X", "DELETE", f"repos/{arguments.repo}/actions/caches/{cache['id']}"])
    print(
        f"{len(doomed)} caches, {freed / 1e9:.2f} GB {'would be ' if arguments.dry_run else ''}freed"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
