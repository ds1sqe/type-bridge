"""Race staged plans from separate processes on a live TypeDB (#253).

Each racer is a `spawn` process with its own connection and staged
transaction. It builds its plans, stages them, waits on a barrier with the
other racers, and then commits. A staged transaction opens nothing until
commit, so the barrier releases three commits that overlap on the server.
When the host is slow they can still run one after another; every scenario
accepts the serial outcome too and checks its invariant on the final state.

The plans use hand-made tokens for the schema in `workspace/` and a
query-only authority over the workspace's declared schema, which
`type-bridge schema export-declared` writes.
"""

from __future__ import annotations

import json
import multiprocessing
import os
import subprocess
import threading
import time
import uuid
from collections import Counter
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass
from multiprocessing.synchronize import Barrier
from pathlib import Path
from queue import Empty
from typing import Any, Protocol

from type_bridge_core import PyRustDatabase, QueryV2Authority, query_v2_query_only_authority

from type_bridge.plan import Count, Matched, Plan

WORKSPACE = Path(__file__).parent / "workspace"
SCOPE = "live-races"
PROCESSES = 3
DEFAULT_ROUNDS = 5
BARRIER_SECONDS = 120.0
RESULT_SECONDS = 300.0

# The workspace schema as TypeDB receives it. The query-only authority
# compares the live schema with the declared one exactly, so any drift from
# workspace/schema/fragments/races.yaml fails every racer's commit.
SCHEMA_TYPEQL = """define
attribute item-id, value string; attribute stock, value integer; attribute price, value integer;
attribute account-id, value string; attribute balance, value integer;
attribute order-id, value string; attribute name, value string; attribute on-call, value boolean;
attribute shift-id, value string;
entity item, owns item-id @key, owns stock @card(0..1), owns price @card(0..1), plays purchase:item;
entity account, owns account-id @key, owns balance @card(0..1), plays purchase:buyer;
entity doctor, owns name @key, owns on-call @card(0..1), plays assignment:assignee @card(0..1);
entity shift, owns shift-id @key, plays assignment:target;
relation purchase, relates buyer, relates item, owns order-id @key;
relation assignment, relates assignee, relates target;
"""


def rounds(environ: Mapping[str, str]) -> int:
    """Rounds per scenario: TB_RACE_ROUNDS, or 5."""
    raw = environ.get("TB_RACE_ROUNDS", "").strip()
    if not raw:
        return DEFAULT_ROUNDS
    value = int(raw)
    if value < 1:
        raise ValueError(f"TB_RACE_ROUNDS must be at least 1, got {value}")
    return value


@dataclass(frozen=True)
class Target:
    """One live TypeDB database and the declared schema its racers trust."""

    address: str
    http_port: int
    database: str
    declared: bytes
    profile: str


@dataclass(frozen=True)
class Outcome:
    """One racer's commit, as plain data that crosses the process boundary.

    `kind` is `committed` (with each staged plan's matched count),
    `conflict` (CommitConflict), `rejected` (CommitRejected or a typed
    query-phase failure), `unknown` (CommitOutcomeUnknown) or `error`.
    """

    kind: str
    code: str | None = None
    phase: str | None = None
    matched: tuple[int, ...] = ()
    error_class: str | None = None
    retry_safe: bool | None = None
    detail: str = ""

    @property
    def label(self) -> str:
        """`committed:1,0`, `conflict:STC2`, `rejected:CNT5`, ..."""
        if self.kind == "committed":
            return "committed:" + ",".join(str(count) for count in self.matched)
        return f"{self.kind}:{self.code}"


class _Failure(Protocol):
    code: str | None
    phase: str
    retry_safe: bool


def classify(error: BaseException) -> Outcome:
    """Map an SDK commit error to an Outcome by its class and typed failure."""
    from type_bridge.failure import CommitConflict, CommitOutcomeUnknown, CommitRejected

    failure: _Failure | None = getattr(error, "failure", None)
    if isinstance(error, CommitOutcomeUnknown):
        kind = "unknown"
    elif isinstance(error, CommitConflict):
        kind = "conflict"
    elif isinstance(error, CommitRejected) or (failure is not None and failure.code):
        kind = "rejected"
    else:
        kind = "error"
    return Outcome(
        kind,
        code=failure.code if failure is not None else None,
        phase=failure.phase if failure is not None else None,
        error_class=type(error).__name__,
        retry_safe=failure.retry_safe if failure is not None else None,
        detail=f"{type(error).__name__}: {error}",
    )


def tally(outcomes: Iterable[Outcome]) -> Counter[str]:
    """Count outcomes by label."""
    return Counter(outcome.label for outcome in outcomes)


