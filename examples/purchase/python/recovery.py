"""Commit the purchase plan and recover from its typed failures.

The concurrency guide (docs/guide/concurrency.md) shows these functions;
tests/unit/docs/test_concurrency_guide.py runs them. `plan` is the
purchase built by `program.purchase`.
"""

from __future__ import annotations

import random
import time

from type_bridge_core import ProxyConnection

from type_bridge import CommitConflict, Database
from type_bridge.plan import Matched, Plan


# --8<-- [start:direct]
def buy(db: Database, plan: Plan, *, attempts: int = 5) -> bool:
    """Commit the purchase; True when it bought, False when a condition failed."""
    for attempt in range(attempts):
        try:
            with db.staged_transaction("write") as tx:
                bought = tx.stage(plan)  # nothing is sent yet
            # Committed on exit. Zero matched rows: out of stock or short of
            # funds, and nothing was written.
            return bought.result() == Matched(1)
        except CommitConflict:
            # Another buyer committed first and nothing was stored. Run the
            # whole plan again on a fresh snapshot, after a jittered backoff.
            if attempt + 1 == attempts:
                raise
            time.sleep(random.uniform(0, 0.01 * 2**attempt))
    raise ValueError("attempts must be at least 1")


# --8<-- [end:direct]


# --8<-- [start:proxy]
def buy_over_proxy(proxy: ProxyConnection, plan: Plan) -> bool:
    """Commit once over the proxy; resend the same request if the reply is lost."""
    tx = proxy.transaction("write")
    try:
        bought = tx.stage(plan)
        try:
            tx.commit()
        except Exception:
            if not tx.has_unknown_outcome():
                raise
            # Same request, same idempotency key: the server replays the
            # reply it kept, so the purchase applies at most once.
            tx.commit_retry()
        return bought.result() == Matched(1)
    finally:
        tx.close()


# --8<-- [end:proxy]
