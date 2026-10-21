"""Live ordinary-commit failure projection through the Python native SDK."""

import json
import os
import subprocess
from concurrent.futures import ThreadPoolExecutor
from contextlib import suppress
from dataclasses import FrozenInstanceError
from pathlib import Path
from uuid import uuid4

import pytest
from type_bridge_core import PyRustDatabase

from type_bridge.failure import CommitConflict, CommitOutcomeUnknown, CommitRejected, Failure

pytestmark = pytest.mark.integration

FIXTURE = json.loads(
    (Path(__file__).resolve().parents[1] / "contracts/typed-failure-scenarios.json").read_bytes()
)


@pytest.mark.parametrize("case", ["stc2", "cnt5", "cnt9", "paused"])
def test_python_typed_provider_failure(case: str) -> None:
    address = os.getenv("TYPEDB_ADDRESS")
    container = os.getenv("TYPE_BRIDGE_FAILURE_CONTAINER")
    if not address or not container:
        pytest.skip("requires isolated failure-smoke environment and owned pause container")
    assert container.startswith("tb-failure-smoke-")
    db = PyRustDatabase.connect(
        address, f"tb_failure_python_{uuid4().hex}", http_port=int(os.environ["TYPEDB_HTTP_PORT"])
    )
    contexts = []
    db.create_database()
    try:
        schema = db.transaction("schema")
        contexts.append(schema)
        schema.execute(FIXTURE["schema"])
        schema.commit()
        seed = db.transaction("write")
        contexts.append(seed)
        seed.execute(FIXTURE["seed"])
        seed.commit()
        tx = db.transaction("write")
        contexts.append(tx)
        if case == "stc2":
            other = db.transaction("write")
            contexts.append(other)
            tx.execute(FIXTURE["purchase"])
            other.execute(FIXTURE["purchase"])
            with ThreadPoolExecutor(max_workers=2) as pool:
                futures = [pool.submit(context.commit) for context in [tx, other]]
                errors = [future.exception() for future in futures]
            assert sum(error is None for error in errors) == 1
            error = next(error for error in errors if error is not None)
            assert isinstance(error, CommitConflict)
            read = db.transaction("read")
            contexts.append(read)
            assert read.execute(FIXTURE["stock_read"]) == [{"stock": 0}]
        elif case == "cnt5":
            tx.execute(FIXTURE["cnt5"])
            with pytest.raises(CommitRejected) as caught:
                tx.commit()
            error = caught.value
        elif case == "cnt9":
            with pytest.raises(RuntimeError) as caught:
                tx.execute(FIXTURE["cnt9"])
            error = caught.value
        else:
            tx.execute(FIXTURE["paused"])
            subprocess.run(["docker", "pause", container], check=True)
            try:
                with pytest.raises(CommitOutcomeUnknown) as caught:
                    tx.commit()
                error = caught.value
            finally:
                subprocess.run(["docker", "unpause", container], check=True)
        failure: Failure = getattr(error, "failure")
        assert isinstance(failure, Failure)
        assert failure.retry_safe == (case != "paused")
        assert failure.phase == ("query" if case == "cnt9" else "commit")
        assert (
            failure.reason
            == {"stc2": "conflict", "cnt5": "cardinality", "cnt9": "unique", "paused": None}[case]
        )
        if case == "stc2":
            assert failure.lock_kind == "exclusive"
        assert "SECRET_SENTINEL" not in f"{error!s} {error!r} {failure!r}"
        with pytest.raises(FrozenInstanceError):
            setattr(failure, "retry_safe", False)
        print(case, failure)
    finally:
        for context in contexts:
            with suppress(RuntimeError):
                context.close()
        db.delete_database()
        db.close()
