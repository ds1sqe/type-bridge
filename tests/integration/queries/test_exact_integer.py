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

    assert [row["n"]["value"] for row in rows] == [BEYOND_F64]
    assert documents == [{"n": BEYOND_F64}]
