"""Integers beyond 2^53 round-trip exactly through rows and fetch documents.

Older official drivers render concept documents through an ``f64`` JSON
number; TypeBridge reads typed concept values, so the value is exact on every
supported server and driver band.
"""

import pytest

from type_bridge import Database

pytestmark = pytest.mark.integration

# 2^53 + 1: the first integer an f64 cannot represent.
BEYOND_F64 = 9_007_199_254_740_993


def test_integers_beyond_two_to_the_53_round_trip_exactly(clean_db: Database) -> None:
    clean_db.execute_query("define attribute n, value integer; entity p, owns n;", "schema")
    clean_db.execute_query(f"insert $p isa p, has n {BEYOND_F64};", "write")

    rows = clean_db.execute_query("match $p isa p, has n $n; select $n;", "read")
    documents = clean_db.execute_query("match $p isa p; fetch { 'n': $p.n };", "read")

    assert [row["data"]["n"]["value"] for row in rows["answers"]] == [BEYOND_F64]
    assert documents["answers"] == [{"n": BEYOND_F64}]


def test_caught_raw_query_error_prevents_commit(clean_db: Database) -> None:
    from type_bridge import RawQueryError

    clean_db.execute_query("define entity raw-failure-test;", "schema")
    with pytest.raises(RawQueryError) as failed_commit:
        with clean_db.raw_transaction("write") as transaction:
            transaction.execute_query("insert $x isa raw-failure-test;")
            with pytest.raises(RawQueryError):
                transaction.execute_query("insert $x isa no-such-type;")
    assert failed_commit.value.code == "raw_transaction_failed"
    assert (
        clean_db.execute_query("match $x isa raw-failure-test; fetch { 'iid': iid($x) };")[
            "answers"
        ]
        == []
    )