def winners(outcomes: Iterable[Outcome]) -> int:
    """Racers that committed and whose first plan matched."""
    return sum(1 for o in outcomes if o.kind == "committed" and o.matched and o.matched[0] > 0)


# ------------------------------------------------------------------ tokens
# The generated token shapes, hand-made for the workspace schema.


def _model(kind: str, label: str) -> type[Any]:
    name = label.title().replace("-", "")
    return type(name, (), {"__type_id__": json.dumps({"kind": kind, "label": label})})


Item = _model("entity", "item")
Account = _model("entity", "account")
Doctor = _model("entity", "doctor")
Shift = _model("entity", "shift")
Purchase = _model("relation", "purchase")
Assignment = _model("relation", "assignment")


@dataclass(frozen=True)
class _Token:
    owner: type[Any]
    fact: Mapping[str, object]


def _field(owner: type[Any], attribute: str) -> _Token:
    return _Token(owner, {"id": {"owner": json.loads(owner.__type_id__), "attribute": attribute}})


def _role(owner: type[Any], relation: str, label: str) -> _Token:
    return _Token(owner, {"role": {"declaring_relation": relation, "label": label}})


# ------------------------------------------------------------------- plans


def purchase(authority: QueryV2Authority, order: str, account: str) -> Plan:
    """The one-query purchase of item I1: stock and balance checked and taken."""
    plan = Plan.builder()
    item = plan.exact(Item)
    buyer = plan.exact(Account)
    stock = plan.field(item, _field(Item, "stock"))
    price = plan.field(item, _field(Item, "price"))
    balance = plan.field(buyer, _field(Account, "balance"))
    plan.where_(plan.field(item, _field(Item, "item-id")).eq("I1"))
    plan.where_(plan.field(buyer, _field(Account, "account-id")).eq(account))
    plan.where_(stock.gt(0)).where_(balance.ge(price))
    plan.update(item, _field(Item, "stock"), plan.let_("next_stock", stock - 1))
    plan.update(buyer, _field(Account, "balance"), plan.let_("next_balance", balance - price))
    (
        plan.insert(Purchase)
        .field(_field(Purchase, "order-id"), order)
        .player(_role(Purchase, "purchase", "buyer"), buyer)
        .player(_role(Purchase, "purchase", "item"), item)
    )
    return plan.build(authority)


def off_call(authority: QueryV2Authority, doctor: str, colleague: str, *, touch: bool) -> Plan:
    """Take `doctor` off call while `colleague` is on call.

    The check reads the colleague's on-call ownership and writes nothing
    there, so concurrent racers write-skew. With `touch` the plan also
    rewrites that ownership with its own value, which takes its lock.
    """
    plan = Plan.builder()
    me = plan.exact(Doctor)
    other = plan.exact(Doctor)
    on_call = _field(Doctor, "on-call")
    plan.where_(plan.field(me, _field(Doctor, "name")).eq(doctor))
    plan.where_(plan.field(other, _field(Doctor, "name")).eq(colleague))
    plan.where_(plan.field(me, on_call).eq(True))
    other_on_call = plan.field(other, on_call)
    plan.where_(other_on_call.eq(True))
    plan.update(me, on_call, False)
    if touch:
        plan.update(other, on_call, other_on_call)
    return plan.build(authority)


def assign(authority: QueryV2Authority, doctor: str, shift: str) -> Plan:
    """Assign `doctor` to `shift`; `plays assignee @card(0..1)` is the rule."""
    plan = Plan.builder()
    assignee = plan.exact(Doctor)
    target = plan.exact(Shift)
    plan.where_(plan.field(assignee, _field(Doctor, "name")).eq(doctor))
    plan.where_(plan.field(target, _field(Shift, "shift-id")).eq(shift))
    (
        plan.insert(Assignment)
        .player(_role(Assignment, "assignment", "assignee"), assignee)
        .player(_role(Assignment, "assignment", "target"), target)
    )
    return plan.build(authority)


def claim_key(authority: QueryV2Authority, stock: int) -> Plan:
    """Insert item K1; `item-id @key` lets one racer have it."""
    plan = Plan.builder()
    plan.insert(Item).field(_field(Item, "item-id"), "K1").field(_field(Item, "stock"), stock)
    return plan.build(authority)


def bulk_items(authority: QueryV2Authority, prefix: str, rows: int) -> Plan:
    """Insert `rows` items with ids `prefix0`, `prefix1`, ..."""
    plan = Plan.builder()
    item_id = plan.input("item_id", "string")
    plan.insert(Item).field(_field(Item, "item-id"), item_id)
    for number in range(rows):
        plan.row([item_id.value(f"{prefix}{number}")])
    return plan.build(authority)


