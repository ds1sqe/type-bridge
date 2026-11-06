import asyncio
import os
import time

from callback_models import Gender, Person, QuerySession, open_migration_catalog
from type_bridge_core import MigrationCancellation

from type_bridge.migration import MigrationTransaction

name = os.environ["TYPE_BRIDGE_CALLBACK_DATABASE"]
db = Person.__runtime_projection__.connect_direct(
    os.environ["TYPEDB_ADDRESS"],
    name,
    os.environ.get("TYPEDB_USERNAME", "admin"),
    os.environ.get("TYPEDB_PASSWORD", "password"),
    int(os.environ["TYPEDB_HTTP_PORT"]),
)
assert db.inspect_database_pair() == "absent"
db.create_database_outcome()
catalog = open_migration_catalog()
ids = [catalog.entry(i).id for i in range(len(catalog))]


def plan(applied, targets, rollback=False):
    p = (
        catalog.preview_rollback(applied, targets)
        if rollback
        else catalog.preview_apply(applied, targets)
    )
    b = p.approval_builder()
    for i in range(p.migration_count()):
        if p.migration(i).safety in ("opaque", "destructive"):
            b.approve(i)
    return p.authorize(b.finish())


print(plan([], [ids[0]]).execute(db, "python-initial").status)
db.execute_query('insert $p isa person, has ssn "1231"; $q isa person, has ssn "1232";', "write")
escaped = []
fail = True
cancellation = None


def derive(tx: MigrationTransaction):
    escaped.append(tx)
    manager = Person.manager(tx)
    people = manager.all()
    assert len(people) == 2
    session = QuerySession(tx)
    binding = session.exact(Person)
    assert asyncio.run(session.query(binding).count_by(binding)) == 2
    for p in people:
        p.gender = Gender("male" if int(p.ssn.value[-1]) % 2 else "female")
        manager.update(p)
    if fail:
        raise RuntimeError("private failure detail")
    if cancellation is not None:
        cancellation.cancel()
        time.sleep(0.05)
        return
    print(
        "callback raw",
        tx.execute_query('match $p isa person; fetch { "ssn": $p.ssn, "gender": $p.gender };'),
    )


def reverse(tx: MigrationTransaction):
    tx.execute_query("match $p isa person, has gender $g; delete has $g of $p;")


catalog.register("derive-gender", derive, reverse)
try:
    plan([ids[0]], [ids[1]]).execute(db, "python-error")
except Exception as e:
    assert "migration_user_step_failed" in str(e) and "RuntimeError: private failure detail" in str(e), str(e)
else:
    raise AssertionError("callback error accepted")
assert not db.execute_query('match $p isa person, has gender $g; fetch {"gender":$g};', "read")[
    "answers"
]
fail = False
cancellation = MigrationCancellation()
try:
    plan([ids[0]], [ids[1]]).execute_controlled(db, "python-cancel", cancellation=cancellation)
except Exception as error:
    assert "migration_execution_cancelled" in str(error), str(error)
else:
    raise AssertionError("cancelled callback committed")
cancellation = None
assert not db.execute_query('match $p isa person, has gender $g; fetch {"gender":$g};', "read")[
    "answers"
]
print(plan([ids[0]], [ids[1]]).execute(db, "python-data").status)
try:
    escaped[0].execute_query('match $p isa person; fetch { "ssn": $p.ssn };')
except Exception as e:
    assert "closed" in str(e).lower(), str(e)
else:
    raise AssertionError("escaped transaction survived")
print(plan(ids, [ids[1]], True).execute(db, "python-reverse").status)
print(plan([ids[0]], [ids[1]]).execute(db, "python-reapply").status)
print("PASS Python generated typed managers + raw + reverse + invalid escaped handle")
