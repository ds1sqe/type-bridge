"""Live raw provider failures and commit certainty through the Python SDK."""

import json
import os
import subprocess
from concurrent.futures import ThreadPoolExecutor
from contextlib import suppress
from pathlib import Path
from uuid import uuid4

import pytest
from type_bridge_core import PyDatabase

from type_bridge import RawQueryError

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
    db = PyDatabase.connect(
        address, f"tb_failure_python_{uuid4().hex}", http_port=int(os.environ["TYPEDB_HTTP_PORT"])
    )
    db.create_database()
    try:
        _raw_provider_failure(db, case, container)
    finally:
        db.delete_database()
        db.close()


@pytest.mark.parametrize("case", ["stc2", "cnt5", "cnt9"])
def test_python_raw_provider_failure_without_pause(clean_db, case: str) -> None:
    _raw_provider_failure(clean_db, case, None)


def _raw_provider_failure(db, case: str, container: str | None) -> None:
    contexts = []
    try:
        schema = db.raw_transaction("schema")
        contexts.append(schema)
        schema.execute_query(FIXTURE["schema"])
        schema.commit()
        seed = db.raw_transaction("write")
        contexts.append(seed)
        seed.execute_query(FIXTURE["seed"])
        seed.commit()
        tx = db.raw_transaction("write")
        contexts.append(tx)
        if case == "stc2":
            other = db.raw_transaction("write")
            contexts.append(other)
            tx.execute_query(FIXTURE["purchase"])
            other.execute_query(FIXTURE["purchase"])
            with ThreadPoolExecutor(max_workers=2) as pool:
                futures = [pool.submit(context.commit) for context in [tx, other]]
                errors = [future.exception() for future in futures]
            assert sum(error is None for error in errors) == 1
            error = next(error for error in errors if error is not None)
            assert isinstance(error, RawQueryError)
            read = db.raw_transaction("read")
            contexts.append(read)
            assert read.execute_query(FIXTURE["stock_read"])["answers"] == [{"stock": 0}]
        elif case == "cnt5":
            tx.execute_query(FIXTURE["cnt5"])
            with pytest.raises(RawQueryError) as caught:
                tx.commit()
            error = caught.value
        elif case == "cnt9":
            with pytest.raises(RawQueryError) as caught:
                tx.execute_query(FIXTURE["cnt9"])
            error = caught.value
        else:
            tx.execute_query(FIXTURE["paused"])
            assert container is not None
            subprocess.run(["docker", "pause", container], check=True)
            try:
                with pytest.raises(RawQueryError) as caught:
                    tx.commit()
                error = caught.value
            finally:
                subprocess.run(["docker", "unpause", container], check=True)
        assert isinstance(error, RawQueryError)
        assert (
            error.code
            == {"stc2": "STC2", "cnt5": "CNT5", "cnt9": "CNT9", "paused": "commit_outcome_unknown"}[
                case
            ]
        )
        assert error.commit_outcome == (
            "unknown" if case == "paused" else None if case == "cnt9" else "definitely_aborted"
        )
        if case == "paused":
            assert error.provider_code
        else:
            assert error.provider_code is None
        print(case, error.code, error.provider_code, error.commit_outcome)
    finally:
        for context in contexts:
            with suppress(RuntimeError):
                context.close()