def count_of(authority: QueryV2Authority, model: type[Any], **equal: str | int | bool) -> Plan:
    """Count `model` instances whose fields equal the given values."""
    plan = Plan.builder()
    thing = plan.exact(model)
    for attribute, value in equal.items():
        plan.where_(plan.field(thing, _field(model, attribute.replace("_", "-"))).eq(value))
    return plan.count().build(authority)


# --------------------------------------------------------------- scenarios

DOCTORS = ("alice", "bob", "carol")


@dataclass(frozen=True)
class Scenario:
    """Seed TypeQL, and the plans racer `index` stages."""

    seed: tuple[str, ...]
    plans: Callable[[QueryV2Authority, int], list[Plan]]


def _skew(authority: QueryV2Authority, index: int, *, touch: bool) -> list[Plan]:
    # Cyclic: racer i reads doctor i+1 and takes doctor i off call.
    me, colleague = DOCTORS[index], DOCTORS[(index + 1) % len(DOCTORS)]
    return [off_call(authority, me, colleague, touch=touch)]


DOCTOR_SEED = tuple(
    f'insert $d isa doctor, has name "{name}", has on-call true;' for name in DOCTORS
)

SCENARIOS: dict[str, Scenario] = {
    "purchase": Scenario(
        (
            'insert $i isa item, has item-id "I1", has stock 1, has price 40;',
            *(
                f'insert $a isa account, has account-id "A{i}", has balance 100;'
                for i in range(PROCESSES)
            ),
        ),
        lambda authority, i: [purchase(authority, f"O{i}", f"A{i}")],
    ),
    "skew": Scenario(DOCTOR_SEED, lambda authority, i: _skew(authority, i, touch=False)),
    "touch": Scenario(DOCTOR_SEED, lambda authority, i: _skew(authority, i, touch=True)),
    "plays-card": Scenario(
        (
            'insert $d isa doctor, has name "alice", has on-call true;',
            *(f'insert $s isa shift, has shift-id "S{i}";' for i in range(PROCESSES + 1)),
        ),
        lambda authority, i: [assign(authority, "alice", f"S{i}")],
    ),
    "key": Scenario((), lambda authority, i: [claim_key(authority, i)]),
}


# -------------------------------------------------------------- live setup


def connect(target: Target) -> PyRustDatabase:
    return PyRustDatabase.connect(target.address, target.database, http_port=target.http_port)


def authority_for(db: PyRustDatabase, target: Target) -> QueryV2Authority:
    return query_v2_query_only_authority(db, target.declared, SCOPE, target.profile)


def fresh_database(base: Target, label: str, seed: Iterable[str]) -> Target:
    """Create a database with the schema and seed; return its target."""
    name = f"tb_race_{label.replace('-', '_')}_{os.getpid()}_{uuid.uuid4().hex[:8]}"
    target = Target(base.address, base.http_port, name, base.declared, base.profile)
    db = connect(target)
    try:
        db.create_database()
        schema = db.transaction("schema")
        schema.execute(SCHEMA_TYPEQL)
        schema.commit()
        queries = list(seed)
        if queries:
            write = db.transaction("write")
            for query in queries:
                write.execute(query)
            write.commit()
    finally:
        db.close()
    return target


DROP_SECONDS = 120.0


def drop_database(target: Target) -> None:
    """Delete the database, waiting out server-side transactions.

    After a pause the server can still hold the abandoned client's
    transaction for a while, and refuses the delete until it ends (DBD2).
    """
    deadline = time.monotonic() + DROP_SECONDS
    while True:
        db = connect(target)
        try:
            db.delete_database()
            return
        except RuntimeError:
            if time.monotonic() >= deadline:
                raise
        finally:
            db.close()
        time.sleep(1.0)


def read_counts(
    target: Target, plans: Mapping[str, Callable[[QueryV2Authority], Plan]]
) -> dict[str, int]:
    """Run count plans in one staged read transaction."""
    db = connect(target)
    try:
        authority = authority_for(db, target)
        with db.staged_transaction("read") as tx:
            pending = {name: tx.stage(build(authority)) for name, build in plans.items()}
        counts: dict[str, int] = {}
        for name, answer in pending.items():
            result = answer.result()
            assert isinstance(result, Count), f"{name} answered {result!r}"
            counts[name] = result.count
        return counts
    finally:
        db.close()


def commit_alone(target: Target, plans: Callable[[QueryV2Authority], list[Plan]]) -> Outcome:
    """Stage and commit plans in one transaction, with no racer."""
    db = connect(target)
    try:
        authority = authority_for(db, target)
        tx = db.staged_transaction("write")
        try:
            pending = [tx.stage(plan) for plan in plans(authority)]
            return _commit(tx, pending)
        finally:
            tx.close()
    finally:
        db.close()


def _commit(tx: Any, pending: list[Any]) -> Outcome:
    try:
        tx.commit()
    except Exception as error:  # noqa: BLE001 - the typed failure is the outcome
        return classify(error)
    matched = []
    for answer in pending:
        result = answer.result()
        matched.append(result.count if isinstance(result, Matched) else -1)
    return Outcome("committed", matched=tuple(matched))


# ------------------------------------------------------------------ racing


def _racer(target: Target, scenario: str, index: int, barrier: Barrier, results: Any) -> None:
    outcome = Outcome("error", detail="racer did not finish")
    try:
        db = connect(target)
        try:
            tx = db.staged_transaction("write")
            try:
                plans = SCENARIOS[scenario].plans(authority_for(db, target), index)
                pending = [tx.stage(plan) for plan in plans]
                barrier.wait(BARRIER_SECONDS)
                outcome = _commit(tx, pending)
            finally:
                tx.close()
        finally:
            db.close()
    except Exception as error:  # noqa: BLE001 - reported to the parent as an error outcome
        barrier.abort()
        outcome = Outcome("error", detail=f"{type(error).__name__}: {error}")
    finally:
        results.put((index, os.getpid(), outcome))


def race(target: Target, scenario: str) -> list[Outcome]:
    """Run one round: PROCESSES racers, one process and connection each."""
    context = multiprocessing.get_context("spawn")
    barrier = context.Barrier(PROCESSES)
    results = context.Queue()
    processes = [
        context.Process(target=_racer, args=(target, scenario, index, barrier, results))
        for index in range(PROCESSES)
    ]
    for process in processes:
        process.start()
    collected: dict[int, Outcome] = {}
    pids: set[int] = set()
    try:
        for _ in processes:
            index, pid, outcome = results.get(timeout=RESULT_SECONDS)
            collected[index] = outcome
            pids.add(pid)
    except Empty as error:
        raise AssertionError(f"{scenario}: a racer did not report: {collected!r}") from error
    finally:
        for process in processes:
            process.join(timeout=30)
            if process.is_alive():
                process.kill()
    assert pids.isdisjoint({os.getpid()}) and len(pids) == PROCESSES, "one process per racer"
    return [collected[index] for index in range(PROCESSES)]


# ----------------------------------------------------------- paused server

# One plan holds at most 4096 input rows; 25 plans of 4000 make a commit
# long enough (seconds) to pause the server inside it.
BULK_PLANS = 25
BULK_ROWS = 4000
BULK_ITEMS = BULK_PLANS * BULK_ROWS
PAUSE_ATTEMPTS = 12
COMMIT_SECONDS = 180.0


def _bulk(authority: QueryV2Authority) -> list[Plan]:
    return [bulk_items(authority, f"P{plan}-", BULK_ROWS) for plan in range(BULK_PLANS)]


def next_pause(
    delay: float, ceiling: float, outcome: Outcome, elapsed: float
) -> tuple[float, float]:
    """The next pause delay and commit-time ceiling after a missed pause.

    A commit that went through finished before the pause, so its time is
    the new ceiling and the next pause lands a little before it. A failure
    without a typed commit outcome paused before the commit was sent, so
    the next pause moves halfway towards the ceiling.
    """
    if outcome.kind == "committed":
        return elapsed * 0.85, elapsed
    return (delay + ceiling) / 2, ceiling


def timed_commit(target: Target) -> tuple[Outcome, float]:
    """Commit the bulk plans unpaused; return the outcome and its seconds."""
    started = time.perf_counter()
    outcome = commit_alone(target, _bulk)
    return outcome, time.perf_counter() - started


def commit_while_paused(target: Target, container: str, delay: float) -> tuple[Outcome, float]:
    """Start the bulk commit, `docker pause` the container after `delay`
    seconds, and unpause it once the commit has returned. Returns the
    outcome and the seconds the commit took."""
    box: dict[str, Outcome] = {}
    finished: list[float] = []

    def commit() -> None:
        box["outcome"] = _commit(tx, pending)
        finished.append(time.perf_counter())

    db = connect(target)
    try:
        tx = db.staged_transaction("write")
        try:
            pending = [tx.stage(plan) for plan in _bulk(authority_for(db, target))]
            started = time.perf_counter()
            thread = threading.Thread(target=commit, daemon=True)
            thread.start()
            time.sleep(delay)
            subprocess.run(["docker", "pause", container], check=True, capture_output=True)
            try:
                thread.join(COMMIT_SECONDS)
            finally:
                subprocess.run(["docker", "unpause", container], check=True, capture_output=True)
            thread.join(COMMIT_SECONDS)
            if thread.is_alive():
                raise AssertionError("the paused commit never returned")
        finally:
            tx.close()
    finally:
        db.close()
    return box["outcome"], finished[0] - started
